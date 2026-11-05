use std::sync::{
    Arc, Mutex, RwLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use cpal::{
    FromSample, SampleFormat, SizedSample, Stream, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

use crate::editing::{GainEnvelope, db_to_linear};

const POSITION_FRACTION_BITS: u32 = 32;
const POSITION_ONE: u64 = 1 << POSITION_FRACTION_BITS;

struct SharedPlayback {
    samples: RwLock<Arc<[f32]>>,
    source_rate: u32,
    source_channels: usize,
    source_frames: u64,
    playback_end_frame: AtomicU64,
    position: AtomicU64,
    playing: AtomicBool,
    gain: RwLock<Vec<GainEnvelope>>,
    device_error: Mutex<Option<String>>,
}

pub struct Playback {
    stream: Stream,
    shared: Arc<SharedPlayback>,
    output_rate: u32,
}

impl Playback {
    pub fn new(
        samples: Arc<[f32]>,
        source_rate: u32,
        source_channels: usize,
        gain: Vec<GainEnvelope>,
    ) -> Result<Self, String> {
        if source_rate == 0 || source_channels == 0 || samples.is_empty() {
            return Err("audio track has no playable samples".to_owned());
        }
        if samples.len() % source_channels != 0 {
            return Err("audio sample data has an incomplete channel frame".to_owned());
        }

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "no default audio output device is available".to_owned())?;
        let device_name = device
            .name()
            .unwrap_or_else(|_| "default output device".to_owned());
        let supported_config = device.default_output_config().map_err(|error| {
            format!("could not read audio output configuration for {device_name}: {error}")
        })?;
        let output_rate = supported_config.sample_rate().0;
        let output_channels = supported_config.channels() as usize;
        let config: StreamConfig = supported_config.clone().into();
        let shared = Arc::new(SharedPlayback {
            source_frames: (samples.len() / source_channels) as u64,
            playback_end_frame: AtomicU64::new((samples.len() / source_channels) as u64),
            samples: RwLock::new(samples),
            source_rate,
            source_channels,
            position: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            gain: RwLock::new(gain),
            device_error: Mutex::new(None),
        });
        let error_shared = Arc::clone(&shared);
        let error_callback = move |error: cpal::StreamError| {
            if let Ok(mut slot) = error_shared.device_error.lock() {
                *slot = Some(format!("audio output device error: {error}"));
            }
            error_shared.playing.store(false, Ordering::Release);
        };

        let stream = match supported_config.sample_format() {
            SampleFormat::F32 => build_stream::<f32>(
                &device,
                &config,
                Arc::clone(&shared),
                output_channels,
                error_callback,
            ),
            SampleFormat::I16 => build_stream::<i16>(
                &device,
                &config,
                Arc::clone(&shared),
                output_channels,
                error_callback,
            ),
            SampleFormat::U16 => build_stream::<u16>(
                &device,
                &config,
                Arc::clone(&shared),
                output_channels,
                error_callback,
            ),
            format => {
                return Err(format!(
                    "audio output device uses unsupported sample format {format:?}"
                ));
            }
        }
        .map_err(|error| format!("could not create audio output stream: {error}"))?;

        Ok(Self {
            stream,
            shared,
            output_rate,
        })
    }

    pub fn play(&self) -> Result<(), String> {
        self.stream
            .play()
            .map_err(|error| format!("could not start audio output: {error}"))?;
        if self.position_frames() >= self.shared.source_frames {
            self.seek_frame(0);
        }
        self.shared.playing.store(true, Ordering::Release);
        Ok(())
    }

    pub fn pause(&self) {
        self.shared.playing.store(false, Ordering::Release);
    }

    pub fn stop(&self) {
        self.shared.playing.store(false, Ordering::Release);
        self.seek_frame(0);
    }

    pub fn seek_frame(&self, frame: u64) {
        let frame = frame.min(self.shared.source_frames);
        self.shared
            .position
            .store(frame.saturating_mul(POSITION_ONE), Ordering::Release);
    }

    pub fn set_playback_end_frame(&self, frame: u64) {
        self.shared
            .playback_end_frame
            .store(frame.min(self.shared.source_frames), Ordering::Release);
    }

    pub fn position_frames(&self) -> u64 {
        self.shared.position.load(Ordering::Acquire) >> POSITION_FRACTION_BITS
    }

    pub fn source_frames(&self) -> u64 {
        self.shared.source_frames
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Acquire)
    }

    pub fn set_gain_envelopes(&self, gain: Vec<GainEnvelope>) {
        if let Ok(mut current) = self.shared.gain.write() {
            *current = gain;
        }
    }

    pub fn set_samples(&self, samples: Arc<[f32]>) {
        if samples.len() == self.shared.source_frames as usize * self.shared.source_channels
            && let Ok(mut current) = self.shared.samples.write()
        {
            *current = samples;
        }
    }

    pub fn take_device_error(&self) -> Option<String> {
        self.shared
            .device_error
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
    }

    pub fn output_rate(&self) -> u32 {
        self.output_rate
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    shared: Arc<SharedPlayback>,
    output_channels: usize,
    error_callback: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<Stream, cpal::BuildStreamError>
where
    T: SizedSample + FromSample<f32>,
{
    let step =
        ((shared.source_rate as f64 / config.sample_rate.0 as f64) * POSITION_ONE as f64) as u64;
    device.build_output_stream(
        config,
        move |output: &mut [T], _| {
            render(output, output_channels, step, &shared);
        },
        error_callback,
        None,
    )
}

fn render<T>(output: &mut [T], output_channels: usize, step: u64, shared: &SharedPlayback)
where
    T: cpal::Sample + FromSample<f32> + Copy,
{
    let silence = T::from_sample(0.0);
    let gain = shared.gain.read().ok();
    let samples = shared.samples.read().ok();
    for output_frame in output.chunks_mut(output_channels.max(1)) {
        if !shared.playing.load(Ordering::Acquire) {
            output_frame.fill(silence);
            continue;
        }

        let position = shared.position.load(Ordering::Relaxed);
        let source_frame = position >> POSITION_FRACTION_BITS;
        let playback_end_frame = shared
            .playback_end_frame
            .load(Ordering::Acquire)
            .min(shared.source_frames);
        if source_frame >= playback_end_frame || samples.is_none() {
            shared.position.store(
                playback_end_frame.saturating_mul(POSITION_ONE),
                Ordering::Release,
            );
            shared.playing.store(false, Ordering::Release);
            output_frame.fill(silence);
            continue;
        }

        let fraction = (position & (POSITION_ONE - 1)) as f32 / POSITION_ONE as f32;
        let next_frame = (source_frame + 1).min(playback_end_frame - 1);
        for (output_channel, sample) in output_frame.iter_mut().enumerate() {
            let value = if output_channels == 1 && shared.source_channels > 1 {
                let mut sum = 0.0;
                for source_channel in 0..shared.source_channels {
                    let gain_db = gain
                        .as_ref()
                        .and_then(|envelopes| envelopes.get(source_channel))
                        .map_or(0.0, |envelope| {
                            envelope.gain_at(source_frame as f32 / shared.source_frames as f32)
                        });
                    sum += interpolated_sample(
                        shared,
                        samples.as_ref().unwrap(),
                        source_frame,
                        next_frame,
                        fraction,
                        source_channel,
                    ) * db_to_linear(gain_db);
                }
                sum / shared.source_channels as f32
            } else {
                let source_channel = if shared.source_channels == 1 {
                    0
                } else {
                    output_channel.min(shared.source_channels - 1)
                };
                let gain_db = gain
                    .as_ref()
                    .and_then(|envelopes| envelopes.get(source_channel))
                    .map_or(0.0, |envelope| {
                        envelope.gain_at(source_frame as f32 / shared.source_frames as f32)
                    });
                interpolated_sample(
                    shared,
                    samples.as_ref().unwrap(),
                    source_frame,
                    next_frame,
                    fraction,
                    source_channel,
                ) * db_to_linear(gain_db)
            };
            *sample = T::from_sample(value.clamp(-1.0, 1.0));
        }
        shared
            .position
            .store(position.saturating_add(step), Ordering::Release);
    }
}

fn interpolated_sample(
    shared: &SharedPlayback,
    samples: &[f32],
    frame: u64,
    next_frame: u64,
    fraction: f32,
    channel: usize,
) -> f32 {
    let first = samples[frame as usize * shared.source_channels + channel];
    let next = samples[next_frame as usize * shared.source_channels + channel];
    first + (next - first) * fraction
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared(samples: &[f32], channels: usize, gain: GainEnvelope) -> SharedPlayback {
        SharedPlayback {
            samples: RwLock::new(Arc::from(samples)),
            source_rate: 48_000,
            source_channels: channels,
            source_frames: (samples.len() / channels) as u64,
            playback_end_frame: AtomicU64::new((samples.len() / channels) as u64),
            position: AtomicU64::new(0),
            playing: AtomicBool::new(true),
            gain: RwLock::new(vec![gain.clone(), gain]),
            device_error: Mutex::new(None),
        }
    }

    #[test]
    fn renders_samples_with_time_range_gain_and_stereo_channel_order() {
        let mut gain = GainEnvelope::default();
        gain.set_region(0.0, 0.5, 6.0206);
        let shared = shared(&[0.25, -0.5, 0.5, -1.0], 2, gain);
        let mut output = [0.0_f32; 4];

        render(&mut output, 2, POSITION_ONE, &shared);

        assert!((output[0] - 0.5).abs() < 0.001);
        assert!((output[1] + 1.0).abs() < 0.001);
        assert!((output[2] - 0.5).abs() < 0.001);
        assert!((output[3] + 1.0).abs() < 0.001);
        assert_eq!(
            shared.position.load(Ordering::Acquire) >> POSITION_FRACTION_BITS,
            2
        );
    }

    #[test]
    fn playback_stops_and_outputs_silence_at_end_of_file() {
        let shared = shared(&[0.0, 0.0, 0.25, -0.5], 2, GainEnvelope::default());
        shared.position.store(POSITION_ONE, Ordering::Release);
        let mut output = [1.0_f32; 4];

        render(&mut output, 2, POSITION_ONE, &shared);

        assert_eq!(output, [0.25, -0.5, 0.0, 0.0]);
        assert!(!shared.playing.load(Ordering::Acquire));
        assert_eq!(shared.position.load(Ordering::Acquire), 2 * POSITION_ONE);
    }

    #[test]
    fn playback_stops_at_selected_end_without_rendering_beyond_it() {
        let shared = shared(&[0.1, 0.2, 0.3, 0.4], 1, GainEnvelope::default());
        shared.playback_end_frame.store(2, Ordering::Release);
        shared.position.store(POSITION_ONE, Ordering::Release);
        let mut output = [0.0_f32; 3];

        render(&mut output, 1, POSITION_ONE, &shared);

        assert_eq!(output, [0.2, 0.0, 0.0]);
        assert!(!shared.playing.load(Ordering::Acquire));
        assert_eq!(shared.position.load(Ordering::Acquire), 2 * POSITION_ONE);
    }

    #[test]
    fn paused_playback_outputs_silence_without_moving_the_cursor() {
        let shared = shared(&[0.1, 0.2], 1, GainEnvelope::default());
        shared.playing.store(false, Ordering::Release);
        let mut output = [1.0_f32; 2];

        render(&mut output, 1, POSITION_ONE, &shared);

        assert_eq!(output, [0.0, 0.0]);
        assert_eq!(shared.position.load(Ordering::Acquire), 0);
    }

    #[test]
    fn playback_applies_left_and_right_gain_envelopes_independently() {
        let mut left = GainEnvelope::default();
        left.set_region(0.0, 1.0, 6.0206);
        let shared = shared(&[0.25, 0.25], 2, GainEnvelope::default());
        *shared.gain.write().unwrap() = vec![left, GainEnvelope::default()];
        let mut output = [0.0_f32; 2];

        render(&mut output, 2, POSITION_ONE, &shared);

        assert!((output[0] - 0.5).abs() < 0.001);
        assert!((output[1] - 0.25).abs() < 0.001);
    }
}
