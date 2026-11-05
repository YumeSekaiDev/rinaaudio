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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReverbParams {
    pub room_size: f32,
    pub decay_seconds: f32,
    pub pre_delay_ms: f32,
    pub damping: f32,
    pub diffusion: f32,
    pub early_reflections: f32,
    pub wet: f32,
    pub dry: f32,
    pub stereo_width: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ReverbPreset {
    #[default]
    Room,
    Hall,
    Plate,
    Cathedral,
    Vocal,
}

impl ReverbPreset {
    pub const ALL: [Self; 5] = [
        Self::Room,
        Self::Hall,
        Self::Plate,
        Self::Cathedral,
        Self::Vocal,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Room => "Room",
            Self::Hall => "Hall",
            Self::Plate => "Plate",
            Self::Cathedral => "Cathedral",
            Self::Vocal => "Vocal",
        }
    }

    pub fn params(self) -> ReverbParams {
        match self {
            Self::Room => ReverbParams {
                room_size: 0.35,
                decay_seconds: 0.75,
                pre_delay_ms: 8.0,
                damping: 0.55,
                diffusion: 0.65,
                early_reflections: 0.3,
                wet: 0.25,
                dry: 1.0,
                stereo_width: 1.0,
            },
            Self::Hall => ReverbParams {
                room_size: 0.72,
                decay_seconds: 2.4,
                pre_delay_ms: 24.0,
                damping: 0.35,
                diffusion: 0.75,
                early_reflections: 0.22,
                wet: 0.3,
                dry: 1.0,
                stereo_width: 1.0,
            },
            Self::Plate => ReverbParams {
                room_size: 0.58,
                decay_seconds: 1.8,
                pre_delay_ms: 12.0,
                damping: 0.18,
                diffusion: 0.9,
                early_reflections: 0.18,
                wet: 0.32,
                dry: 1.0,
                stereo_width: 0.9,
            },
            Self::Cathedral => ReverbParams {
                room_size: 1.0,
                decay_seconds: 6.0,
                pre_delay_ms: 38.0,
                damping: 0.25,
                diffusion: 0.8,
                early_reflections: 0.2,
                wet: 0.35,
                dry: 1.0,
                stereo_width: 1.0,
            },
            Self::Vocal => ReverbParams {
                room_size: 0.42,
                decay_seconds: 1.2,
                pre_delay_ms: 18.0,
                damping: 0.62,
                diffusion: 0.55,
                early_reflections: 0.35,
                wet: 0.2,
                dry: 1.0,
                stereo_width: 0.8,
            },
        }
    }
}

impl Default for ReverbParams {
    fn default() -> Self {
        ReverbPreset::Room.params()
    }
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

pub fn apply_fade_in(
    samples: &mut [f32],
    channels: usize,
    selection: [f32; 2],
) -> Result<(), &'static str> {
    let (start, end) = selected_frames(samples, channels, selection)?;
    let frames = end - start;
    for (offset, frame) in samples[start * channels..end * channels]
        .chunks_exact_mut(channels)
        .enumerate()
    {
        let gain = if frames == 1 {
            0.0
        } else {
            offset as f32 / (frames - 1) as f32
        };
        for sample in frame {
            *sample = limit_sample(finite_sample(*sample) * gain);
        }
    }
    Ok(())
}

pub fn apply_fade_out(
    samples: &mut [f32],
    channels: usize,
    selection: [f32; 2],
) -> Result<(), &'static str> {
    let (start, end) = selected_frames(samples, channels, selection)?;
    let frames = end - start;
    for (offset, frame) in samples[start * channels..end * channels]
        .chunks_exact_mut(channels)
        .enumerate()
    {
        let gain = if frames == 1 {
            0.0
        } else {
            (frames - offset - 1) as f32 / (frames - 1) as f32
        };
        for sample in frame {
            *sample = limit_sample(finite_sample(*sample) * gain);
        }
    }
    Ok(())
}

pub fn apply_denoise(
    samples: &mut [f32],
    channels: usize,
    selection: [f32; 2],
    threshold_db: f32,
    reduction_db: f32,
) -> Result<(), &'static str> {
    let (start, end) = selected_frames(samples, channels, selection)?;
    if !threshold_db.is_finite() || !reduction_db.is_finite() {
        return Err("Denoise settings must be finite");
    }
    let threshold = db_to_linear(threshold_db.clamp(-90.0, -6.0));
    let floor = threshold * 0.5;
    let reduction = db_to_linear(-reduction_db.clamp(0.0, 60.0));
    for frame in samples[start * channels..end * channels].chunks_exact_mut(channels) {
        let peak = frame
            .iter()
            .map(|&sample| finite_sample(sample).abs())
            .fold(0.0_f32, f32::max);
        let gain = if peak <= floor {
            reduction
        } else if peak < threshold {
            let blend = (peak - floor) / (threshold - floor);
            reduction + (1.0 - reduction) * blend
        } else {
            1.0
        };
        for sample in frame {
            *sample = limit_sample(finite_sample(*sample) * gain);
        }
    }
    Ok(())
}

pub fn apply_gate(
    samples: &mut [f32],
    channels: usize,
    selection: [f32; 2],
    threshold_db: f32,
) -> Result<(), &'static str> {
    let (start, end) = selected_frames(samples, channels, selection)?;
    if !threshold_db.is_finite() {
        return Err("Gate threshold must be finite");
    }
    let threshold = db_to_linear(threshold_db.clamp(-90.0, -6.0));
    let mut envelope = 0.0_f32;
    for frame in samples[start * channels..end * channels].chunks_exact_mut(channels) {
        let peak = frame
            .iter()
            .map(|&sample| finite_sample(sample).abs())
            .fold(0.0_f32, f32::max);
        let coefficient = if peak > envelope { 0.35 } else { 0.04 };
        envelope += coefficient * (peak - envelope);
        let gain = if envelope >= threshold { 1.0 } else { 0.0 };
        for sample in frame {
            *sample = finite_sample(*sample) * gain;
        }
    }
    Ok(())
}

pub fn apply_limiter(
    samples: &mut [f32],
    channels: usize,
    selection: [f32; 2],
    ceiling_db: f32,
) -> Result<(), &'static str> {
    let (start, end) = selected_frames(samples, channels, selection)?;
    if !ceiling_db.is_finite() {
        return Err("Limiter ceiling must be finite");
    }
    let ceiling = db_to_linear(ceiling_db.clamp(-24.0, 0.0));
    let mut envelope = 0.0_f32;
    for frame in samples[start * channels..end * channels].chunks_exact_mut(channels) {
        let peak = frame
            .iter()
            .map(|&sample| finite_sample(sample).abs())
            .fold(0.0_f32, f32::max);
        let coefficient = if peak > envelope { 0.8 } else { 0.02 };
        envelope += coefficient * (peak - envelope);
        let gain = if envelope > ceiling {
            ceiling / envelope
        } else {
            1.0
        };
        for sample in frame {
            *sample = (finite_sample(*sample) * gain).clamp(-ceiling, ceiling);
        }
    }
    Ok(())
}

pub fn apply_reverb(
    samples: &mut [f32],
    sample_rate: u32,
    channels: usize,
    selection: [f32; 2],
    params: ReverbParams,
) -> Result<(), &'static str> {
    let (start, end) = selected_frames(samples, channels, selection)?;
    if sample_rate == 0 || channels == 0 {
        return Err("Audio sample rate is invalid");
    }
    if !params.room_size.is_finite()
        || !params.decay_seconds.is_finite()
        || !params.pre_delay_ms.is_finite()
        || !params.damping.is_finite()
        || !params.diffusion.is_finite()
        || !params.early_reflections.is_finite()
        || !params.wet.is_finite()
        || !params.dry.is_finite()
        || !params.stereo_width.is_finite()
    {
        return Err("Reverb settings must be finite");
    }
    let room_size = params.room_size.clamp(0.1, 1.0);
    let decay = params.decay_seconds.clamp(0.2, 8.0);
    let pre_delay =
        (params.pre_delay_ms.clamp(0.0, 100.0) * sample_rate as f32 / 1000.0).round() as usize;
    let damping = params.damping.clamp(0.0, 1.0);
    let diffusion = params.diffusion.clamp(0.0, 1.0);
    let early_reflections = params.early_reflections.clamp(0.0, 1.0);
    let wet = params.wet.clamp(0.0, 1.0);
    let dry = params.dry.clamp(0.0, 1.0);
    let stereo_width = params.stereo_width.clamp(0.0, 1.0);
    let frame_count = end - start;
    let mut output = vec![0.0_f32; frame_count * channels];
    let mut wet_output = vec![0.0_f32; frame_count * channels];

    for channel in 0..channels {
        let input: Vec<f32> = (start..end)
            .map(|frame| finite_sample(samples[frame * channels + channel]))
            .collect();
        let predelayed = delay_line(&input, pre_delay);
        let mut comb_delays = [0.0_f32; 4];
        let mut comb_lines = Vec::with_capacity(comb_delays.len());
        let mut comb_positions = Vec::with_capacity(comb_delays.len());
        let mut comb_filters = vec![0.0_f32; comb_delays.len()];
        for (index, base_ms) in [29.7_f32, 37.1, 41.1, 43.7].into_iter().enumerate() {
            let channel_spread = if channel % 2 == 0 { 1.0 } else { 1.013 };
            let delay =
                ((base_ms * 0.001 * sample_rate as f32 * room_size * channel_spread).round()
                    as usize)
                    .max(1);
            comb_delays[index] = delay as f32 / sample_rate as f32;
            comb_lines.push(vec![0.0_f32; delay]);
            comb_positions.push(0_usize);
        }
        let feedback: Vec<_> = comb_delays
            .iter()
            .map(|delay_seconds| 10.0_f32.powf(-3.0 * delay_seconds / decay).clamp(0.0, 0.97))
            .collect();
        let mut allpass_a = Allpass::new(
            ((5.0 * 0.001 * sample_rate as f32 * room_size).round() as usize).max(1),
            0.1 + diffusion * 0.75,
        );
        let mut allpass_b = Allpass::new(
            ((1.7 * 0.001 * sample_rate as f32 * room_size).round() as usize).max(1),
            0.1 + diffusion * 0.75,
        );
        for frame in 0..frame_count {
            let source = predelayed[frame];
            let mut reverberated = 0.0;
            for index in 0..comb_lines.len() {
                let position = comb_positions[index];
                let delayed = comb_lines[index][position];
                comb_filters[index] += (1.0 - damping) * (delayed - comb_filters[index]);
                comb_lines[index][position] = source + comb_filters[index] * feedback[index];
                comb_positions[index] = (position + 1) % comb_lines[index].len();
                reverberated += delayed;
            }
            reverberated *= 0.25;
            reverberated = allpass_a.process(reverberated);
            reverberated = allpass_b.process(reverberated);
            let channel_spread = if channel % 2 == 0 { 1.0 } else { 1.013 };
            let mut early = 0.0;
            for (delay_ms, amplitude) in [
                (5.3_f32, 0.55_f32),
                (11.7, 0.35),
                (19.1, 0.22),
                (29.3, 0.12),
            ] {
                let delay = ((delay_ms * room_size * channel_spread * sample_rate as f32 / 1000.0)
                    .round() as usize)
                    .max(1);
                if frame >= delay {
                    early += predelayed[frame - delay] * amplitude * early_reflections;
                }
            }
            wet_output[frame * channels + channel] = limit_sample(reverberated + early);
        }
    }
    if channels >= 2 {
        for frame in 0..frame_count {
            let left_index = frame * channels;
            let right_index = left_index + 1;
            let left = wet_output[left_index];
            let right = wet_output[right_index];
            let mid = (left + right) * 0.5;
            let side = (left - right) * 0.5 * stereo_width;
            wet_output[left_index] = mid + side;
            wet_output[right_index] = mid - side;
        }
    }
    for frame in 0..frame_count {
        for channel in 0..channels {
            let index = frame * channels + channel;
            output[index] = limit_sample(
                samples[(start + frame) * channels + channel] * dry + wet_output[index] * wet,
            );
        }
    }
    samples[start * channels..end * channels].copy_from_slice(&output);
    Ok(())
}

fn delay_line(input: &[f32], delay: usize) -> Vec<f32> {
    if delay == 0 {
        return input.to_vec();
    }
    let mut output = vec![0.0; input.len()];
    if delay < input.len() {
        output[delay..].copy_from_slice(&input[..input.len() - delay]);
    }
    output
}

struct Allpass {
    buffer: Vec<f32>,
    position: usize,
    feedback: f32,
}

impl Allpass {
    fn new(delay: usize, feedback: f32) -> Self {
        Self {
            buffer: vec![0.0; delay],
            position: 0,
            feedback,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let delayed = self.buffer[self.position];
        let output = delayed - self.feedback * input;
        self.buffer[self.position] = input + self.feedback * output;
        self.position = (self.position + 1) % self.buffer.len();
        output
    }
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

    #[test]
    fn fades_change_only_the_selected_range() {
        let mut fade_in = [0.8, 0.8, 0.8, 0.8, 0.8];
        apply_fade_in(&mut fade_in, 1, [0.2, 0.8]).unwrap();
        assert_eq!(fade_in[0], 0.8);
        assert_eq!(&fade_in[1..4], &[0.0, 0.4, 0.8]);
        assert_eq!(fade_in[4], 0.8);

        let mut fade_out = [0.8, 0.8, 0.8, 0.8, 0.8];
        apply_fade_out(&mut fade_out, 1, [0.2, 0.8]).unwrap();
        assert_eq!(&fade_out[1..4], &[0.8, 0.4, 0.0]);
        assert_eq!(fade_out[0], 0.8);
        assert_eq!(fade_out[4], 0.8);
    }

    #[test]
    fn denoise_gate_limiter_and_reverb_process_only_the_selection() {
        let mut denoised = [0.5, 0.002, 0.002, 0.5];
        apply_denoise(&mut denoised, 1, [0.25, 0.75], -40.0, 20.0).unwrap();
        assert_eq!(denoised[0], 0.5);
        assert!(denoised[1] < 0.002);
        assert_eq!(denoised[3], 0.5);

        let mut gated = [0.5, 0.001, 0.001, 0.5];
        apply_gate(&mut gated, 1, [0.25, 0.75], -30.0).unwrap();
        assert_eq!(gated[0], 0.5);
        assert_eq!(&gated[1..3], &[0.0, 0.0]);
        assert_eq!(gated[3], 0.5);

        let mut limited = [1.0, 1.0, 1.0, 1.0];
        apply_limiter(&mut limited, 1, [0.25, 0.75], -6.0).unwrap();
        assert_eq!(limited[0], 1.0);
        assert!(limited[1..3].iter().all(|sample| sample.abs() <= 0.502));
        assert_eq!(limited[3], 1.0);

        let mut reverberated = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        apply_reverb(
            &mut reverberated,
            10,
            1,
            [0.0, 1.0],
            ReverbPreset::Room.params(),
        )
        .unwrap();
        assert_eq!(reverberated[0], 1.0);
        assert!(reverberated[1] > 0.0);
    }

    #[test]
    fn reverb_keeps_audio_outside_selection_and_preserves_stereo_separation() {
        let mut samples = [
            0.25, 0.25, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.25, 0.25,
        ];
        let original = samples;
        let params = ReverbParams {
            room_size: 0.1,
            decay_seconds: 0.2,
            pre_delay_ms: 0.0,
            wet: 1.0,
            dry: 0.0,
            ..ReverbPreset::Hall.params()
        };
        apply_reverb(&mut samples, 100, 2, [1.0 / 6.0, 5.0 / 6.0], params).unwrap();

        assert_eq!(&samples[..2], &original[..2]);
        assert_eq!(&samples[10..], &original[10..]);
        assert_ne!(&samples[2..10], &original[2..10]);
        assert_ne!(samples[4], samples[5]);
        assert!(
            samples
                .iter()
                .all(|sample| sample.is_finite() && sample.abs() <= 1.0)
        );
    }

    #[test]
    fn reverb_dry_only_leaves_selected_samples_unchanged() {
        let mut samples = [0.2, -0.5, 0.8, -0.1];
        let original = samples;
        apply_reverb(
            &mut samples,
            48_000,
            2,
            [0.0, 1.0],
            ReverbParams {
                wet: 0.0,
                dry: 1.0,
                ..ReverbPreset::Room.params()
            },
        )
        .unwrap();
        assert_eq!(samples, original);
    }

    #[test]
    fn reverb_presets_have_distinct_useful_settings() {
        let presets = ReverbPreset::ALL.map(ReverbPreset::params);
        assert!(presets.iter().all(|preset| {
            (0.1..=1.0).contains(&preset.room_size)
                && (0.2..=8.0).contains(&preset.decay_seconds)
                && (0.0..=100.0).contains(&preset.pre_delay_ms)
                && (0.0..=1.0).contains(&preset.diffusion)
                && (0.0..=1.0).contains(&preset.early_reflections)
                && (0.0..=1.0).contains(&preset.wet)
                && (0.0..=1.0).contains(&preset.stereo_width)
                && preset.dry == 1.0
        }));
        assert_ne!(presets[0], presets[1]);
        assert_ne!(presets[2], presets[4]);
    }

    #[test]
    fn reverb_diffusion_and_early_reflections_change_the_wet_signal() {
        let render = |params: ReverbParams| {
            let mut samples = vec![0.0_f32; 3000];
            samples[0] = 1.0;
            apply_reverb(&mut samples, 1000, 1, [0.0, 1.0], params).unwrap();
            samples
        };
        let base = ReverbParams {
            pre_delay_ms: 0.0,
            early_reflections: 0.0,
            wet: 1.0,
            dry: 0.0,
            ..ReverbPreset::Room.params()
        };
        let no_diffusion = render(ReverbParams {
            diffusion: 0.0,
            ..base
        });
        let diffuse = render(ReverbParams {
            diffusion: 1.0,
            ..base
        });
        let no_early = render(base);
        let with_early = render(ReverbParams {
            early_reflections: 1.0,
            ..base
        });

        assert_ne!(no_diffusion, diffuse);
        assert_ne!(no_early, with_early);
    }

    #[test]
    fn reverb_stereo_width_controls_wet_channel_spread() {
        let render = |stereo_width| {
            let mut samples = vec![0.0_f32; 4000];
            samples[0] = 1.0;
            apply_reverb(
                &mut samples,
                1000,
                2,
                [0.0, 1.0],
                ReverbParams {
                    pre_delay_ms: 0.0,
                    early_reflections: 0.0,
                    wet: 1.0,
                    dry: 0.0,
                    stereo_width,
                    ..ReverbPreset::Room.params()
                },
            )
            .unwrap();
            samples
        };
        let mono_width = render(0.0);
        let full_width = render(1.0);

        assert_ne!(mono_width, full_width);
        assert!(
            mono_width
                .chunks_exact(2)
                .all(|frame| (frame[0] - frame[1]).abs() < 1e-6)
        );
    }
}
