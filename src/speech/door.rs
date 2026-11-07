// The speech door's protocol: lines of `KEY value` pairs ended by a blank line, one connected program at a time (the screen reader's VOICE-INTERFACE.md).
// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

use super::phrases::Store;
use super::play::Cues;
use super::queue::{Engine, Priority, Request, Renderer};
use super::reader::{DEFAULT_VOICE, Reader, has_voice, unchecked};

const MAX_LINE: usize = 1 << 20;

const MAX_TEXT_CHARS: usize = 100_000;

static NEXT_CLIENT: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, PartialEq)]
pub enum Message {
    Say { id: String, priority: Priority, lang: String, voice: String, rate: u8, volume: u8, text: String },
    Stop,
    Pause,
    Resume,
    Cue { path: String, volume: u8 },
    Ping,
    Unknown(String),
}

fn number(fields: &HashMap<String, String>, key: &str, default: u8) -> u8 {
    fields.get(key).and_then(|v| v.parse::<u16>().ok()).map_or(default, |v| v.min(100) as u8)
}

pub fn parse(block: &str) -> Message {
    let mut lines = block.lines();
    let first = lines.next().unwrap_or("").trim_end();
    let (command, rest) = first.split_once(' ').unwrap_or((first, ""));
    let mut fields = HashMap::new();
    let mut text = None;
    let head = match rest.find("text=") {
        Some(at) if at == 0 || rest.as_bytes()[at - 1] == b' ' => {
            text = Some(rest[at + 5..].to_string());
            &rest[..at]
        }
        _ => rest,
    };
    for word in head.split_whitespace() {
        if let Some((k, v)) = word.split_once('=') {
            fields.insert(k.to_string(), v.to_string());
        }
    }
    for line in lines {
        if let Some((k, v)) = line.split_once('=') {
            if k == "text" {
                text = Some(v.to_string());
            } else {
                fields.insert(k.to_string(), v.to_string());
            }
        }
    }
    match command {
        "SAY" => match Priority::parse(fields.get("priority").map_or("normal", String::as_str)) {
            Some(priority) => Message::Say {
                id: fields.get("id").cloned().unwrap_or_default(),
                priority,
                lang: fields.get("lang").cloned().unwrap_or_else(|| "en-GB".into()),
                voice: fields.get("voice").cloned().unwrap_or_else(|| "default".into()),
                rate: number(&fields, "rate", 50),
                volume: number(&fields, "volume", 80),
                text: text.unwrap_or_default(),
            },
            None => Message::Unknown(first.to_string()),
        },
        "STOP" => Message::Stop,
        "PAUSE" => Message::Pause,
        "RESUME" => Message::Resume,
        "PING" => Message::Ping,
        "CUE" => match fields.get("path") {
            Some(path) => Message::Cue { path: path.clone(), volume: number(&fields, "volume", 60) },
            None => Message::Unknown(first.to_string()),
        },
        _ => Message::Unknown(first.to_string()),
    }
}

pub fn choose_voice(folder: &Path, voice: &str, lang: &str) -> Result<String, &'static str> {
    if !matches!(voice, "default" | "system" | "") {
        return if has_voice(folder, voice) { Ok(voice.to_string()) } else { Err("voice-not-ready") };
    }
    let english = lang == "en" || lang.starts_with("en-") || lang.starts_with("en_");
    if !english {
        return Err("language-not-ready");
    }
    let name = format!("supertonic-{DEFAULT_VOICE}");
    if has_voice(folder, &name) { Ok(name) } else { Err("voice-not-ready") }
}

pub struct Speaker {
    folder: std::path::PathBuf,
    reader: Reader,
    stores: Mutex<HashMap<String, Option<Arc<Store>>>>,
}

impl Speaker {
    pub fn new(folder: std::path::PathBuf) -> Speaker {
        Speaker::checked(folder, unchecked)
    }

    pub fn checked(folder: std::path::PathBuf, check: super::reader::Check) -> Speaker {
        Speaker { folder, reader: Reader::new(check), stores: Mutex::new(HashMap::new()) }
    }
}

impl Renderer for Speaker {
    fn render(
        &self,
        voice: &str,
        text: &str,
        speed: f32,
        cancel: &Arc<std::sync::atomic::AtomicBool>,
        each: &mut dyn FnMut(Vec<i16>) -> bool,
    ) -> Result<u32, String> {
        self.reader.render(&self.folder, voice, text, speed, Some(cancel.clone()), each)
    }

    fn status(&self) -> &'static str {
        self.reader.status(&self.folder)
    }

    fn warm(&self) {
        let _ = self.reader.warm(&self.folder);
    }

    fn phrases(&self, voice: &str) -> Option<Arc<Store>> {
        let mut stores = self.stores.lock().unwrap_or_else(|p| p.into_inner());
        stores.entry(voice.to_string()).or_insert_with(|| Store::open(&self.folder, voice).map(Arc::new)).clone()
    }
}

pub fn serve(stream: UnixStream, engine: Arc<Engine>, folder: &Path, cues: Arc<Mutex<Cues>>) {
    let _ = stream.set_nonblocking(false);
    let client = NEXT_CLIENT.fetch_add(1, Ordering::SeqCst);
    let Ok(mut out) = stream.try_clone() else { return };
    let (tx, rx) = channel::<String>();
    engine.connect(client, tx.clone());

    {
        let engine = engine.clone();
        std::thread::spawn(move || engine.warm());
    }
    let writer = std::thread::spawn(move || {
        for line in rx {
            if writeln!(out, "{line}").is_err() {
                break;
            }
        }
    });
    let _ = tx.send(format!("STATUS {}", engine.status()));
    let mut reader = BufReader::new(stream);
    let mut block = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        let n = (&mut reader).take(MAX_LINE as u64).read_line(&mut line).unwrap_or(0);
        if n == 0 || !line.ends_with('\n') && n >= MAX_LINE {
            break;
        }
        if !line.trim().is_empty() {
            block.push_str(&line);
            if block.len() < MAX_LINE {
                continue;
            }
        }
        dispatch(parse(&std::mem::take(&mut block)), client, &engine, folder, &cues, &tx);
    }
    engine.disconnect(client);
    drop(tx);
    let _ = writer.join();
}

fn dispatch(message: Message, client: u64, engine: &Engine, folder: &Path, cues: &Mutex<Cues>, tx: &Sender<String>) {
    match message {
        Message::Say { id, priority, lang, voice, rate, volume, text } => match choose_voice(folder, &voice, &lang) {
            Ok(voice) => engine.submit(Request { client, id, priority, voice, rate, volume, text: text.chars().take(MAX_TEXT_CHARS).collect() }),
            Err(reason) => {
                let _ = tx.send(format!("ERROR id={id} reason={reason}"));
            }
        },
        Message::Stop => engine.stop(client),
        Message::Pause => engine.pause(true),
        Message::Resume => engine.pause(false),
        Message::Cue { path, volume } => {
            if let Err(why) = cues.lock().unwrap_or_else(|p| p.into_inner()).play(&path, volume) {
                let _ = tx.send(format!("ERROR reason={why}"));
            }
        }
        Message::Ping => {
            let _ = tx.send(format!("PONG {}", engine.status()));
        }
        Message::Unknown(what) => {
            let _ = tx.send(format!("ERROR reason=unknown-message {what}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_say_message_is_read() {
        let m = parse("SAY id=17 priority=focus lang=en-GB voice=default rate=60 pitch=50 volume=80 source=focus\ntext=Save, button\n");
        assert_eq!(
            m,
            Message::Say { id: "17".into(), priority: Priority::Focus, lang: "en-GB".into(), voice: "default".into(), rate: 60, volume: 80, text: "Save, button".into() }
        );
    }

    #[test]
    fn text_on_the_first_line_runs_to_the_end_of_it() {
        let Message::Say { text, id, .. } = parse("SAY id=1 text=Equal = sign, text=here") else { panic!() };
        assert_eq!((id.as_str(), text.as_str()), ("1", "Equal = sign, text=here"));
    }

    #[test]
    fn short_messages_and_mistakes() {
        assert_eq!(parse("STOP"), Message::Stop);
        assert_eq!(parse("PING\n"), Message::Ping);
        assert_eq!(parse("CUE path=/x/y.ogg volume=40"), Message::Cue { path: "/x/y.ogg".into(), volume: 40 });
        assert!(matches!(parse("CUE name=dialog"), Message::Unknown(_)));
        assert!(matches!(parse("SAY id=1 priority=loud\ntext=x"), Message::Unknown(_)));
        assert!(matches!(parse("DANCE"), Message::Unknown(_)));
    }

    #[test]
    fn numbers_stay_in_range() {
        let Message::Say { rate, volume, .. } = parse("SAY id=1 rate=900 volume=abc\ntext=x") else { panic!() };
        assert_eq!((rate, volume), (100, 80));
    }

    #[test]
    fn the_voice_comes_from_the_language() {
        let dir = std::env::temp_dir().join(format!("door-voice-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(choose_voice(&dir, "default", "en-GB"), Err("voice-not-ready"));
        for n in ["duration_predictor", "text_encoder", "vector_estimator", "vocoder"] {
            std::fs::write(dir.join(format!("supertonic-{n}.onnx")), b"x").unwrap();
        }
        std::fs::write(dir.join("supertonic-unicode_indexer.json"), b"x").unwrap();
        std::fs::write(dir.join("supertonic-voice-M5.json"), b"x").unwrap();
        assert_eq!(choose_voice(&dir, "default", "en-US"), Ok("supertonic-M5".into()));
        assert_eq!(choose_voice(&dir, "system", "en"), Ok("supertonic-M5".into()));
        assert_eq!(choose_voice(&dir, "supertonic-M5", "fr-FR"), Ok("supertonic-M5".into()));
        assert_eq!(choose_voice(&dir, "default", "fr-FR"), Err("language-not-ready"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
