// Reads words aloud with the loaded Supertonic 2 model, a sentence at a time; without the voice files it says "voice-not-ready".
// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::{Style, Voice};

const PREFIX: &str = "supertonic-";

pub const DEFAULT_VOICE: &str = "M5";

pub const NORMAL_SPEED: f32 = 1.35;

pub type Check = fn(&Path, &str) -> Result<(), String>;

pub fn unchecked(_: &Path, _: &str) -> Result<(), String> {
    Ok(())
}

struct Loaded {
    folder: PathBuf,
    model: Voice,
    styles: HashMap<String, Style>,
}

pub struct Reader {
    slot: Mutex<Option<Loaded>>,
    is_loaded: AtomicBool,
    check: Check,
}

pub fn has_voice(folder: &Path, voice: &str) -> bool {
    voice.strip_prefix(PREFIX).is_some_and(|name| crate::files(folder) && folder.join(format!("supertonic-voice-{name}.json")).is_file())
}

impl Reader {
    pub const fn new(check: Check) -> Reader {
        Reader { slot: Mutex::new(None), is_loaded: AtomicBool::new(false), check }
    }

    pub fn unload(&self) {
        *self.slot.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.is_loaded.store(false, Ordering::SeqCst);
    }

    pub fn warm(&self, folder: &Path) -> Result<(), String> {
        let mut slot = self.slot.lock().unwrap_or_else(|p| p.into_inner());
        self.loaded(&mut slot, folder, DEFAULT_VOICE).map(|_| ())
    }

    fn loaded<'a>(&self, slot: &'a mut Option<Loaded>, folder: &Path, voice: &str) -> Result<&'a mut Loaded, String> {
        if slot.as_ref().is_none_or(|l| l.folder != folder) {
            if !crate::files(folder) {
                return Err("voice-not-ready".into());
            }
            (self.check)(folder, "supertonic")?;
            let model = Voice::load(folder)?;
            *slot = Some(Loaded { folder: folder.to_path_buf(), model, styles: Default::default() });
            self.is_loaded.store(true, Ordering::SeqCst);
        }
        let l = slot.as_mut().ok_or_else(|| "voice-not-ready".to_string())?;
        if !l.styles.contains_key(voice) {
            (self.check)(folder, &format!("{PREFIX}{}", voice.trim_start_matches(PREFIX)))?;
            l.styles.insert(voice.to_string(), Voice::style(folder, voice)?);
        }
        Ok(l)
    }

    pub fn status(&self, folder: &Path) -> &'static str {
        if self.is_loaded.load(Ordering::SeqCst) {
            "ready"
        } else if crate::files(folder) {
            "loading"
        } else {
            "voice-missing"
        }
    }

    pub fn render(
        &self,
        folder: &Path,
        voice: &str,
        words: &str,
        speed: f32,
        cancel: Option<Arc<AtomicBool>>,
        each: &mut dyn FnMut(Vec<i16>) -> bool,
    ) -> Result<u32, String> {
        let voice = voice.strip_prefix(PREFIX).unwrap_or(if voice.is_empty() { DEFAULT_VOICE } else { voice });
        let cancelled = || cancel.as_ref().is_some_and(|c| c.load(Ordering::SeqCst));
        for sentence in crate::text::lead_pieces(words) {
            let said = crate::spoken::words(&sentence, true);
            if said.trim().is_empty() {
                continue;
            }

            let mut wave = {
                let mut slot = self.slot.lock().unwrap_or_else(|p| p.into_inner());
                let l = self.loaded(&mut slot, folder, voice)?;
                let style = l.styles.get(voice).ok_or("voice-not-ready")?;
                l.model.speak(&said, "en", style, speed.clamp(0.25, 4.0), 0.0, None, &cancelled)?
            };
            crate::level(&mut wave);
            let mut samples: Vec<i16> = wave.iter().map(|s| (s * 32767.0) as i16).collect();
            samples.extend(vec![0i16; crate::RATE as usize / 10]);
            if !each(samples) {
                break;
            }
        }
        Ok(crate::RATE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speaking_says_it_is_not_ready_without_the_voice_files() {
        let dir = std::env::temp_dir().join(format!("reader-none-{}", std::process::id()));
        let reader = Reader::new(unchecked);
        assert_eq!(reader.render(&dir, "supertonic-M5", "Hi", NORMAL_SPEED, None, &mut |_| true).unwrap_err(), "voice-not-ready");
        assert_eq!(reader.status(&dir), "voice-missing");
        assert!(!has_voice(&dir, "supertonic-M5"));
    }
}
