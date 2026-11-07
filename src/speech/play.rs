// Sound out through `pw-play`: one player kept running, fed in 20 ms pieces a few ahead of the clock, so cutting speech short stops it quickly.
// SPDX-License-Identifier: GPL-3.0-or-later
use std::io::Write;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const LEAD: Duration = Duration::from_millis(60);

pub trait Sink: Send {
    fn play(&mut self, samples: &[i16], rate: u32, volume: u8, cancel: &AtomicBool, paused: &AtomicBool) -> Result<bool, String>;
}

pub struct Player {
    child: Option<(Child, ChildStdin)>,

    heard_until: Instant,
    rate: u32,
}

impl Player {
    pub fn new() -> Player {
        Player { child: None, heard_until: Instant::now(), rate: 0 }
    }

    fn open(&mut self, rate: u32) -> Result<(), String> {
        if self.child.is_some() && self.rate == rate {
            return Ok(());
        }
        self.close();
        let mut child = Command::new("pw-play")
            .args(["--raw", "--rate", &rate.to_string(), "--channels", "1", "--format", "s16", "--latency", "20ms", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start pw-play: {e}"))?;
        let input = child.stdin.take().ok_or("pw-play took no input")?;
        self.child = Some((child, input));
        self.rate = rate;
        self.heard_until = Instant::now();
        Ok(())
    }

    pub fn finish(&self, stop: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) && Instant::now() < self.heard_until {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn close(&mut self) {
        if let Some((mut child, input)) = self.child.take() {
            drop(input);
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Default for Player {
    fn default() -> Player {
        Player::new()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.close();
    }
}

impl Sink for Player {
    fn play(&mut self, samples: &[i16], rate: u32, volume: u8, cancel: &AtomicBool, paused: &AtomicBool) -> Result<bool, String> {
        self.open(rate)?;
        let gain = f32::from(volume.min(100)) / 100.0;
        let piece = (rate / 50) as usize;
        for chunk in samples.chunks(piece) {
            loop {
                if cancel.load(Ordering::SeqCst) {
                    return Ok(false);
                }
                let now = Instant::now();
                if self.heard_until < now {
                    self.heard_until = now;
                }
                if !paused.load(Ordering::SeqCst) && self.heard_until.duration_since(now) < LEAD {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let bytes: Vec<u8> = chunk.iter().flat_map(|s| ((f32::from(*s) * gain) as i16).to_le_bytes()).collect();
            let written = self.child.as_mut().is_some_and(|(_, input)| input.write_all(&bytes).is_ok());
            if !written {
                self.close();
                return Err("the sound player stopped".into());
            }
            self.heard_until += Duration::from_secs_f64(chunk.len() as f64 / f64::from(rate));
        }
        Ok(true)
    }
}

#[derive(Default)]
pub struct Cues {
    playing: Vec<Child>,
}

impl Cues {
    pub fn play(&mut self, path: &str, volume: u8) -> Result<(), String> {
        self.playing.retain_mut(|c| c.try_wait().map(|s| s.is_none()).unwrap_or(false));
        if self.playing.len() >= 3 {
            return Err("too-many-cues".into());
        }
        if !Path::new(path).is_absolute() || !Path::new(path).is_file() {
            return Err("bad-cue-path".into());
        }
        let child = Command::new("pw-play")
            .args(["--volume", &format!("{:.2}", f32::from(volume.min(100)) / 100.0), "--", path])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start pw-play: {e}"))?;
        self.playing.push(child);
        Ok(())
    }
}
