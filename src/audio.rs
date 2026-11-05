use std::{fs::File, io::ErrorKind, path::Path, sync::Arc};

use symphonia::core::{
    codecs::DecoderOptions, errors::Error as SymphoniaError, formats::FormatOptions,
    io::MediaSourceStream, meta::MetadataOptions, probe::Hint,
};

pub struct AudioData {
    pub name: String,
    pub sample_rate: u32,
    pub peaks: Vec<Vec<[f32; 2]>>,
    pub samples: Arc<[f32]>,
    pub duration: f32,
}

impl AudioData {
    pub fn channel_count(&self) -> usize {
        self.peaks.len()
    }
}

pub fn decode_audio(path: &Path) -> Result<AudioData, String> {
    let file = File::open(path).map_err(|error| format!("could not open file: {error}"))?;
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(extension);
    }

    let source = MediaSourceStream::new(Box::new(file), Default::default());
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            source,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|error| format!("unsupported or invalid audio file: {error}"))?;
    let mut format = probed.format;
    let (track_id, codec_params) = {
        let track = format
            .default_track()
            .ok_or_else(|| "audio file contains no audio track".to_owned())?;
        (track.id, track.codec_params.clone())
    };
    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &DecoderOptions::default())
        .map_err(|error| format!("unsupported audio codec: {error}"))?;

    const PEAK_BUCKET_FRAMES: usize = 512;
    let mut peaks: Vec<Vec<[f32; 2]>> = Vec::new();
    let mut bucket_peaks: Vec<[f32; 2]> = Vec::new();
    let mut samples = Vec::new();
    let mut frames_in_bucket = 0;
    let mut total_frames = 0_u64;
    let mut sample_rate = 0;

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(error)) if error.kind() == ErrorKind::UnexpectedEof => {
                break;
            }
            Err(error) => return Err(format!("could not read audio data: {error}")),
        };
        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(error) => return Err(format!("could not decode audio data: {error}")),
        };
        let current_rate = decoded.spec().rate;
        let current_channels = decoded.spec().channels.count();
        if current_channels == 0 || current_rate == 0 {
            return Err("audio track has invalid channel or sample-rate metadata".to_owned());
        }
        if sample_rate != 0 && sample_rate != current_rate {
            return Err("sample rate changes within the audio track".to_owned());
        }
        if peaks.is_empty() {
            sample_rate = current_rate;
            peaks = vec![Vec::new(); current_channels];
            bucket_peaks = vec![[f32::INFINITY, f32::NEG_INFINITY]; current_channels];
        } else if peaks.len() != current_channels {
            return Err("channel count changes within the audio track".to_owned());
        }

        let mut decoded_samples = symphonia::core::audio::SampleBuffer::<f32>::new(
            decoded.capacity() as u64,
            *decoded.spec(),
        );
        decoded_samples.copy_interleaved_ref(decoded);
        for frame in decoded_samples.samples().chunks_exact(current_channels) {
            for (channel, &sample) in frame.iter().enumerate() {
                let sample = if sample.is_finite() {
                    sample.clamp(-1.0, 1.0)
                } else {
                    0.0
                };
                samples.push(sample);
                bucket_peaks[channel][0] = bucket_peaks[channel][0].min(sample);
                bucket_peaks[channel][1] = bucket_peaks[channel][1].max(sample);
            }
            frames_in_bucket += 1;
            total_frames += 1;
            if frames_in_bucket == PEAK_BUCKET_FRAMES {
                for (channel, channel_peaks) in peaks.iter_mut().zip(bucket_peaks.iter_mut()) {
                    channel.push(*channel_peaks);
                    *channel_peaks = [f32::INFINITY, f32::NEG_INFINITY];
                }
                frames_in_bucket = 0;
            }
        }
    }

    if frames_in_bucket > 0 {
        for (channel, channel_peaks) in peaks.iter_mut().zip(bucket_peaks) {
            channel.push(channel_peaks);
        }
    }
    if total_frames == 0 || sample_rate == 0 {
        return Err("audio file contains no decodable samples".to_owned());
    }

    Ok(AudioData {
        name: String::new(),
        sample_rate,
        peaks,
        samples: samples.into(),
        duration: total_frames as f32 / sample_rate as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write};

    #[test]
    fn decodes_stereo_pcm_samples_and_waveform_peaks() {
        let path =
            std::env::temp_dir().join(format!("rina-audio-import-{}.wav", std::process::id()));
        let samples = [-32768_i16, 0, 16384, -16384, 0, 32767, 8192, 0];
        let data_len = (samples.len() * 2) as u32;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&8000_u32.to_le_bytes());
        wav.extend_from_slice(&32000_u32.to_le_bytes());
        wav.extend_from_slice(&4_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        fs::File::create(&path).unwrap().write_all(&wav).unwrap();

        let audio = decode_audio(&path).unwrap();
        fs::remove_file(path).unwrap();

        assert_eq!(audio.sample_rate, 8000);
        assert_eq!(audio.channel_count(), 2);
        assert!((audio.duration - 0.0005).abs() < 0.000001);
        assert_eq!(audio.peaks[0].len(), 1);
        assert_eq!(audio.peaks[0][0][0], -1.0);
        assert!((audio.peaks[0][0][1] - 0.5).abs() < 0.0001);
        assert_eq!(audio.samples.len(), samples.len());
        assert_eq!(audio.samples[0], -1.0);
        assert!(audio.samples[5] > 0.99);
    }
}
