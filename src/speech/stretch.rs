// Changes the speed of sound that is already made, keeping its pitch (overlap-add with a search for the best join).
// SPDX-License-Identifier: GPL-3.0-or-later
pub fn speed(samples: &[i16], rate: u32, factor: f32) -> Vec<i16> {
    if (factor - 1.0).abs() < 0.02 || samples.len() < 2000 {
        return samples.to_vec();
    }
    let frame = (rate as usize / 40).max(64) & !1;
    let hop = frame / 2;
    let analysis = hop as f32 * factor;
    let reach = frame / 4;
    let input: Vec<f32> = samples.iter().map(|&s| s as f32).collect();
    let window: Vec<f32> = (0..frame).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * (i as f32 + 0.5) / frame as f32).cos()).collect();
    let steps = (input.len() as f32 / analysis) as usize;
    let mut out = vec![0f32; steps * hop + frame];
    let mut follow = 0usize;
    for k in 0..steps {
        let wanted = (k as f32 * analysis) as usize;
        let start = if k == 0 {
            0
        } else {
            let (lo, hi) = (wanted.saturating_sub(reach), (wanted + reach).min(input.len().saturating_sub(frame)));
            let mut best = (f32::MIN, wanted.min(hi));
            let target = &input[follow..(follow + frame).min(input.len())];
            let mut c = lo;
            while c <= hi {
                let seg = &input[c..(c + frame).min(input.len())];
                let n = seg.len().min(target.len()).min(frame / 2);
                let score: f32 = (0..n).map(|i| seg[i] * target[i]).sum();
                if score > best.0 {
                    best = (score, c);
                }
                c += 4;
            }
            best.1
        };
        for i in 0..frame {
            if let Some(v) = input.get(start + i) {
                out[k * hop + i] += v * window[i];
            }
        }
        follow = (start + hop).min(input.len().saturating_sub(1));
    }
    out.truncate(steps * hop);
    out.iter().map(|v| v.clamp(-32768.0, 32767.0) as i16).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize) -> Vec<i16> {
        (0..n).map(|i| ((i as f32 * 0.05).sin() * 8000.0) as i16).collect()
    }

    #[test]
    fn faster_is_shorter_and_slower_is_longer() {
        let t = tone(24000);
        let fast = speed(&t, 24000, 1.5);
        let slow = speed(&t, 24000, 0.7);
        assert!((fast.len() as f32 / 16000.0 - 1.0).abs() < 0.1, "{}", fast.len());
        assert!((slow.len() as f32 / 34285.0 - 1.0).abs() < 0.1, "{}", slow.len());
    }

    #[test]
    fn normal_speed_is_left_alone() {
        let t = tone(5000);
        assert_eq!(speed(&t, 24000, 1.0), t);
    }

    #[test]
    fn a_tone_keeps_its_pitch() {
        let t = tone(24000);
        let fast = speed(&t, 24000, 1.5);
        let crossings = |s: &[i16]| s.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count() as f32 / s.len() as f32;
        assert!((crossings(&fast) / crossings(&t) - 1.0).abs() < 0.1);
    }
}
