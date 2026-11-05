use std::sync::mpsc::{self, Receiver, Sender};

use cpal::{
    SampleFormat, SizedSample, Stream, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

use crate::audio::AudioData;

pub struct Recorder {
    stream: Stream,
    sample_receiver: Receiver<Vec<f32>>,
    sample_rate: u32,
    channels: usize,
    error_receiver: Receiver<String>,
}

impl Recorder {
    pub fn start() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "no default audio input device is available".to_owned())?;
        let device_name = device
            .name()
            .unwrap_or_else(|_| "default input device".to_owned());
        let supported_config = device.default_input_config().map_err(|error| {
            format!("could not read audio input configuration for {device_name}: {error}")
        })?;
        let sample_rate = supported_config.sample_rate().0;
        let channels = supported_config.channels() as usize;
        if sample_rate == 0 || channels == 0 {
            return Err("audio input device has invalid format metadata".to_owned());
        }

        let (sample_sender, sample_receiver) = mpsc::channel();
        let (error_sender, error_receiver) = mpsc::channel();
        let error_callback = move |error: cpal::StreamError| {
            let _ = error_sender.send(format!("audio input device error: {error}"));
        };
        let config: StreamConfig = supported_config.clone().into();
        let stream = match supported_config.sample_format() {
            SampleFormat::F32 => build_stream::<f32>(
                &device,
                &config,
                sample_sender.clone(),
                error_callback,
                |sample| sample,
            ),
            SampleFormat::I16 => build_stream::<i16>(
                &device,
                &config,
                sample_sender.clone(),
                error_callback,
                |sample| sample as f32 / i16::MAX as f32,
            ),
            SampleFormat::U16 => {
                build_stream::<u16>(&device, &config, sample_sender, error_callback, |sample| {
                    (sample as f32 / u16::MAX as f32) * 2.0 - 1.0
                })
            }
            format => {
                return Err(format!(
                    "audio input device uses unsupported sample format {format:?}"
                ));
            }
        }
        .map_err(|error| format!("could not create audio input stream: {error}"))?;
        stream
            .play()
            .map_err(|error| format!("could not start audio input: {error}"))?;

        Ok(Self {
            stream,
            sample_receiver,
            sample_rate,
            channels,
            error_receiver,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn drain_samples(&mut self, output: &mut Vec<f32>) {
        output.extend(self.sample_receiver.try_iter().flatten());
    }

    pub fn stop(self, mut samples: Vec<f32>) -> Result<AudioData, String> {
        let Recorder {
            stream,
            sample_receiver,
            sample_rate,
            channels,
            error_receiver,
        } = self;
        drop(stream);
        samples.extend(sample_receiver.try_iter().flatten());
        if let Ok(error) = error_receiver.try_recv() {
            return Err(error);
        }
        if samples.is_empty() || samples.len() % channels != 0 {
            return Err("recording contains no complete audio frames".to_owned());
        }
        let frames = samples.len() / channels;
        let mut audio = AudioData {
            name: format!("Recording {}", chrono_label()),
            sample_rate,
            peaks: vec![Vec::new(); channels],
            samples: samples.into(),
            duration: frames as f32 / sample_rate as f32,
        };
        audio.rebuild_peaks();
        Ok(audio)
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    sample_sender: Sender<Vec<f32>>,
    error_callback: impl FnMut(cpal::StreamError) + Send + 'static,
    convert: impl Fn(T) -> f32 + Send + 'static,
) -> Result<Stream, cpal::BuildStreamError>
where
    T: SizedSample + Copy + Send + 'static,
{
    device.build_input_stream(
        config,
        move |input: &[T], _| {
            let samples: Vec<f32> = input
                .iter()
                .copied()
                .map(|sample| {
                    let converted = convert(sample);
                    if converted.is_finite() {
                        converted.clamp(-1.0, 1.0)
                    } else {
                        0.0
                    }
                })
                .collect();
            let _ = sample_sender.send(samples);
        },
        error_callback,
        None,
    )
}

fn chrono_label() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now().duration_since(UNIX_EPOCH).map_or_else(
        |_| "take".to_owned(),
        |duration| duration.as_secs().to_string(),
    )
}
