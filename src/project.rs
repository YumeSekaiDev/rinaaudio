use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{
    audio::{AudioData, TrackMix},
    editing::GainEnvelope,
};

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    format_version: u32,
    tracks: Vec<ProjectTrack>,
}

#[derive(Serialize, Deserialize)]
struct ProjectTrack {
    name: String,
    sample_rate: u32,
    channels: usize,
    samples: Vec<f32>,
    envelopes: Vec<GainEnvelope>,
    mix: TrackMix,
}

pub fn save_project(
    path: &Path,
    tracks: &[AudioData],
    envelopes: &[Vec<GainEnvelope>],
    settings: &[TrackMix],
) -> Result<(), String> {
    let project = ProjectFile {
        format_version: 1,
        tracks: tracks
            .iter()
            .enumerate()
            .map(|(index, track)| ProjectTrack {
                name: track.name.clone(),
                sample_rate: track.sample_rate,
                channels: track.channel_count(),
                samples: track.samples.to_vec(),
                envelopes: envelopes.get(index).cloned().unwrap_or_default(),
                mix: settings.get(index).copied().unwrap_or_default(),
            })
            .collect(),
    };
    let encoded = serde_json::to_vec(&project)
        .map_err(|error| format!("could not encode project: {error}"))?;
    std::fs::write(path, encoded).map_err(|error| format!("could not save project: {error}"))
}

pub fn load_project(
    path: &Path,
) -> Result<(Vec<AudioData>, Vec<Vec<GainEnvelope>>, Vec<TrackMix>), String> {
    let encoded =
        std::fs::read(path).map_err(|error| format!("could not read project: {error}"))?;
    let project: ProjectFile = serde_json::from_slice(&encoded)
        .map_err(|error| format!("invalid project file: {error}"))?;
    if project.format_version != 1 {
        return Err(format!(
            "unsupported project format version {}",
            project.format_version
        ));
    }

    let mut tracks = Vec::with_capacity(project.tracks.len());
    let mut envelopes = Vec::with_capacity(project.tracks.len());
    let mut settings = Vec::with_capacity(project.tracks.len());
    for track in project.tracks {
        if track.channels == 0
            || track.sample_rate == 0
            || track.samples.is_empty()
            || track.samples.len() % track.channels != 0
        {
            return Err(format!(
                "project track {:?} has invalid audio data",
                track.name
            ));
        }
        let frame_count = track.samples.len() / track.channels;
        let mut audio = AudioData {
            name: track.name,
            sample_rate: track.sample_rate,
            peaks: vec![Vec::new(); track.channels],
            samples: track.samples.into(),
            duration: frame_count as f32 / track.sample_rate as f32,
        };
        audio.rebuild_peaks();
        tracks.push(audio);
        envelopes.push(track.envelopes);
        settings.push(track.mix);
    }
    if tracks.is_empty() {
        return Err("project contains no audio tracks".to_owned());
    }
    Ok((tracks, envelopes, settings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn project_round_trip_preserves_tracks_envelopes_and_mixer_settings() {
        let path = std::env::temp_dir().join(format!(
            "rina-project-roundtrip-{}.rina",
            std::process::id()
        ));
        let tracks = vec![AudioData {
            name: "Interview".to_owned(),
            sample_rate: 48_000,
            peaks: vec![Vec::new()],
            samples: Arc::from([0.25, -0.5]),
            duration: 2.0 / 48_000.0,
        }];
        let mut envelope = GainEnvelope::default();
        envelope.set_region(0.0, 0.5, 3.0);
        let envelopes = vec![vec![envelope]];
        let settings = vec![TrackMix {
            muted: true,
            solo: false,
            volume_db: -6.0,
            pan: 0.4,
        }];

        save_project(&path, &tracks, &envelopes, &settings).unwrap();
        let (loaded, loaded_envelopes, loaded_settings) = load_project(&path).unwrap();
        std::fs::remove_file(path).unwrap();

        assert_eq!(loaded[0].name, "Interview");
        assert_eq!(loaded[0].samples.as_ref(), [0.25, -0.5]);
        assert_eq!(loaded_envelopes, envelopes);
        assert_eq!(loaded_settings, settings);
    }
}
