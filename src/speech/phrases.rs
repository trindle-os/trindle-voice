// The phrase store: Trindle's own words spoken ahead of time and kept as sound, one `phrases-<voice>.bin` beside the voice files.
// SPDX-License-Identifier: GPL-3.0-or-later
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::reader::{NORMAL_SPEED, Reader};

const MAGIC: &[u8; 8] = b"TVPHRAS1";
const ENTRY: usize = 32 + 8 + 4;

pub struct Store {
    pub rate: u32,
    index: HashMap<[u8; 32], (usize, usize)>,
    data: Vec<i16>,
}

pub fn path(folder: &Path, voice: &str) -> PathBuf {
    folder.join(format!("phrases-{voice}.bin"))
}

fn key(text: &str) -> [u8; 32] {
    Sha256::digest(text.as_bytes()).into()
}

impl Store {
    pub fn parse(bytes: &[u8]) -> Option<Store> {
        if bytes.len() < 16 || &bytes[..8] != MAGIC {
            return None;
        }
        let rate = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
        let count = u32::from_le_bytes(bytes[12..16].try_into().ok()?) as usize;
        if !(8000..=96_000).contains(&rate) {
            return None;
        }
        let head = count.checked_mul(ENTRY)?.checked_add(16)?;
        if bytes.len() < head {
            return None;
        }
        let data: Vec<i16> = bytes[head..].as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect();
        let mut index = HashMap::new();
        for i in 0..count {
            let e = &bytes[16 + i * ENTRY..16 + (i + 1) * ENTRY];
            let start = u64::from_le_bytes(e[32..40].try_into().ok()?) as usize;
            let len = u32::from_le_bytes(e[40..44].try_into().ok()?) as usize;
            if start.checked_add(len)? > data.len() {
                return None;
            }
            index.insert(e[..32].try_into().ok()?, (start, len));
        }
        Some(Store { rate, index, data })
    }

    pub fn open(folder: &Path, voice: &str) -> Option<Store> {
        Store::parse(&std::fs::read(path(folder, voice)).ok()?)
    }

    pub fn get(&self, text: &str) -> Option<&[i16]> {
        self.index.get(&key(text)).map(|&(s, l)| &self.data[s..s + l])
    }
}

pub fn build(rate: u32, phrases: &[(String, Vec<i16>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(phrases.len() as u32).to_le_bytes());
    let mut start = 0u64;
    for (text, samples) in phrases {
        out.extend_from_slice(&key(text));
        out.extend_from_slice(&start.to_le_bytes());
        out.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        start += samples.len() as u64;
    }
    for (_, samples) in phrases {
        out.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
    }
    out
}

pub fn render_all(reader: &Reader, folder: &Path, voice: &str, lines: &str, progress: &mut dyn FnMut(usize, usize)) -> Result<Vec<u8>, String> {
    let texts: Vec<&str> = {
        let mut seen = std::collections::HashSet::new();
        lines.lines().map(str::trim).filter(|l| !l.is_empty() && seen.insert(*l)).collect()
    };
    let mut done = Vec::new();
    let mut rate = 24000;
    for (i, text) in texts.iter().enumerate() {
        progress(i, texts.len());
        let mut all = Vec::new();
        rate = reader.render(folder, voice, text, NORMAL_SPEED, None, &mut |s| {
            all.extend(s);
            true
        })?;
        done.push((text.to_string(), all));
    }
    Ok(build(rate, &done))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_built_store_is_read_back_by_exact_text() {
        let bytes = build(24000, &[("Save, button".into(), vec![1, 2, 3]), ("Close".into(), vec![9; 5])]);
        let s = Store::parse(&bytes).unwrap();
        assert_eq!(s.rate, 24000);
        assert_eq!(s.get("Save, button"), Some(&[1i16, 2, 3][..]));
        assert_eq!(s.get("Close").map(<[i16]>::len), Some(5));
        assert!(s.get("close").is_none());
    }

    #[test]
    fn a_damaged_store_is_refused() {
        let mut bytes = build(24000, &[("a".into(), vec![1, 2, 3])]);
        assert!(Store::parse(&bytes[..20]).is_none());
        bytes[0] = b'X';
        assert!(Store::parse(&bytes).is_none());
    }
}
