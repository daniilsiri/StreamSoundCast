use crate::audio::{Frame, SAMPLES};
use crate::spectral_echo::SpectralEcho;

const HISTORY: usize = 32_768;
const MAX_DELAY: usize = SAMPLES * 40;
const PROBE_STEP: usize = 16;
const SEARCH_BAND: usize = SAMPLES * 4;
const TRACK_RADIUS: usize = 12;
const MIN_ENERGY: f32 = 1.0e-7;
const ACQUIRE_SCORE: f32 = 0.20;
const TRACK_SCORE: f32 = 0.50;
const MIN_EXPLAINED: f32 = 0.52;
const TAPS: usize = 7;

/// Rejects audio returned by external renderers, such as a virtual mixer that
/// forwards a WaveCast output to a second device under its own process ID.
pub struct SystemEcho {
    references: [Reference; 2],
    spectral: SpectralEcho,
}

impl SystemEcho {
    pub fn new() -> Self {
        Self {
            references: std::array::from_fn(|_| Reference::new()),
            spectral: SpectralEcho::new(),
        }
    }

    pub fn record(&mut self, lane: usize, frame: &Frame) {
        self.references[lane].record(frame);
        self.spectral.record(lane, frame);
    }

    pub fn cancel(&mut self, capture: &mut Frame) {
        for reference in &mut self.references {
            reference.cancel(capture);
        }
        self.spectral.suppress(capture);
    }
}

struct Reference {
    samples: Vec<[f32; 2]>,
    end: usize,
    len: usize,
    since_signal: usize,
    delay: Option<usize>,
}

impl Reference {
    fn new() -> Self {
        Self {
            samples: vec![[0.0; 2]; HISTORY],
            end: 0,
            len: 0,
            since_signal: HISTORY,
            delay: None,
        }
    }

    fn record(&mut self, frame: &Frame) {
        let mut peak = 0.0_f32;
        for (i, pair) in frame.chunks_exact(2).enumerate() {
            self.samples[(self.end + i) & (HISTORY - 1)] = [pair[0], pair[1]];
            peak = peak.max(pair[0].abs()).max(pair[1].abs());
        }
        self.end = (self.end + SAMPLES) & (HISTORY - 1);
        self.len = (self.len + SAMPLES).min(HISTORY);
        self.since_signal = if peak > 0.0001 {
            0
        } else {
            self.since_signal.saturating_add(SAMPLES)
        };
    }

    fn cancel(&mut self, capture: &mut Frame) {
        if self.len < SAMPLES || self.since_signal > MAX_DELAY + SAMPLES {
            self.delay = None;
            return;
        }
        let max_delay = MAX_DELAY.min(self.len - SAMPLES);
        let energy = capture.iter().map(|sample| sample * sample).sum::<f32>();
        if !energy.is_finite() || energy < MIN_ENERGY {
            return;
        }

        let tracked = self.delay.and_then(|last| {
            self.best::<1>(
                capture,
                energy,
                last.saturating_sub(TRACK_RADIUS)..=(last + TRACK_RADIUS).min(max_delay),
            )
            .filter(|(_, score)| *score >= TRACK_SCORE)
        });
        let candidate = tracked.or_else(|| {
            let probe_energy = capture
                .chunks_exact(2)
                .step_by(PROBE_STEP)
                .map(|pair| pair[0] * pair[0] + pair[1] * pair[1])
                .sum::<f32>();
            if probe_energy < MIN_ENERGY {
                return None;
            }
            // Pick candidates across the delay range and verify them at full
            // resolution. Decimating the lag itself misses some tone phases.
            (0..=max_delay)
                .step_by(SEARCH_BAND)
                .filter_map(|start| {
                    let end = (start + SEARCH_BAND - 1).min(max_delay);
                    let (lag, _) = self.best::<PROBE_STEP>(capture, probe_energy, start..=end)?;
                    self.best::<1>(
                        capture,
                        energy,
                        lag.saturating_sub(2)..=(lag + 2).min(max_delay),
                    )
                })
                .max_by(|left, right| left.1.total_cmp(&right.1))
                .filter(|(_, score)| *score >= ACQUIRE_SCORE)
        });
        let Some((delay, _)) = candidate else {
            return;
        };

        let start = self.start(delay);
        let mut matched = false;
        for channel in 0..2 {
            if let Some(filter) = self.fit(capture, start, channel) {
                matched = true;
                for (i, pair) in capture.chunks_exact_mut(2).enumerate() {
                    pair[channel] -= self.estimate(start + i, channel, &filter);
                }
            }
        }
        if matched {
            self.delay = Some(delay);
        }
    }

    fn fit(&self, capture: &Frame, start: usize, channel: usize) -> Option<[f32; TAPS]> {
        // A short filter allows for fractional delay and coloration from resampling
        // or processing in a virtual mixer. Regularization bounds the gain.
        let mut matrix = [[0.0_f32; TAPS]; TAPS];
        let mut rhs = [0.0_f32; TAPS];
        let mut energy = 0.0_f32;
        for (i, pair) in capture.chunks_exact(2).enumerate() {
            let sample = pair[channel];
            let reference = std::array::from_fn::<_, TAPS, _>(|tap| {
                self.samples[(start + i + HISTORY - TAPS / 2 + tap) & (HISTORY - 1)][channel]
            });
            energy += sample * sample;
            for row in 0..TAPS {
                rhs[row] += reference[row] * sample;
                for col in row..TAPS {
                    matrix[row][col] += reference[row] * reference[col];
                }
            }
        }
        let power = matrix[TAPS / 2][TAPS / 2];
        if power < MIN_ENERGY || energy < MIN_ENERGY {
            return None;
        }
        for i in 0..TAPS {
            for j in 0..i {
                matrix[i][j] = matrix[j][i];
            }
            matrix[i][i] += power * 0.003;
        }
        // The regularized covariance is positive definite, so elimination
        // can proceed without row pivoting.
        for i in 0..TAPS {
            for row in i + 1..TAPS {
                let factor = matrix[row][i] / matrix[i][i];
                for col in i + 1..TAPS {
                    matrix[row][col] -= factor * matrix[i][col];
                }
                rhs[row] -= factor * rhs[i];
            }
        }
        let mut filter = [0.0_f32; TAPS];
        for row in (0..TAPS).rev() {
            let rest = (row + 1..TAPS)
                .map(|col| matrix[row][col] * filter[col])
                .sum::<f32>();
            filter[row] = (rhs[row] - rest) / matrix[row][row];
        }
        if filter.iter().map(|gain| gain.abs()).sum::<f32>() > 4.0 {
            return None;
        }
        let residue = capture
            .chunks_exact(2)
            .enumerate()
            .map(|(i, pair)| {
                let error = pair[channel] - self.estimate(start + i, channel, &filter);
                error * error
            })
            .sum::<f32>();
        (residue <= energy * (1.0 - MIN_EXPLAINED)).then_some(filter)
    }

    fn estimate(&self, sample: usize, channel: usize, filter: &[f32; TAPS]) -> f32 {
        filter
            .iter()
            .enumerate()
            .map(|(tap, gain)| {
                gain * self.samples[(sample + HISTORY - TAPS / 2 + tap) & (HISTORY - 1)][channel]
            })
            .sum()
    }

    fn start(&self, delay: usize) -> usize {
        (self.end + HISTORY - SAMPLES - delay) & (HISTORY - 1)
    }

    fn best<const STEP: usize>(
        &self,
        capture: &Frame,
        energy: f32,
        delays: impl Iterator<Item = usize>,
    ) -> Option<(usize, f32)> {
        let mut best = None;
        for delay in delays {
            let start = self.start(delay);
            let mut dot_left = 0.0;
            let mut dot_right = 0.0;
            let mut power_left = 0.0;
            let mut power_right = 0.0;
            for i in (0..SAMPLES).step_by(STEP) {
                let [left, right] = self.samples[(start + i) & (HISTORY - 1)];
                dot_left += left * capture[i * 2];
                dot_right += right * capture[i * 2 + 1];
                power_left += left * left;
                power_right += right * right;
            }
            if power_left + power_right > MIN_ENERGY {
                let score = (dot_left * dot_left / power_left.max(MIN_ENERGY)
                    + dot_right * dot_right / power_right.max(MIN_ENERGY))
                    / energy;
                if best.is_none_or(|(_, previous)| score > previous) {
                    best = Some((delay, score));
                }
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_delayed_stereo_playback_while_preserving_local_audio() {
        let reference = |i: usize| {
            let t = i as f32 / 48_000.0;
            let sample =
                0.3 * (t * (1100.0 + 2500.0 * t)).sin() + 0.2 * (t * (500.0 + 1100.0 * t)).sin();
            [sample, sample * -0.5]
        };
        let mut canceller = SystemEcho::new();
        for frame in 0..30 {
            let samples = std::array::from_fn(|i| reference(frame * SAMPLES + i / 2)[i % 2]);
            canceller.record(0, &samples);
        }
        let local = |i: usize| 0.05 * (i as f32 * 0.15).sin();
        let mut captured =
            std::array::from_fn(|i| reference(29 * SAMPLES - 2937 + i / 2)[i % 2] * 0.4 + local(i));
        canceller.cancel(&mut captured);
        let error = captured
            .iter()
            .enumerate()
            .map(|(i, sample)| (sample - local(i)).powi(2))
            .sum::<f32>();
        assert!((error / captured.len() as f32).sqrt() < 0.008);

        let mut independent = std::array::from_fn(local);
        canceller.cancel(&mut independent);
        assert_eq!(independent, std::array::from_fn(local));
    }
}
