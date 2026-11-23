use crate::audio::{Frame, SAMPLES};

const FFT_SIZE: usize = 512;
const BINS: usize = FFT_SIZE / 2 + 1;
const HISTORY: usize = 40;
const MIN_ENERGY: f32 = 1.0e-8;

#[derive(Clone, Copy, Default)]
struct Complex {
    real: f32,
    imaginary: f32,
}

struct Spectrum {
    power: [f32; BINS],
    peak: f32,
}

impl Spectrum {
    fn empty() -> Self {
        Self {
            power: [0.0; BINS],
            peak: 0.0,
        }
    }
}

struct History {
    frames: Vec<[Spectrum; 2]>,
    next: usize,
}

pub struct SpectralEcho {
    history: [History; 2],
    window: [f32; SAMPLES],
    twiddles: [Complex; FFT_SIZE / 2],
}

impl SpectralEcho {
    pub fn new() -> Self {
        Self {
            history: std::array::from_fn(|_| History {
                frames: (0..HISTORY)
                    .map(|_| [Spectrum::empty(), Spectrum::empty()])
                    .collect(),
                next: 0,
            }),
            window: std::array::from_fn(|i| {
                0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / SAMPLES as f32).cos()
            }),
            twiddles: std::array::from_fn(|i| {
                let (imaginary, real) =
                    (-std::f32::consts::TAU * i as f32 / FFT_SIZE as f32).sin_cos();
                Complex { real, imaginary }
            }),
        }
    }

    pub fn record(&mut self, lane: usize, frame: &Frame) {
        let spectrum = [self.spectrum(frame, 0), self.spectrum(frame, 1)];
        let history = &mut self.history[lane];
        history.frames[history.next] = spectrum;
        history.next = (history.next + 1) % HISTORY;
    }

    pub fn suppress(&self, frame: &mut Frame) {
        for channel in 0..2 {
            self.suppress_channel(frame, channel);
        }
    }

    fn suppress_channel(&self, frame: &mut Frame, channel: usize) {
        let templates = self
            .history
            .iter()
            .flat_map(|history| &history.frames)
            .flatten()
            .filter(|spectrum| spectrum.peak > MIN_ENERGY);
        let observed = self.spectrum(frame, channel);
        if observed.peak < MIN_ENERGY {
            return;
        }
        // Some virtual mixers change the phase of the returned audio. Compare
        // magnitude only as a fallback, with a high similarity requirement.
        let estimate = templates
            .filter_map(|reference| {
                // Score the bands where the reference is strong. Independent
                // sound in other bands must not prevent matching a returned copy.
                let (cross, captured, playback) = observed
                    .power
                    .iter()
                    .zip(&reference.power)
                    .filter(|(_, value)| **value >= reference.peak * 0.03)
                    .fold((0.0, 0.0, 0.0), |(cross, captured, playback), (a, b)| {
                        (cross + a * b, captured + a * a, playback + b * b)
                    });
                let similarity = cross / (captured * playback).sqrt();
                let scale = cross / playback;
                (captured >= MIN_ENERGY && similarity >= 0.94 && (0.002..=4.0).contains(&scale))
                    .then_some((similarity, scale, reference))
            })
            .max_by(|a, b| a.0.total_cmp(&b.0));
        let Some((_, scale, reference)) = estimate else {
            return;
        };
        let gains: [f32; BINS] = std::array::from_fn(|bin| {
            if reference.power[bin] <= reference.peak * 0.001 {
                return 1.0;
            }
            let ratio = scale * reference.power[bin] / observed.power[bin].max(MIN_ENERGY);
            (1.0 - (1.35 * ratio).clamp(0.0, 0.995)).sqrt()
        });
        let mut signal = [Complex::default(); FFT_SIZE];
        for (value, pair) in signal.iter_mut().zip(frame.chunks_exact(2)) {
            value.real = pair[channel];
        }
        self.transform(&mut signal, false);
        for (bin, value) in signal.iter_mut().enumerate() {
            let gain = gains[bin.min(FFT_SIZE - bin)];
            value.real *= gain;
            value.imaginary *= gain;
        }
        self.transform(&mut signal, true);
        for (pair, value) in frame.chunks_exact_mut(2).zip(signal) {
            pair[channel] = value.real / FFT_SIZE as f32;
        }
    }

    fn spectrum(&self, frame: &Frame, channel: usize) -> Spectrum {
        let energy = frame
            .iter()
            .skip(channel)
            .step_by(2)
            .map(|sample| sample * sample)
            .sum::<f32>();
        if energy < MIN_ENERGY || !energy.is_finite() {
            return Spectrum::empty();
        }
        let mut signal = [Complex::default(); FFT_SIZE];
        for (i, pair) in frame.chunks_exact(2).enumerate() {
            signal[i].real = pair[channel] * self.window[i];
        }
        self.transform(&mut signal, false);
        let power = std::array::from_fn(|bin| {
            let value = signal[bin];
            value.real * value.real + value.imaginary * value.imaginary
        });
        let peak = power.iter().copied().fold(0.0, f32::max);
        Spectrum { power, peak }
    }

    fn transform(&self, signal: &mut [Complex; FFT_SIZE], inverse: bool) {
        let shift = usize::BITS - FFT_SIZE.trailing_zeros();
        for i in 1..FFT_SIZE {
            let reversed = i.reverse_bits() >> shift;
            if i < reversed {
                signal.swap(i, reversed);
            }
        }
        let mut width = 2;
        while width <= FFT_SIZE {
            let half = width / 2;
            for block in signal.chunks_exact_mut(width) {
                let (left, right) = block.split_at_mut(half);
                for (i, (even, odd)) in left.iter_mut().zip(right).enumerate() {
                    let t = self.twiddles[i * FFT_SIZE / width];
                    let imaginary = if inverse { -t.imaginary } else { t.imaginary };
                    let real = odd.real * t.real - odd.imaginary * imaginary;
                    let imag = odd.real * imaginary + odd.imaginary * t.real;
                    *odd = Complex {
                        real: even.real - real,
                        imaginary: even.imaginary - imag,
                    };
                    *even = Complex {
                        real: even.real + real,
                        imaginary: even.imaginary + imag,
                    };
                }
            }
            width *= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attenuates_a_phase_shifted_copy_without_muting_an_unrelated_band() {
        let mut echo = SpectralEcho::new();
        let tone = |i: usize, shifts: [f32; 3]| {
            [4300.0_f32, 5300.0, 6100.0]
                .into_iter()
                .zip(shifts)
                .map(|(hz, phase)| {
                    (std::f32::consts::TAU * hz * (i / 2) as f32 / 48_000.0 + phase).sin()
                })
                .sum::<f32>()
        };
        let reference = std::array::from_fn(|i| tone(i, [0.0, 0.0, 0.0]) * 0.1);
        echo.record(0, &reference);
        let local =
            |i: usize| (std::f32::consts::TAU * 1200.0 * (i / 2) as f32 / 48_000.0).sin() * 0.01;
        let mut captured = std::array::from_fn(|i| tone(i, [-2.0, 0.8, 2.5]) * 0.03 + local(i));
        echo.suppress(&mut captured);
        let error = captured
            .iter()
            .enumerate()
            .map(|(i, sample)| (sample - local(i)).powi(2))
            .sum::<f32>();
        assert!((error / captured.len() as f32).sqrt() < 0.008);
    }
}
