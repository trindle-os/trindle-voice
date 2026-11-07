// The trindle-voice command: turns text into a WAV file, serves the speech door, or builds a phrase store.
// SPDX-License-Identifier: GPL-3.0-or-later
use std::io::Write;
use std::path::{Path, PathBuf};
use trindle_voice::speech::reader::{Reader, unchecked};
use trindle_voice::speech::{phrases, server};
use trindle_voice::{Voice, level, wav};

const USAGE: &str = "usage: trindle-voice say --dir DIR --out FILE.wav [--voice M1] [--lang en] [--speed 1.05] [--seed N] TEXT
       trindle-voice serve --dir DIR
       trindle-voice render-phrases --dir DIR VOICE FILE-OF-LINES [OUT]";

#[derive(Debug, PartialEq)]
struct Say {
    dir: PathBuf,
    out: PathBuf,
    voice: String,
    lang: String,
    speed: f32,
    seed: Option<u64>,
    words: String,
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Say, String> {
    let mut args = args.into_iter();
    if args.next().as_deref() != Some("say") {
        return Err(USAGE.into());
    }
    let (mut dir, mut out, mut voice, mut lang, mut speed, mut seed, mut words) = (None, None, "M1".to_string(), "en".to_string(), 1.05f32, None, Vec::new());
    while let Some(a) = args.next() {
        let mut value = |flag: &str| args.next().ok_or_else(|| format!("{flag} needs a value"));
        match a.as_str() {
            "--dir" => dir = Some(PathBuf::from(value("--dir")?)),
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--voice" => voice = value("--voice")?,
            "--lang" => lang = value("--lang")?,
            "--speed" => speed = value("--speed")?.parse().map_err(|_| "--speed needs a number".to_string())?,
            "--seed" => seed = Some(value("--seed")?.parse().map_err(|_| "--seed needs a whole number".to_string())?),
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}. {USAGE}")),
            _ => words.push(a),
        }
    }
    if !trindle_voice::text::is_lang(&lang) {
        return Err(format!("--lang {lang} is not a language this voice speaks"));
    }
    Ok(Say { dir: dir.ok_or("--dir is required")?, out: out.ok_or("--out is required")?, voice, lang, speed, seed, words: words.join(" ") })
}

fn with_dir(args: impl IntoIterator<Item = String>) -> Result<(PathBuf, Vec<String>), String> {
    let (mut dir, mut rest) = (None, Vec::new());
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        if a == "--dir" {
            dir = Some(PathBuf::from(args.next().ok_or("--dir needs a value")?));
        } else {
            rest.push(a);
        }
    }
    Ok((dir.ok_or("--dir is required")?, rest))
}

fn serve(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let (dir, rest) = with_dir(args)?;
    if !rest.is_empty() {
        return Err(USAGE.into());
    }
    server::run(&dir, &server::door_path()?)
}

fn door_voice(voice: &str) -> String {
    if voice.starts_with("supertonic-") { voice.to_string() } else { format!("supertonic-{voice}") }
}

fn render_phrases(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let (dir, rest) = with_dir(args)?;
    let [voice, list, out @ ..] = rest.as_slice() else { return Err(USAGE.into()) };
    if out.len() > 1 {
        return Err(USAGE.into());
    }
    let voice = door_voice(voice);
    let out = out.first().map(PathBuf::from).unwrap_or_else(|| phrases::path(&dir, &voice));
    let lines = std::fs::read_to_string(list).map_err(|e| format!("{list}: {e}"))?;
    let bytes = phrases::render_all(&Reader::new(unchecked), &dir, &voice, &lines, &mut |i, n| eprintln!("{} of {n}", i + 1))?;
    std::fs::write(&out, bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("{}", out.display());
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let _ = std::fs::remove_file(path);
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1).peekable();
    match args.peek().map(String::as_str) {
        Some("serve") => return serve(args.skip(1)),
        Some("render-phrases") => return render_phrases(args.skip(1)),
        _ => {}
    }
    let say = parse(args)?;
    let engine = Voice::load(&say.dir)?;
    let style = Voice::style(&say.dir, &say.voice)?;
    let mut samples = engine.speak(&say.words, &say.lang, &style, say.speed, 0.3, say.seed, &|| false)?;
    level(&mut samples);
    let temp = say.out.with_extension("wav.part");
    write_new(&temp, &wav(&samples)?).map_err(|e| format!("{}: {e}", temp.display()))?;
    std::fs::rename(&temp, &say.out).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("{}: {e}", say.out.display())
    })
}

fn main() {
    if let Err(e) = run() {
        eprintln!("failed: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split(' ').map(String::from).collect()
    }

    #[test]
    fn reads_the_options_and_the_text() {
        let say = parse(args("say --dir d --out o.wav --voice F2 --speed 1.2 --seed 7 Hello there")).unwrap();
        assert_eq!((say.voice.as_str(), say.speed, say.seed, say.words.as_str()), ("F2", 1.2, Some(7), "Hello there"));
    }

    #[test]
    fn the_folder_is_taken_out_of_the_arguments() {
        let (dir, rest) = with_dir(args("M5 --dir d lines.txt")).unwrap();
        assert_eq!((dir, rest), (PathBuf::from("d"), args("M5 lines.txt")));
        assert!(with_dir(args("M5")).unwrap_err().contains("--dir is required"));
        assert!(serve(args("--dir d extra")).unwrap_err().starts_with("usage"));
        assert!(render_phrases(args("--dir d M5")).unwrap_err().starts_with("usage"));
    }

    #[test]
    fn a_voice_gets_the_door_prefix_once() {
        assert_eq!((door_voice("M5"), door_voice("supertonic-M5")), ("supertonic-M5".to_string(), "supertonic-M5".to_string()));
    }

    #[test]
    fn a_link_at_the_temp_name_is_replaced_not_followed() {
        let dir = std::env::temp_dir().join(format!("tv-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (target, temp) = (dir.join("other.txt"), dir.join("hello.wav.part"));
        std::fs::write(&target, "keep").unwrap();
        std::os::unix::fs::symlink(&target, &temp).unwrap();
        write_new(&temp, b"RIFF").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
        assert_eq!(std::fs::read(&temp).unwrap(), b"RIFF");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mistakes_are_named() {
        assert!(parse(args("say --dir")).unwrap_err().contains("--dir needs a value"));
        assert!(parse(args("say --dir d --out o --vioce M1 hi")).unwrap_err().contains("unknown option --vioce"));
        assert!(parse(args("say --dir d --out o --speed fast hi")).unwrap_err().contains("needs a number"));
        assert!(parse(args("say --out o hi")).unwrap_err().contains("--dir is required"));
        assert!(parse(args("hello")).unwrap_err().starts_with("usage"));
    }
}
