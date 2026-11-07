// Supertonic 2 text to speech on tract: load the voice files, turn text into samples; `speech` is the server around it.
// SPDX-License-Identifier: GPL-3.0-or-later
pub mod speech;
pub mod spoken;
pub mod text;

use rand::SeedableRng;
use rand::rngs::StdRng;
use rand_distr::{Distribution, Normal};
use serde::Deserialize;
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use tract_onnx::prelude::*;

pub const RATE: u32 = 44100;
const BASE_CHUNK: usize = 512;
const COMPRESS: usize = 6;
const LATENT_DIM: usize = 24;

pub const STEPS: usize = 4;

pub const SPEED_RANGE: std::ops::RangeInclusive<f32> = 0.25..=4.0;
const MAX_PIECE_SECONDS: f32 = 120.0;

#[derive(Debug, PartialEq)]
pub enum Error {
    NotReady,
    Cancelled,
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::NotReady => f.write_str("the voice files are not all there"),
            Error::Cancelled => f.write_str("cancelled"),
            Error::Other(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for Error {}

impl From<Error> for String {
    fn from(e: Error) -> String {
        e.to_string()
    }
}

fn err<E: fmt::Display>(e: E) -> Error {
    Error::Other(e.to_string())
}

fn other<S: Into<String>>(e: S) -> Error {
    Error::Other(e.into())
}

pub struct Voice {
    dp: InferenceModel,
    text: InferenceModel,
    vector: InferenceModel,
    vocoder: InferenceModel,
    indexer: Vec<i64>,
}

pub struct Style {
    ttl: Vec<f32>,
    dp: Vec<f32>,
}

#[derive(Deserialize)]
struct Part {
    data: Vec<Vec<Vec<f32>>>,
}

#[derive(Deserialize)]
struct StyleFile {
    style_ttl: Part,
    style_dp: Part,
}

const MODELS: [&str; 4] = ["duration_predictor", "text_encoder", "vector_estimator", "vocoder"];

pub fn files(folder: &Path) -> bool {
    MODELS.iter().all(|n| folder.join(format!("supertonic-{n}.onnx")).is_file()) && folder.join("supertonic-unicode_indexer.json").is_file()
}

fn load_model(path: &Path) -> Result<InferenceModel, Error> {
    tract_onnx::onnx().with_ignore_value_info(true).model_for_path(path).map_err(err)
}

impl Voice {
    pub fn load(folder: &Path) -> Result<Self, Error> {
        if !files(folder) {
            return Err(Error::NotReady);
        }
        let model = |n: &str| load_model(&folder.join(format!("supertonic-{n}.onnx")));
        let indexer = std::fs::read(folder.join("supertonic-unicode_indexer.json")).map_err(err)?;
        Ok(Voice {
            dp: model(MODELS[0])?,
            text: model(MODELS[1])?,
            vector: model(MODELS[2])?,
            vocoder: model(MODELS[3])?,
            indexer: serde_json::from_slice(&indexer).map_err(err)?,
        })
    }

    pub fn style(folder: &Path, name: &str) -> Result<Style, Error> {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(other(format!("bad voice name: {name}")));
        }
        let bytes = std::fs::read(folder.join(format!("supertonic-voice-{name}.json"))).map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { Error::NotReady } else { err(e) })?;
        let f: StyleFile = serde_json::from_slice(&bytes).map_err(err)?;
        let flat = |p: Part| p.data.into_iter().flatten().flatten().collect();
        Ok(Style { ttl: flat(f.style_ttl), dp: flat(f.style_dp) })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn speak(&self, words: &str, lang: &str, style: &Style, speed: f32, pause: f32, seed: Option<u64>, cancelled: &dyn Fn() -> bool) -> Result<Vec<f32>, Error> {
        if !SPEED_RANGE.contains(&speed) {
            return Err(other(format!("speed must be between {} and {}", SPEED_RANGE.start(), SPEED_RANGE.end())));
        }
        let pieces = text::chunk(words, text::default_max(lang));
        if pieces.is_empty() {
            return Err(other("nothing to say"));
        }
        let mut out: Vec<f32> = Vec::new();
        for (i, piece) in pieces.iter().enumerate() {
            if i > 0 {
                out.resize(out.len() + (pause.max(0.0) * RATE as f32) as usize, 0.0);
            }
            out.extend(self.piece(piece, lang, style, speed, seed.map(|s| s.wrapping_add(i as u64)), cancelled)?);
        }
        Ok(out)
    }

    fn piece(&self, words: &str, lang: &str, style: &Style, speed: f32, seed: Option<u64>, cancelled: &dyn Fn() -> bool) -> Result<Vec<f32>, Error> {
        let prepared = text::prepare(words, lang).map_err(other)?;
        let ids: Vec<i64> = prepared.chars().filter_map(|c| self.indexer.get(c as usize).copied()).filter(|id| *id >= 0).collect();
        let n = ids.len();
        let text_ids = Tensor::from_shape(&[1, n], &ids).map_err(err)?;
        let text_mask = Tensor::from_shape(&[1, 1, n], &vec![1f32; n]).map_err(err)?;
        let style_dp = Tensor::from_shape(&[1, 8, 16], &style.dp).map_err(err)?;
        let style_ttl = Tensor::from_shape(&[1, 50, 256], &style.ttl).map_err(err)?;

        let duration = run(&self.dp, &[("text_ids", &text_ids), ("style_dp", &style_dp), ("text_mask", &text_mask)])?;
        let seconds = floats(&duration)?.first().copied().ok_or_else(|| other("no duration"))? / speed;
        if !(0.0..=MAX_PIECE_SECONDS).contains(&seconds) {
            return Err(other("the model gave a length that makes no sense"));
        }
        let samples = (seconds * RATE as f32) as usize;
        let text_emb = run(&self.text, &[("text_ids", &text_ids), ("style_ttl", &style_ttl), ("text_mask", &text_mask)])?;

        let chunk = BASE_CHUNK * COMPRESS;
        let frames = samples.div_ceil(chunk).max(1);
        let channels = LATENT_DIM * COMPRESS;
        let normal = Normal::new(0.0f32, 1.0).map_err(err)?;
        let mut rng = match seed {
            Some(s) => StdRng::seed_from_u64(s),
            None => StdRng::from_rng(&mut rand::rng()),
        };
        let mut latent: Vec<f32> = (0..channels * frames).map(|_| normal.sample(&mut rng)).collect();
        let latent_mask = Tensor::from_shape(&[1, 1, frames], &vec![1f32; frames]).map_err(err)?;

        let total = Tensor::from_shape(&[1], &[STEPS as f32]).map_err(err)?;
        let mut vector: Option<Plan> = None;
        for step in 0..STEPS {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            let current = Tensor::from_shape(&[1], &[step as f32]).map_err(err)?;
            let noisy = Tensor::from_shape(&[1, channels, frames], &latent).map_err(err)?;
            let inputs = [("noisy_latent", &noisy), ("text_emb", &text_emb), ("style_ttl", &style_ttl), ("latent_mask", &latent_mask), ("text_mask", &text_mask), ("current_step", &current), ("total_step", &total)];
            let plan = match &vector {
                Some(p) => p,
                None => vector.insert(plan(&self.vector, &inputs)?),
            };
            latent = floats(&run_plan(plan, &inputs)?)?;
        }
        let latent = Tensor::from_shape(&[1, channels, frames], &latent).map_err(err)?;
        let wave = floats(&run(&self.vocoder, &[("latent", &latent)])?)?;
        Ok(wave[..samples.min(wave.len())].to_vec())
    }
}

pub fn level(wave: &mut [f32]) {
    let (sum, count) = wave.iter().filter(|s| **s != 0.0).fold((0.0f32, 0usize), |(a, n), s| (a + s * s, n + 1));
    let rms = (sum / count.max(1) as f32).sqrt();
    let gain = if rms > 1e-4 { (0.12 / rms).min(8.0) } else { 1.0 };
    for s in wave {
        let x = *s * gain;
        let a = x.abs();
        *s = if a > 0.7 { x.signum() * (0.7 + 0.3 * ((a - 0.7) / 0.3).tanh()) } else { x };
    }
}

pub fn wav(samples: &[f32]) -> Result<Vec<u8>, Error> {
    let data = u32::try_from(samples.len() * 2).ok().filter(|d| *d <= u32::MAX - 36).ok_or_else(|| other("too much audio for one WAV file"))?;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    out.extend(samples.iter().flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()));
    Ok(out)
}

struct Plan {
    names: Vec<String>,
    model: Arc<TypedSimplePlan>,
}

fn floats(t: &Tensor) -> Result<Vec<f32>, Error> {
    Ok(t.try_as_plain_ram().map_err(err)?.as_slice::<f32>().map_err(err)?.to_vec())
}

fn plan(model: &InferenceModel, inputs: &[(&str, &Tensor)]) -> Result<Plan, Error> {
    let mut m = model.clone();
    let names: Vec<String> = m.input_outlets().map_err(err)?.iter().map(|o| m.node(o.node).name.clone()).collect();
    for (i, name) in names.iter().enumerate() {
        let t = inputs.iter().find(|s| s.0 == name).ok_or_else(|| other(format!("no value for {name}")))?.1;
        m.set_input_fact(i, InferenceFact::dt_shape(t.datum_type(), t.shape())).map_err(err)?;
    }
    Ok(Plan { names, model: m.into_typed().map_err(err)?.into_runnable().map_err(err)? })
}

fn run_plan(plan: &Plan, inputs: &[(&str, &Tensor)]) -> Result<Tensor, Error> {
    let given: TVec<TValue> = plan
        .names
        .iter()
        .map(|n| inputs.iter().find(|i| i.0 == n).map(|i| i.1.clone().into()).ok_or_else(|| other(format!("no value for {n}"))))
        .collect::<Result<_, _>>()?;
    let out = plan.model.run(given).map_err(err)?;
    Ok(out.first().ok_or_else(|| other("no output"))?.clone().into_tensor())
}

fn run(model: &InferenceModel, inputs: &[(&str, &Tensor)]) -> Result<Tensor, Error> {
    run_plan(&plan(model, inputs)?, inputs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_files_are_not_ready() {
        let dir = std::env::temp_dir().join(format!("tv-none-{}", std::process::id()));
        assert!(!files(&dir));
        assert_eq!(Voice::load(&dir).err(), Some(Error::NotReady));
        assert_eq!(Error::NotReady.to_string(), "the voice files are not all there");
        assert_eq!(String::from(Error::Cancelled), "cancelled");
    }

    #[test]
    fn voice_names_are_checked() {
        let dir = std::env::temp_dir();
        assert!(matches!(Voice::style(&dir, "../x"), Err(Error::Other(_))));
        assert!(matches!(Voice::style(&dir, ""), Err(Error::Other(_))));
        assert_eq!(Voice::style(&dir, "ZZ9").err(), Some(Error::NotReady));
    }

    #[test]
    fn level_never_clips_and_ignores_pauses() {
        let mut w = vec![0.9, -0.9, 0.01, 0.0];
        level(&mut w);
        assert!(w.iter().all(|s| s.abs() <= 1.0));
        let mut a = vec![0.05, -0.05, 0.05, -0.05];
        let mut b = vec![0.05, -0.05, 0.0, 0.0, 0.0, 0.0, 0.05, -0.05];
        level(&mut a);
        level(&mut b);
        assert!((a[0] - b[0]).abs() < 1e-6);
    }

    #[test]
    fn wav_header_is_right() {
        let b = wav(&[0.0, 0.5, -0.5]).unwrap();
        assert_eq!(&b[..4], b"RIFF");
        assert_eq!(b.len(), 44 + 6);
        assert_eq!(u32::from_le_bytes(b[24..28].try_into().unwrap()), RATE);
    }

    #[test]
    fn speed_range_is_sane() {
        for bad in [0.0, 0.01, f32::INFINITY, f32::NAN, 100.0] {
            assert!(!SPEED_RANGE.contains(&bad), "{bad}");
        }
        assert!(SPEED_RANGE.contains(&1.05));
    }
}
