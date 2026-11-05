#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Equalizer {
    pub low_db: f32,
    pub mid_db: f32,
    pub high_db: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Compressor {
    pub threshold_db: f32,
    pub ratio: f32,
}

impl Default for Compressor {
    fn default() -> Self {
        Self {
            threshold_db: -18.0,
            ratio: 3.0,
        }
    }
}

pub fn apply_equalizer(
    samples: &mut [f32],
    sample_rate: u32,
    channels: usize,
    selection: [f32; 2],
    eq: Equalizer,
) -> Result<(), &'static str> {
    let (start, end) = selected_frames(samples, channels, selection)?;
    if sample_rate == 0 {
        return Err("Audio sample rate is invalid");
    }
    let low_gain = db_to_linear(eq.low_db.clamp(-24.0, 24.0));
    let mid_gain = db_to_linear(eq.mid_db.clamp(-24.0, 24.0));
    let high_gain = db_to_linear(eq.high_db.clamp(-24.0, 24.0));
    let low_alpha = one_pole_alpha(250.0, sample_rate);
    let high_alpha = one_pole_alpha(4_000.0_f32.min(sample_rate as f32 * 0.45), sample_rate);
    let mut low_state = vec![0.0; channels];
    let mut high_state = vec![0.0; channels];

    for frame_index in start..end {
        let frame = &mut samples[frame_index * channels..(frame_index + 1) * channels];
        for (channel, sample) in frame.iter_mut().enumerate() {
            let input = finite_sample(*sample);
            low_state[channel] += low_alpha * (input - low_state[channel]);
            high_state[channel] += high_alpha * (input - high_state[channel]);
            let low = low_state[channel];
            let mid = high_state[channel] - low;
            let high = input - high_state[channel];
            *sample = limit_sample(low * low_gain + mid * mid_gain + high * high_gain);
        }
    }
    Ok(())
}

pub fn apply_compressor(
    samples: &mut [f32],
    channels: usize,
    selection: [f32; 2],
    compressor: Compressor,
) -> Result<(), &'static str> {
    let (start, end) = selected_frames(samples, channels, selection)?;
    if !compressor.threshold_db.is_finite()
        || !compressor.ratio.is_finite()
        || compressor.ratio < 1.0
    {
        return Err("Compressor threshold or ratio is invalid");
    }

    let threshold = db_to_linear(compressor.threshold_db.clamp(-60.0, 0.0));
    let ratio = compressor.ratio.clamp(1.0, 20.0);
    let mut envelope = 0.0_f32;
    let attack = 0.35;
    let release = 0.01;
    for frame in samples[start * channels..end * channels].chunks_exact_mut(channels) {
        let peak = frame
            .iter()
            .map(|&sample| finite_sample(sample).abs())
            .fold(0.0_f32, f32::max);
        let coefficient = if peak > envelope { attack } else { release };
        envelope += coefficient * (peak - envelope);

        let gain = if envelope > threshold && envelope > 0.0 {
            let compressed_db = threshold.log10() * 20.0
                + (20.0 * envelope.log10() - threshold.log10() * 20.0) / ratio;
            db_to_linear(compressed_db - 20.0 * envelope.log10())
        } else {
            1.0
        };
        for sample in frame {
            *sample = limit_sample(finite_sample(*sample) * gain);
        }
    }
    Ok(())
}

fn selected_frames(
    samples: &[f32],
    channels: usize,
    selection: [f32; 2],
) -> Result<(usize, usize), &'static str> {
    if channels == 0 || samples.is_empty() || samples.len() % channels != 0 {
        return Err("Audio has no valid samples to process");
    }
    let frames = samples.len() / channels;
    let start = selection[0].min(selection[1]).clamp(0.0, 1.0);
    let end = selection[0].max(selection[1]).clamp(0.0, 1.0);
    let first = (start * frames as f32).floor() as usize;
    let last = ((end * frames as f32).ceil() as usize).min(frames);
    if last <= first {
        return Err("Select a non-empty audio region to process");
    }
    Ok((first, last))
}

fn one_pole_alpha(cutoff_hz: f32, sample_rate: u32) -> f32 {
    let cutoff = cutoff_hz.clamp(1.0, sample_rate as f32 * 0.49);
    1.0 - (-std::f32::consts::TAU * cutoff / sample_rate as f32).exp()
}

fn finite_sample(sample: f32) -> f32 {
    if sample.is_finite() { sample } else { 0.0 }
}

fn limit_sample(sample: f32) -> f32 {
    finite_sample(sample).clamp(-1.0, 1.0)
}

fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equalizer_processes_only_selected_frames_and_keeps_stereo_channels() {
        let mut samples = [0.25, -0.5, 0.25, -0.5, 0.25, -0.5, 0.25, -0.5];
        let original = samples;
        apply_equalizer(
            &mut samples,
            48_000,
            2,
            [0.25, 0.75],
            Equalizer {
                low_db: 6.0,
                mid_db: 0.0,
                high_db: 0.0,
            },
        )
        .unwrap();

        assert_eq!(&samples[..2], &original[..2]);
        assert_eq!(&samples[6..], &original[6..]);
        assert_ne!(&samples[2..6], &original[2..6]);
        assert!(
            samples
                .iter()
                .all(|sample| sample.is_finite() && sample.abs() <= 1.0)
        );
    }

    #[test]
    fn compressor_reduces_loud_signal_only_in_selection() {
        let mut samples = [0.1, 0.1, 0.9, -0.9, 0.8, -0.8, 0.1, 0.1];
        let original = samples;
        apply_compressor(
            &mut samples,
            2,
            [0.25, 0.75],
            Compressor {
                threshold_db: -12.0,
                ratio: 4.0,
            },
        )
        .unwrap();

        assert_eq!(&samples[..2], &original[..2]);
        assert_eq!(&samples[6..], &original[6..]);
        assert!(samples[2].abs() < original[2].abs());
        assert!((samples[2] + samples[3]).abs() < 1e-6);
    }
}
