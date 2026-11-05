use eframe::egui::{
    self, Align, Color32, FontId, Frame, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, Vec2,
};
use std::{
    sync::Arc,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::Duration,
};

mod audio;
mod dsp;
mod editing;
mod playback;
mod recording;

use audio::{AudioData, decode_audio, export_wav};
use dsp::{
    Compressor, Equalizer, apply_compressor, apply_denoise, apply_equalizer, apply_fade_in,
    apply_fade_out, apply_gate, apply_limiter, apply_reverb,
};
use editing::{GainEnvelope, db_to_linear, normalize_region};
use playback::Playback;
use recording::Recorder;

const BG: Color32 = Color32::from_rgb(15, 18, 24);
const PANEL: Color32 = Color32::from_rgb(21, 25, 33);
const PANEL_ALT: Color32 = Color32::from_rgb(27, 32, 42);
const BORDER: Color32 = Color32::from_rgb(43, 50, 63);
const TEXT: Color32 = Color32::from_rgb(231, 236, 243);
const MUTED: Color32 = Color32::from_rgb(134, 145, 162);
const ACCENT: Color32 = Color32::from_rgb(105, 219, 190);
const WAVE: Color32 = Color32::from_rgb(84, 191, 170);
const ORANGE: Color32 = Color32::from_rgb(244, 173, 100);
const RECORDING_RED: Color32 = Color32::from_rgb(220, 54, 64);

#[derive(Clone, Copy)]
enum Icon {
    Select,
    Trim,
    Split,
    Draw,
    Undo,
    Redo,
    ZoomIn,
    ZoomOut,
    More,
    Previous,
    Play,
    Pause,
    Stop,
    Next,
    Record,
    Action,
    Chevron,
    Brand,
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([900.0, 620.0])
            .with_title("Rina Audio Studio"),
        ..Default::default()
    };

    eframe::run_native(
        "Rina Audio Studio",
        options,
        Box::new(|_creation_context| Ok(Box::new(RinaApp::default()))),
    )
}

#[derive(Clone)]
struct EditState {
    gain_db: f32,
    track_index: usize,
    channel_index: usize,
    stereo_mode: bool,
    selection: [f32; 2],
    channel_selections: [[f32; 2]; 2],
    gain_envelope: GainEnvelope,
    track_envelopes: Vec<GainEnvelope>,
    samples: Option<Arc<[f32]>>,
    peaks: Option<Vec<Vec<[f32; 2]>>>,
}

#[derive(Clone, Copy, PartialEq)]
enum EffectPanel {
    Equalizer,
    Compressor,
}

struct RinaApp {
    tracks: Vec<AudioData>,
    track_envelopes: Vec<Vec<GainEnvelope>>,
    selected_track: usize,
    playback: Option<Playback>,
    recorder: Option<Recorder>,
    recording_track: Option<usize>,
    recording_previous_track: Option<usize>,
    recording_samples: Vec<f32>,
    recording_bucket_frames: usize,
    recording_view_seconds: f32,
    import_receiver: Option<Receiver<Result<AudioData, String>>>,
    export_receiver: Option<Receiver<Result<(), String>>>,
    gain_envelope: GainEnvelope,
    effect_panel: Option<EffectPanel>,
    equalizer: Equalizer,
    compressor: Compressor,
    selection: [f32; 2],
    channel_selections: [[f32; 2]; 2],
    active_channel: usize,
    stereo_mode: bool,
    channel_clipboard: Vec<f32>,
    clipboard_channels: usize,
    gain_db: f32,
    gain_dragging: bool,
    gain_drag_lane: usize,
    playing: bool,
    waveform_focused: bool,
    zoom: f32,
    view_start: f32,
    playhead: f32,
    snap_enabled: bool,
    active_tool: &'static str,
    notice: String,
    undo: Vec<EditState>,
    redo: Vec<EditState>,
}

impl Default for RinaApp {
    fn default() -> Self {
        Self {
            tracks: Vec::new(),
            track_envelopes: Vec::new(),
            selected_track: 0,
            playback: None,
            recorder: None,
            recording_track: None,
            recording_previous_track: None,
            recording_samples: Vec::new(),
            recording_bucket_frames: 0,
            recording_view_seconds: 10.0,
            import_receiver: None,
            export_receiver: None,
            gain_envelope: GainEnvelope::default(),
            effect_panel: None,
            equalizer: Equalizer::default(),
            compressor: Compressor::default(),
            selection: [0.18, 0.63],
            channel_selections: [[0.18, 0.63], [0.18, 0.63]],
            active_channel: 0,
            stereo_mode: true,
            channel_clipboard: Vec::new(),
            clipboard_channels: 0,
            gain_db: 0.0,
            gain_dragging: false,
            gain_drag_lane: 0,
            playing: false,
            waveform_focused: false,
            zoom: 1.0,
            view_start: 0.0,
            playhead: 0.26,
            snap_enabled: false,
            active_tool: "Select",
            notice: "Ready".to_owned(),
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
}

impl RinaApp {
    fn audio(&self) -> Option<&AudioData> {
        self.tracks.get(self.selected_track)
    }

    fn audio_mut(&mut self) -> Option<&mut AudioData> {
        self.tracks.get_mut(self.selected_track)
    }

    fn select_track(&mut self, index: usize) {
        if self.recorder.is_some() {
            return;
        }
        if index < self.tracks.len() && index != self.selected_track {
            if let Some(playback) = &self.playback {
                playback.pause();
            }
            self.playback = None;
            self.playing = false;
            if let Some(envelopes) = self.track_envelopes.get_mut(self.selected_track) {
                if let Some(envelope) = envelopes.get_mut(self.active_channel) {
                    *envelope = self.gain_envelope.clone();
                }
            }
            self.selected_track = index;
            self.active_channel = 0;
            self.stereo_mode = true;
            self.channel_selections = [[0.0, 1.0], [0.0, 1.0]];
            self.selection = self.channel_selections[0];
            self.gain_envelope = self
                .track_envelopes
                .get(index)
                .and_then(|envelopes| envelopes.first())
                .cloned()
                .unwrap_or_default();
            self.gain_db = 0.0;
            self.playhead = 0.0;
            self.undo.clear();
            self.redo.clear();
        }
    }

    fn select_channel(&mut self, channel: usize) {
        let channel_count = self.audio().map_or(0, AudioData::channel_count);
        if channel >= channel_count.min(2) || (channel == self.active_channel && !self.stereo_mode)
        {
            return;
        }
        self.save_active_channel_state();
        self.active_channel = channel;
        self.stereo_mode = false;
        self.selection = self.channel_selections[channel];
        self.gain_envelope = self
            .track_envelopes
            .get(self.selected_track)
            .and_then(|envelopes| envelopes.get(channel))
            .cloned()
            .unwrap_or_default();
        self.gain_db = self
            .gain_envelope
            .gain_at((self.selection[0] + self.selection[1]) * 0.5);
    }

    fn active_envelopes(&self) -> Vec<GainEnvelope> {
        self.track_envelopes
            .get(self.selected_track)
            .cloned()
            .unwrap_or_default()
    }

    fn envelopes_for_active_track(&self) -> Vec<GainEnvelope> {
        let mut envelopes = self.active_envelopes();
        if self.stereo_mode {
            for envelope in envelopes.iter_mut().take(2) {
                *envelope = self.gain_envelope.clone();
            }
        } else if let Some(envelope) = envelopes.get_mut(self.active_channel) {
            *envelope = self.gain_envelope.clone();
        }
        envelopes
    }

    fn save_active_channel_state(&mut self) {
        if self.stereo_mode {
            self.channel_selections = [self.selection; 2];
            if let Some(envelopes) = self.track_envelopes.get_mut(self.selected_track) {
                for envelope in envelopes.iter_mut().take(2) {
                    *envelope = self.gain_envelope.clone();
                }
            }
            return;
        }
        self.channel_selections[self.active_channel] = self.selection;
        if let Some(envelopes) = self.track_envelopes.get_mut(self.selected_track)
            && let Some(envelope) = envelopes.get_mut(self.active_channel)
        {
            *envelope = self.gain_envelope.clone();
        }
    }

    fn select_all(&mut self) {
        self.selection = [0.0, 1.0];
        if self.stereo_mode {
            self.channel_selections = [self.selection; 2];
        } else {
            self.channel_selections[self.active_channel] = self.selection;
        }
        self.save_active_channel_state();
    }

    fn select_stereo(&mut self) {
        self.save_active_channel_state();
        self.stereo_mode = true;
        self.channel_selections = [self.selection; 2];
        self.save_active_channel_state();
        let midpoint = (self.selection[0] + self.selection[1]) * 0.5;
        self.gain_db = self.gain_envelope.gain_at(midpoint);
    }

    fn copy_channel(&mut self, source: usize, destination: usize) {
        let Some(audio) = self.audio() else {
            self.notice = "Import audio before copying channels".to_owned();
            return;
        };
        let channels = audio.channel_count();
        if source >= channels || destination >= channels || source == destination {
            self.notice = "Channel copy requires distinct left and right channels".to_owned();
            return;
        }
        let Some((start, end)) =
            selection_frame_range(self.selection, (audio.samples.len() / channels) as u64)
        else {
            self.notice = "Select audio to copy between channels".to_owned();
            return;
        };
        self.save_edit();
        if let Some(audio) = self.audio_mut() {
            let mut samples = audio.samples.to_vec();
            for frame in start as usize..end as usize {
                samples[frame * channels + destination] = samples[frame * channels + source];
            }
            audio.samples = samples.into();
            audio.rebuild_peaks();
            let updated_samples = Arc::clone(&audio.samples);
            if let Some(playback) = &self.playback {
                playback.set_samples(updated_samples);
            }
        }
        self.notice = format!(
            "Copied {} to {}",
            channel_label_single(source),
            channel_label_single(destination)
        );
    }

    fn copy_selection(&mut self) -> bool {
        let Some(audio) = self.audio() else {
            self.notice = "Import audio before copying".to_owned();
            return false;
        };
        let channels = audio.channel_count();
        let channel = if self.stereo_mode {
            audio.samples.to_vec()
        } else {
            interleave_channel(&audio.samples, channels, self.active_channel)
        };
        let frames = (channel.len() / if self.stereo_mode { channels } else { 1 }) as u64;
        let Some((start, end)) = selection_frame_range(self.selection, frames) else {
            self.notice = "Select audio to copy".to_owned();
            return false;
        };
        let clipboard_channels = if self.stereo_mode { channels } else { 1 };
        let start = start as usize * clipboard_channels;
        let end = end as usize * clipboard_channels;
        self.channel_clipboard = channel[start..end].to_vec();
        self.clipboard_channels = clipboard_channels;
        self.notice = format!("Copied {} channel samples", self.channel_clipboard.len());
        true
    }

    fn silence_selection(&mut self) {
        self.process_selection("Silence", |samples, _, channels, selection| {
            let (start, end) = selection_frame_range(selection, (samples.len() / channels) as u64)
                .ok_or("Select audio to silence")?;
            let start = start as usize * channels;
            let end = end as usize * channels;
            samples[start..end].fill(0.0);
            Ok(())
        });
    }

    fn delete_selection(&mut self) {
        let Some(audio) = self.audio() else {
            self.notice = "Import audio before deleting".to_owned();
            return;
        };
        let channels = audio.channel_count();
        if channels == 0 {
            self.notice = "Audio has no channels to delete".to_owned();
            return;
        }
        let frame_count = audio.samples.len() / channels;
        let Some((mut start, mut end)) = selection_frame_range(self.selection, frame_count as u64)
        else {
            self.notice = "Select audio to delete".to_owned();
            return;
        };

        if self.stereo_mode && self.snap_enabled {
            let samples = Arc::clone(&audio.samples);
            let radius = (audio.sample_rate as usize / 100).max(1);
            start = nearest_zero_crossing(&samples, channels, start as usize, radius)
                .unwrap_or(start as usize) as u64;
            end = nearest_zero_crossing(&samples, channels, end as usize, radius)
                .unwrap_or(end as usize) as u64;
        }
        if end <= start {
            self.notice = "Zero-crossing snap leaves no audio selected".to_owned();
            return;
        }

        let stereo_mode = self.stereo_mode;
        let active_channel = self.active_channel;
        let frame_count = frame_count as u64;
        self.save_edit();
        if let Some(audio) = self.audio_mut() {
            let mut samples = audio.samples.to_vec();
            if stereo_mode {
                let start_sample = start as usize * channels;
                let end_sample = end as usize * channels;
                samples.drain(start_sample..end_sample);
            } else {
                for frame in start as usize..end as usize {
                    samples[frame * channels + active_channel] = 0.0;
                }
            }
            audio.samples = samples.into();
            audio.duration = (audio.samples.len() / channels) as f32 / audio.sample_rate as f32;
            audio.rebuild_peaks();
            let updated_samples = Arc::clone(&audio.samples);
            if let Some(playback) = &self.playback {
                playback.set_samples(updated_samples);
            }
        }
        let remaining_frames = if stereo_mode {
            frame_count.saturating_sub(end - start)
        } else {
            frame_count
        };
        let start = start as f32 / remaining_frames.max(1) as f32;
        self.selection = [start, start];
        if stereo_mode {
            self.channel_selections = [self.selection; 2];
        } else {
            self.channel_selections[active_channel] = self.selection;
        }
        self.playhead = start;
        self.notice = if stereo_mode {
            "Deleted selected time from both channels".to_owned()
        } else {
            "Silenced selected channel range".to_owned()
        };
    }

    fn cut_selection(&mut self) {
        if self.copy_selection() {
            self.delete_selection();
        }
    }

    fn paste_selection(&mut self) {
        if self.channel_clipboard.is_empty() {
            self.notice = "Copy channel audio before pasting".to_owned();
            return;
        }
        let Some(audio) = self.audio() else {
            self.notice = "Import audio before pasting".to_owned();
            return;
        };
        let channels = audio.channel_count();
        if channels == 0 {
            self.notice = "Audio has no channels to paste into".to_owned();
            return;
        }
        let paste_channels = if self.stereo_mode { channels } else { 1 };
        if self.clipboard_channels != paste_channels {
            self.notice = "Paste channel mode must match the copied audio".to_owned();
            return;
        }

        let clipboard = self.channel_clipboard.clone();
        let clipboard_frames = clipboard.len() / paste_channels;
        let frame_count = audio.samples.len() / channels;
        let selected_range = selection_frame_range(self.selection, frame_count as u64)
            .filter(|(start, end)| (*end - *start) as usize == clipboard_frames);
        let start = selected_range.map_or_else(
            || (self.playhead.clamp(0.0, 1.0) * frame_count as f32).floor() as usize,
            |(start, _)| start as usize,
        );
        let end = start + clipboard_frames;
        let stereo_mode = self.stereo_mode;
        let active_channel = self.active_channel;

        self.save_edit();
        if let Some(audio) = self.audio_mut() {
            let mut samples = audio.samples.to_vec();
            samples.resize(samples.len().max(end * channels), 0.0);
            for frame in 0..clipboard_frames {
                if stereo_mode {
                    let sample_start = frame * channels;
                    let destination_start = (start + frame) * channels;
                    samples[destination_start..destination_start + channels]
                        .copy_from_slice(&clipboard[sample_start..sample_start + channels]);
                } else {
                    samples[(start + frame) * channels + active_channel] = clipboard[frame];
                }
            }
            audio.samples = samples.into();
            audio.duration = (audio.samples.len() / channels) as f32 / audio.sample_rate as f32;
            audio.rebuild_peaks();
            let updated_samples = Arc::clone(&audio.samples);
            if let Some(playback) = &self.playback {
                playback.set_samples(updated_samples);
            }
        }
        self.notice = if selected_range.is_some() {
            "Pasted into selection".to_owned()
        } else {
            "Pasted at cursor".to_owned()
        };
    }

    fn save_edit(&mut self) {
        self.save_edit_with_gain(self.gain_db);
    }

    fn save_edit_with_gain(&mut self, gain_db: f32) {
        self.undo.push(EditState {
            gain_db,
            track_index: self.selected_track,
            channel_index: self.active_channel,
            stereo_mode: self.stereo_mode,
            gain_envelope: self.gain_envelope.clone(),
            track_envelopes: self.active_envelopes(),
            selection: self.selection,
            channel_selections: self.channel_selections,
            samples: self.audio().map(|audio| Arc::clone(&audio.samples)),
            peaks: self.audio().map(|audio| audio.peaks.clone()),
        });
        self.redo.clear();
    }

    fn restore(&mut self, state: EditState) {
        self.gain_db = state.gain_db;
        self.selected_track = state.track_index;
        self.active_channel = state.channel_index;
        self.stereo_mode = state.stereo_mode;
        self.selection = state.selection;
        self.channel_selections = state.channel_selections;
        self.gain_envelope = state.gain_envelope;
        if let Some(envelopes) = self.track_envelopes.get_mut(self.selected_track) {
            *envelopes = state.track_envelopes;
        }
        if let (Some(audio), Some(samples)) = (self.audio_mut(), state.samples) {
            audio.samples = samples;
        }
        if let (Some(audio), Some(peaks)) = (self.audio_mut(), state.peaks) {
            audio.peaks = peaks;
        }
        if let Some(audio) = self.audio_mut() {
            audio.duration = (audio.samples.len() / audio.channel_count().max(1)) as f32
                / audio.sample_rate as f32;
        }
        if let Some(playback) = &self.playback {
            playback.set_gain_envelopes(self.envelopes_for_active_track());
            if let Some(audio) = self.audio() {
                playback.set_samples(Arc::clone(&audio.samples));
            }
        }
    }

    fn undo(&mut self) {
        if let Some(state) = self.undo.pop() {
            self.redo.push(EditState {
                gain_db: self.gain_db,
                track_index: self.selected_track,
                channel_index: self.active_channel,
                stereo_mode: self.stereo_mode,
                gain_envelope: self.gain_envelope.clone(),
                track_envelopes: self.active_envelopes(),
                selection: self.selection,
                channel_selections: self.channel_selections,
                samples: self.audio().map(|audio| Arc::clone(&audio.samples)),
                peaks: self.audio().map(|audio| audio.peaks.clone()),
            });
            self.restore(state);
            self.notice = "Edit undone".to_owned();
        }
    }

    fn redo(&mut self) {
        if let Some(state) = self.redo.pop() {
            self.undo.push(EditState {
                gain_db: self.gain_db,
                track_index: self.selected_track,
                channel_index: self.active_channel,
                stereo_mode: self.stereo_mode,
                gain_envelope: self.gain_envelope.clone(),
                track_envelopes: self.active_envelopes(),
                selection: self.selection,
                channel_selections: self.channel_selections,
                samples: self.audio().map(|audio| Arc::clone(&audio.samples)),
                peaks: self.audio().map(|audio| audio.peaks.clone()),
            });
            self.restore(state);
            self.notice = "Edit restored".to_owned();
        }
    }

    fn duration(&self) -> f32 {
        self.audio().map_or(42.0, |audio| audio.duration)
    }

    fn visible_span(&self) -> f32 {
        (1.0 / self.zoom).min(1.0)
    }

    fn set_zoom(&mut self, zoom: f32) {
        let center = selection_range(self.selection)
            .map(|(start, end)| (start + end) * 0.5)
            .unwrap_or(self.playhead)
            .clamp(0.0, 1.0);
        self.zoom = zoom.clamp(1.0, 32.0);
        if self.recorder.is_some() && self.duration() > 0.0 {
            self.recording_view_seconds = self.duration() * self.visible_span();
        }
        let span = self.visible_span();
        self.view_start = (center - span * 0.5).clamp(0.0, 1.0 - span);
    }

    fn scroll_timeline(&mut self, delta: f32) {
        let span = self.visible_span();
        self.view_start = (self.view_start + delta).clamp(0.0, 1.0 - span);
    }

    fn pan_timeline(&mut self, direction: f32) {
        self.scroll_timeline(direction * self.visible_span() * 0.1);
    }

    fn toggle_playback(&mut self) {
        if self.playback.as_ref().is_some_and(Playback::is_playing) {
            if let Some(playback) = &self.playback {
                playback.pause();
            }
            self.playing = false;
            self.notice = "Playback paused".to_owned();
            return;
        }

        if self.playback.is_none() {
            let Some(audio) = self.audio() else {
                self.notice = "Import an audio file before playback".to_owned();
                return;
            };
            match Playback::new(
                audio.samples.clone(),
                audio.sample_rate,
                audio.channel_count(),
                self.envelopes_for_active_track(),
            ) {
                Ok(playback) => {
                    playback.seek_frame(
                        (self.playhead.clamp(0.0, 1.0) * playback.source_frames() as f32) as u64,
                    );
                    self.playback = Some(playback);
                }
                Err(error) => {
                    self.notice = format!("Playback error: {error}");
                    return;
                }
            }
        }

        if let Some(playback) = &self.playback {
            let frame_count = playback.source_frames();
            if let Some((start, end)) = selection_frame_range(self.selection, frame_count) {
                self.playhead = start as f32 / frame_count.max(1) as f32;
                playback.seek_frame(start);
                playback.set_playback_end_frame(end);
            } else {
                playback.set_playback_end_frame(frame_count);
                playback.seek_frame((self.playhead.clamp(0.0, 1.0) * frame_count as f32) as u64);
            }
            match playback.play() {
                Ok(()) => {
                    self.playing = true;
                    self.notice = format!("Playing at {} Hz", playback.output_rate());
                }
                Err(error) => {
                    self.playing = false;
                    self.notice = format!("Playback error: {error}");
                }
            }
        }
    }

    fn stop_playback(&mut self) {
        if let Some(playback) = &self.playback {
            playback.stop();
        }
        self.playing = false;
        self.playhead = 0.0;
        self.notice = "Playback stopped".to_owned();
    }

    fn normalize(&mut self) {
        let Some(audio) = self.audio() else {
            self.notice = "Import an audio file before normalizing".to_owned();
            return;
        };
        let channels = audio.channel_count();
        let samples = if self.stereo_mode {
            audio.samples.to_vec()
        } else {
            interleave_channel(&audio.samples, channels, self.active_channel)
        };
        let process_channels = if self.stereo_mode { channels } else { 1 };
        let mut envelope = self.gain_envelope.clone();
        match normalize_region(
            &samples,
            process_channels,
            self.selection,
            -1.0,
            &mut envelope,
        ) {
            Ok(gain_delta_db) => {
                self.save_edit();
                self.gain_envelope = envelope;
                self.save_active_channel_state();
                let midpoint = (self.selection[0] + self.selection[1]) * 0.5;
                self.gain_db = self.gain_envelope.gain_at(midpoint);
                if let Some(playback) = &self.playback {
                    playback.set_gain_envelopes(self.envelopes_for_active_track());
                }
                self.notice = format!("Normalized to −1 dBFS ({gain_delta_db:+.1} dB)");
            }
            Err(error) => self.notice = error.to_owned(),
        }
    }

    fn process_selection(
        &mut self,
        label: &str,
        processor: impl FnOnce(&mut [f32], u32, usize, [f32; 2]) -> Result<(), &'static str>,
    ) {
        let Some(audio) = self.audio() else {
            self.notice = format!("Import an audio file before applying {label}");
            return;
        };
        let sample_rate = audio.sample_rate;
        let channels = audio.channel_count();
        let is_stereo = self.stereo_mode;
        let active_channel = self.active_channel;
        let mut samples = if is_stereo {
            audio.samples.to_vec()
        } else {
            interleave_channel(&audio.samples, channels, active_channel)
        };
        let process_channels = if is_stereo { channels } else { 1 };
        if let Err(error) = processor(&mut samples, sample_rate, process_channels, self.selection) {
            self.notice = error.to_owned();
            return;
        }
        self.save_edit();
        let updated_samples = if let Some(audio) = self.audio_mut() {
            if is_stereo {
                audio.samples = samples.into();
            } else {
                let mut interleaved = audio.samples.to_vec();
                write_channel(&mut interleaved, channels, active_channel, &samples);
                audio.samples = interleaved.into();
            }
            audio.rebuild_peaks();
            Some(Arc::clone(&audio.samples))
        } else {
            None
        };
        if let (Some(playback), Some(samples)) = (&self.playback, updated_samples) {
            playback.set_samples(samples);
        }
        self.notice = format!("{label} applied to selected region");
    }

    fn apply_eq(&mut self) {
        let equalizer = self.equalizer;
        self.process_selection("EQ", move |samples, sample_rate, channels, selection| {
            apply_equalizer(samples, sample_rate, channels, selection, equalizer)
        });
    }

    fn apply_compressor(&mut self) {
        let compressor = self.compressor;
        self.process_selection("Compression", move |samples, _, channels, selection| {
            apply_compressor(samples, channels, selection, compressor)
        });
    }

    fn apply_fade_in(&mut self) {
        self.process_selection("Fade in", |samples, _, channels, selection| {
            apply_fade_in(samples, channels, selection)
        });
    }

    fn apply_fade_out(&mut self) {
        self.process_selection("Fade out", |samples, _, channels, selection| {
            apply_fade_out(samples, channels, selection)
        });
    }

    fn apply_denoise(&mut self) {
        self.process_selection("Denoise", |samples, _, channels, selection| {
            apply_denoise(samples, channels, selection, -45.0, 18.0)
        });
    }

    fn apply_gate(&mut self) {
        self.process_selection("Noise gate", |samples, _, channels, selection| {
            apply_gate(samples, channels, selection, -42.0)
        });
    }

    fn apply_limiter(&mut self) {
        self.process_selection("Limiter", |samples, _, channels, selection| {
            apply_limiter(samples, channels, selection, -1.0)
        });
    }

    fn apply_reverb(&mut self) {
        self.process_selection("Reverb", |samples, sample_rate, channels, selection| {
            apply_reverb(samples, sample_rate, channels, selection)
        });
    }

    fn toggle_recording(&mut self) {
        if let Some(mut recorder) = self.recorder.take() {
            recorder.drain_samples(&mut self.recording_samples);
            match recorder.stop(std::mem::take(&mut self.recording_samples)) {
                Ok(audio) => {
                    if let Some(track_index) = self.recording_track.take() {
                        self.tracks[track_index] = audio;
                    }
                    self.recording_previous_track = None;
                    self.active_channel = 0;
                    self.stereo_mode = true;
                    self.channel_selections = [[0.0, 0.0], [0.0, 0.0]];
                    self.playback = None;
                    self.playing = false;
                    self.gain_envelope = GainEnvelope::default();
                    self.gain_db = 0.0;
                    self.selection = [0.0, 0.0];
                    self.playhead = 1.0;
                    self.undo.clear();
                    self.redo.clear();
                    self.notice = "Recording added as a new track".to_owned();
                }
                Err(error) => {
                    if let Some(track_index) = self.recording_track.take() {
                        self.tracks.remove(track_index);
                        self.track_envelopes.remove(track_index);
                    }
                    self.selected_track = self
                        .recording_previous_track
                        .take()
                        .filter(|&index| index < self.tracks.len())
                        .unwrap_or(0);
                    self.notice = format!("Recording failed: {error}");
                }
            }
            self.recording_bucket_frames = 0;
            return;
        }
        if self.import_receiver.is_some() {
            self.notice = "Wait for audio import to finish before recording".to_owned();
            return;
        }
        if let Some(playback) = &self.playback {
            playback.pause();
        }
        self.playing = false;
        match Recorder::start() {
            Ok(recorder) => {
                let channels = recorder.channels();
                let track_index = self.tracks.len();
                let previous_track = self.selected_track;
                self.tracks.push(AudioData {
                    name: "Recording".to_owned(),
                    sample_rate: recorder.sample_rate(),
                    peaks: vec![Vec::new(); channels],
                    samples: Arc::from([]),
                    duration: 0.0,
                });
                self.track_envelopes
                    .push((0..channels).map(|_| GainEnvelope::default()).collect());
                self.recording_track = Some(track_index);
                self.recording_previous_track = Some(previous_track);
                self.recording_samples.clear();
                self.recording_bucket_frames = 0;
                self.recording_view_seconds = 10.0 / self.zoom;
                self.selected_track = track_index;
                self.active_channel = 0;
                self.stereo_mode = true;
                self.channel_selections = [[0.0, 0.0], [0.0, 0.0]];
                self.gain_envelope = GainEnvelope::default();
                self.gain_db = 0.0;
                self.selection = [0.0, 0.0];
                self.playhead = 0.0;
                self.view_start = 0.0;
                self.recorder = Some(recorder);
                self.notice = "Recording input audio...".to_owned();
            }
            Err(error) => self.notice = format!("Recording error: {error}"),
        }
    }

    fn poll_recording(&mut self) {
        let mut incoming = Vec::new();
        if let Some(recorder) = &mut self.recorder {
            recorder.drain_samples(&mut incoming);
        }
        if incoming.is_empty() {
            return;
        }
        self.recording_samples.extend_from_slice(&incoming);
        if let Some(track_index) = self.recording_track
            && let Some(audio) = self.tracks.get_mut(track_index)
        {
            audio.append_peak_samples(&incoming, &mut self.recording_bucket_frames);
            audio.duration = self.recording_samples.len() as f32
                / audio.channel_count() as f32
                / audio.sample_rate as f32;
        }
        self.zoom = (self.duration() / self.recording_view_seconds.max(0.01)).clamp(1.0, 32.0);
        let span = self.visible_span();
        self.playhead = 1.0;
        self.view_start = (1.0 - span * 0.88).max(0.0);
    }

    fn export_audio(&mut self) {
        if self.export_receiver.is_some() {
            self.notice = "Audio export is already in progress".to_owned();
            return;
        }
        let Some(audio) = self.audio() else {
            self.notice = "Import an audio file before exporting".to_owned();
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("WAV audio", &["wav"])
            .set_file_name(format!(
                "{}.wav",
                audio
                    .name
                    .rsplit_once('.')
                    .map_or(audio.name.as_str(), |(stem, _)| stem)
            ))
            .save_file()
        else {
            return;
        };

        let samples = audio.samples.clone();
        let sample_rate = audio.sample_rate;
        let channels = audio.channel_count();
        let envelopes = self.envelopes_for_active_track();
        let (sender, receiver) = mpsc::channel();
        self.export_receiver = Some(receiver);
        self.notice = "Exporting processed WAV…".to_owned();
        if let Err(error) = std::thread::Builder::new()
            .name("rina-audio-export".to_owned())
            .spawn(move || {
                let result = export_wav(&path, &samples, sample_rate, channels, &envelopes);
                let _ = sender.send(result);
            })
        {
            self.export_receiver = None;
            self.notice = format!("Could not start WAV export: {error}");
        }
    }

    fn poll_playback(&mut self) {
        if let Some(playback) = &self.playback {
            self.playing = playback.is_playing();
            self.playhead =
                playback.position_frames() as f32 / playback.source_frames().max(1) as f32;
            if let Some(error) = playback.take_device_error() {
                self.playing = false;
                self.notice = error;
            }
        }
    }

    fn poll_export(&mut self) {
        let result = match self.export_receiver.as_ref().map(Receiver::try_recv) {
            Some(Ok(result)) => Some(result),
            Some(Err(TryRecvError::Disconnected)) => {
                Some(Err("audio exporter stopped unexpectedly".to_owned()))
            }
            Some(Err(TryRecvError::Empty)) | None => None,
        };
        if let Some(result) = result {
            self.export_receiver = None;
            self.notice = match result {
                Ok(()) => "Processed WAV exported".to_owned(),
                Err(error) => format!("Export failed: {error}"),
            };
        }
    }

    fn open_audio(&mut self) {
        if self.recorder.is_some() {
            self.notice = "Stop recording before opening another audio file".to_owned();
            return;
        }
        if self.import_receiver.is_some() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Audio files",
                &["wav", "flac", "mp3", "ogg", "m4a", "aac", "aiff", "aif"],
            )
            .pick_file()
        else {
            return;
        };

        self.notice = "Importing audio…".to_owned();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let (sender, receiver) = mpsc::channel();
        self.import_receiver = Some(receiver);
        if let Err(error) = std::thread::Builder::new()
            .name("rina-audio-import".to_owned())
            .spawn(move || {
                let result = decode_audio(&path).map(|mut audio| {
                    audio.name = name;
                    audio
                });
                let _ = sender.send(result);
            })
        {
            self.import_receiver = None;
            self.notice = format!("Could not start audio import: {error}");
        }
    }

    fn poll_import(&mut self) {
        let result = match self.import_receiver.as_ref().map(Receiver::try_recv) {
            Some(Ok(result)) => Some(result),
            Some(Err(TryRecvError::Disconnected)) => {
                Some(Err("audio importer stopped unexpectedly".to_owned()))
            }
            Some(Err(TryRecvError::Empty)) | None => None,
        };
        if let Some(result) = result {
            self.import_receiver = None;
            match result {
                Ok(audio) => {
                    self.tracks.clear();
                    self.tracks.push(audio);
                    self.track_envelopes.clear();
                    let channel_count = self.tracks[0].channel_count();
                    self.track_envelopes.push(
                        (0..channel_count)
                            .map(|_| GainEnvelope::default())
                            .collect(),
                    );
                    self.selected_track = 0;
                    self.active_channel = 0;
                    self.stereo_mode = true;
                    self.playback = None;
                    self.selection = [0.0, 0.0];
                    self.channel_selections = [[0.0, 0.0], [0.0, 0.0]];
                    self.gain_db = 0.0;
                    self.gain_envelope = GainEnvelope::default();
                    self.playhead = 0.0;
                    self.zoom = 1.0;
                    self.view_start = 0.0;
                    self.undo.clear();
                    self.redo.clear();
                    self.playing = false;
                    self.recorder = None;
                    self.notice = "Audio imported".to_owned();
                }
                Err(error) => self.notice = format!("Import failed: {error}"),
            }
        }
    }

    fn update_shortcuts(&mut self, context: &egui::Context) {
        if context.wants_keyboard_input() && !self.waveform_focused {
            return;
        }
        context.input(|input| {
            if input.modifiers.command && input.key_pressed(egui::Key::O) {
                self.open_audio();
            } else if input.modifiers.command && input.key_pressed(egui::Key::Z) {
                self.undo();
            } else if input.modifiers.command && input.key_pressed(egui::Key::Y) {
                self.redo();
            } else if self.waveform_focused
                && input.modifiers.command
                && input.key_pressed(egui::Key::C)
            {
                self.copy_selection();
            } else if self.waveform_focused
                && input.modifiers.command
                && input.key_pressed(egui::Key::X)
            {
                self.cut_selection();
            } else if self.waveform_focused
                && input.modifiers.command
                && input.key_pressed(egui::Key::V)
            {
                self.paste_selection();
            } else if self.waveform_focused
                && input.modifiers.command
                && input.key_pressed(egui::Key::A)
            {
                self.select_all();
            } else if self.waveform_focused
                && (input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace))
            {
                self.delete_selection();
            } else if self.waveform_focused && input.key_pressed(egui::Key::Space) {
                self.toggle_playback();
            } else if self.waveform_focused && input.key_pressed(egui::Key::W) {
                self.set_zoom(self.zoom * 1.2);
            } else if self.waveform_focused && input.key_pressed(egui::Key::S) {
                self.set_zoom(self.zoom / 1.2);
            } else if self.waveform_focused && input.key_pressed(egui::Key::A) {
                self.pan_timeline(-1.0);
            } else if self.waveform_focused && input.key_pressed(egui::Key::D) {
                self.pan_timeline(1.0);
            } else if self.waveform_focused
                && (input.key_pressed(egui::Key::Plus) || input.key_pressed(egui::Key::Equals))
            {
                self.set_zoom(self.zoom * 1.2);
            } else if self.waveform_focused && input.key_pressed(egui::Key::Minus) {
                self.set_zoom(self.zoom / 1.2);
            }
        });
    }

    fn top_bar(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::top("top_bar")
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(20, 10)),
            )
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    icon(ui, Icon::Brand, 23.0, ACCENT);
                    ui.label(RichText::new("rina").size(20.0).strong().color(TEXT));
                    ui.label(
                        RichText::new("AUDIO STUDIO")
                            .size(10.0)
                            .strong()
                            .color(MUTED),
                    );
                    ui.add_space(28.0);
                    ui.separator();
                    ui.add_space(12.0);
                    ui.label(RichText::new("SESSION").size(9.0).strong().color(MUTED));
                    ui.label(
                        RichText::new("Field recording — take 04")
                            .size(13.0)
                            .color(TEXT),
                    );
                    icon(ui, Icon::Record, 6.0, ACCENT);
                    ui.label(RichText::new("Saved").size(11.0).color(MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if pill_button(ui, "Export", true).clicked() {
                            self.export_audio();
                        }
                        ui.add_space(8.0);
                        if pill_button(ui, "Open audio", false).clicked() {
                            self.open_audio();
                        }
                        ui.add_space(12.0);
                        icon(ui, Icon::More, 22.0, MUTED);
                    });
                });
            });
    }

    fn toolbar(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::top("toolbar")
            .frame(
                Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(20, 10)),
            )
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    for (tool, tool_icon) in [
                        ("Select", Icon::Select),
                        ("Trim", Icon::Trim),
                        ("Split", Icon::Split),
                        ("Draw", Icon::Draw),
                    ] {
                        let active = self.active_tool == tool;
                        let response = tool_button(ui, tool_icon, tool, active);
                        if response.clicked() {
                            self.active_tool = tool;
                            self.notice = format!("{tool} tool selected");
                        }
                    }
                    ui.add_space(14.0);
                    ui.separator();
                    ui.add_space(7.0);
                    let undo = ui
                        .add_enabled_ui(!self.undo.is_empty(), |ui| {
                            icon_button(ui, Icon::Undo, 30.0, BG)
                        })
                        .inner;
                    if undo.clicked() {
                        self.undo();
                    }
                    let redo = ui
                        .add_enabled_ui(!self.redo.is_empty(), |ui| {
                            icon_button(ui, Icon::Redo, 30.0, BG)
                        })
                        .inner;
                    if redo.clicked() {
                        self.redo();
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, Icon::ZoomIn, 25.0, BG)
                            .on_hover_text("Zoom in")
                            .clicked()
                        {
                            self.set_zoom(self.zoom * 1.2);
                        }
                        if icon_button(ui, Icon::ZoomOut, 25.0, BG)
                            .on_hover_text("Zoom out")
                            .clicked()
                        {
                            self.set_zoom(self.zoom / 1.2);
                        }
                        ui.label(
                            RichText::new(format!("{:.0}%", self.zoom * 100.0))
                                .size(11.0)
                                .monospace()
                                .color(TEXT),
                        );
                        ui.add_space(14.0);
                        ui.label(RichText::new("ZOOM").size(9.0).strong().color(MUTED));
                    });
                });
            });
    }

    fn side_panel(&mut self, context: &egui::Context) {
        egui::SidePanel::right("inspector")
            .resizable(true)
            .default_width(260.0)
            .width_range(220.0..=340.0)
            .frame(Frame::new().fill(PANEL).inner_margin(Margin::same(16)))
            .show(context, |ui| {
                ui.label(RichText::new("QUICK EDIT").size(10.0).strong().color(MUTED));
                ui.add_space(7.0);
                ui.label(
                    RichText::new("Selected region")
                        .size(16.0)
                        .strong()
                        .color(TEXT),
                );
                ui.add_space(12.0);
                Frame::new()
                    .fill(PANEL_ALT)
                    .corner_radius(9.0)
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.label(RichText::new("LENGTH").size(9.0).strong().color(MUTED));
                        ui.add_space(3.0);
                        ui.label(
                            RichText::new(format!(
                                "{:.2} s",
                                (self.selection[1] - self.selection[0]).abs() * self.duration()
                            ))
                            .size(17.0)
                            .monospace()
                            .color(TEXT),
                        );
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("GAIN").size(9.0).strong().color(MUTED));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format!("{:+.1} dB", self.gain_db))
                                        .size(12.0)
                                        .monospace()
                                        .strong()
                                        .color(ACCENT),
                                );
                            });
                        });
                        let previous_gain = self.gain_db;
                        let slider = egui::Slider::new(&mut self.gain_db, -24.0..=18.0)
                            .show_value(false)
                            .step_by(0.1);
                        let slider_response = ui.add(slider);
                        if slider_response.drag_started() {
                            self.save_edit();
                        }
                        if slider_response.changed() {
                            if !slider_response.dragged() {
                                self.save_edit_with_gain(previous_gain);
                            }
                            self.gain_envelope.set_region(
                                self.selection[0],
                                self.selection[1],
                                self.gain_db,
                            );
                            self.save_active_channel_state();
                            if let Some(playback) = &self.playback {
                                playback.set_gain_envelopes(self.envelopes_for_active_track());
                            }
                            self.notice = "Region gain adjusted".to_owned();
                        }
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("−24").size(9.0).color(MUTED));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(RichText::new("+18 dB").size(9.0).color(MUTED));
                            });
                        });
                    });
                ui.add_space(18.0);
                ui.label(RichText::new("ONE-CLICK").size(10.0).strong().color(MUTED));
                ui.add_space(8.0);
                if quick_action(
                    ui,
                    "Normalize",
                    "Set peak to −1 dB",
                    "Normalize selected audio to −1 dBFS",
                    &mut self.notice,
                ) {
                    self.normalize();
                }
                if quick_action(
                    ui,
                    "Fade in",
                    "Fade selected region from silence",
                    "Fade in applied to selected region",
                    &mut self.notice,
                ) {
                    self.apply_fade_in();
                }
                if quick_action(
                    ui,
                    "Fade out",
                    "Fade selected region to silence",
                    "Fade out applied to selected region",
                    &mut self.notice,
                ) {
                    self.apply_fade_out();
                }
                ui.add_space(10.0);
                ui.label(RichText::new("PROCESS").size(10.0).strong().color(MUTED));
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("EQ").size(10.0).color(TEXT))
                                .fill(if self.effect_panel == Some(EffectPanel::Equalizer) {
                                    ACCENT
                                } else {
                                    PANEL_ALT
                                })
                                .stroke(Stroke::new(1.0_f32, BORDER))
                                .corner_radius(6.0),
                        )
                        .clicked()
                    {
                        self.effect_panel = Some(EffectPanel::Equalizer);
                    }
                    if ui
                        .add(
                            egui::Button::new(RichText::new("Compress").size(10.0).color(TEXT))
                                .fill(if self.effect_panel == Some(EffectPanel::Compressor) {
                                    ACCENT
                                } else {
                                    PANEL_ALT
                                })
                                .stroke(Stroke::new(1.0_f32, BORDER))
                                .corner_radius(6.0),
                        )
                        .clicked()
                    {
                        self.effect_panel = Some(EffectPanel::Compressor);
                    }
                    for label in ["Denoise", "Gate", "Limit", "Reverb"] {
                        if ui
                            .add(
                                egui::Button::new(RichText::new(label).size(10.0).color(TEXT))
                                    .fill(PANEL_ALT)
                                    .stroke(Stroke::new(1.0_f32, BORDER))
                                    .corner_radius(6.0),
                            )
                            .clicked()
                        {
                            match label {
                                "Denoise" => self.apply_denoise(),
                                "Gate" => self.apply_gate(),
                                "Limit" => self.apply_limiter(),
                                "Reverb" => self.apply_reverb(),
                                _ => {}
                            }
                        }
                    }
                });
                if let Some(effect) = self.effect_panel {
                    ui.add_space(8.0);
                    Frame::new()
                        .fill(PANEL_ALT)
                        .stroke(Stroke::new(1.0_f32, BORDER))
                        .corner_radius(8.0)
                        .inner_margin(Margin::same(10))
                        .show(ui, |ui| match effect {
                            EffectPanel::Equalizer => {
                                ui.label(
                                    RichText::new("3-BAND EQ").size(9.0).strong().color(ACCENT),
                                );
                                ui.add(
                                    egui::Slider::new(&mut self.equalizer.low_db, -12.0..=12.0)
                                        .text("Low")
                                        .suffix(" dB"),
                                );
                                ui.add(
                                    egui::Slider::new(&mut self.equalizer.mid_db, -12.0..=12.0)
                                        .text("Mid")
                                        .suffix(" dB"),
                                );
                                ui.add(
                                    egui::Slider::new(&mut self.equalizer.high_db, -12.0..=12.0)
                                        .text("High")
                                        .suffix(" dB"),
                                );
                                if ui.button("Apply EQ to selection").clicked() {
                                    self.apply_eq();
                                }
                            }
                            EffectPanel::Compressor => {
                                ui.label(
                                    RichText::new("COMPRESSOR").size(9.0).strong().color(ACCENT),
                                );
                                ui.add(
                                    egui::Slider::new(
                                        &mut self.compressor.threshold_db,
                                        -48.0..=0.0,
                                    )
                                    .text("Threshold")
                                    .suffix(" dB"),
                                );
                                ui.add(
                                    egui::Slider::new(&mut self.compressor.ratio, 1.0..=12.0)
                                        .text("Ratio")
                                        .suffix(":1"),
                                );
                                if ui.button("Compress selection").clicked() {
                                    self.apply_compressor();
                                }
                            }
                        });
                }
                ui.add_space(18.0);
                ui.label(RichText::new("SELECTION").size(10.0).strong().color(MUTED));
                ui.add_space(8.0);
                time_edit(
                    ui,
                    "Start",
                    &format_time(self.selection[0].min(self.selection[1]) * self.duration()),
                );
                time_edit(
                    ui,
                    "End",
                    &format_time(self.selection[0].max(self.selection[1]) * self.duration()),
                );
                ui.add_space(8.0);
                ui.checkbox(
                    &mut self.snap_enabled,
                    RichText::new("Snap to zero crossing")
                        .size(11.0)
                        .color(MUTED),
                );
            });
    }

    fn bottom_bar(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::bottom("transport")
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(20, 11)),
            )
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format_time(self.playhead * self.duration()))
                            .size(13.0)
                            .monospace()
                            .strong()
                            .color(TEXT),
                    );
                    ui.add_space(10.0);
                    if transport_button(ui, Icon::Previous).clicked() {
                        if let Some(playback) = &self.playback {
                            playback.seek_frame(0);
                        }
                    }
                    if icon_button(
                        ui,
                        if self.playing {
                            Icon::Pause
                        } else {
                            Icon::Play
                        },
                        34.0,
                        ACCENT,
                    )
                    .clicked()
                    {
                        self.toggle_playback();
                    }
                    if transport_button(ui, Icon::Stop).clicked() {
                        self.stop_playback();
                    }
                    if transport_button(ui, Icon::Next).clicked() {
                        if let Some(playback) = &self.playback {
                            playback.seek_frame(playback.source_frames());
                        }
                    }
                    ui.add_space(13.0);
                    if icon_button(
                        ui,
                        if self.recorder.is_some() {
                            Icon::Stop
                        } else {
                            Icon::Record
                        },
                        30.0,
                        if self.recorder.is_some() {
                            RECORDING_RED
                        } else {
                            PANEL
                        },
                    )
                    .on_hover_text(if self.recorder.is_some() {
                        "Stop recording"
                    } else {
                        "Start recording"
                    })
                    .clicked()
                    {
                        self.toggle_recording();
                    }
                    ui.label(
                        RichText::new(if self.recorder.is_some() {
                            "RECORDING"
                        } else {
                            "REC"
                        })
                        .size(10.0)
                        .strong()
                        .color(if self.recorder.is_some() {
                            RECORDING_RED
                        } else {
                            MUTED
                        }),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let audio_info = self.audio().map_or_else(
                            || "No audio loaded".to_owned(),
                            |audio| {
                                format!(
                                    "{:.1} kHz  ·  {} ch",
                                    audio.sample_rate as f32 / 1000.0,
                                    audio.channel_count()
                                )
                            },
                        );
                        ui.label(RichText::new(audio_info).size(10.0).color(MUTED));
                        ui.add_space(18.0);
                        ui.label(RichText::new(&self.notice).size(10.0).color(ACCENT));
                        ui.add_space(16.0);
                        ui.label(RichText::new("OUTPUT").size(9.0).strong().color(MUTED));
                        ui.add(
                            egui::Slider::new(&mut 0.78_f32, 0.0..=1.0)
                                .show_value(false)
                                .fixed_decimals(2),
                        );
                    });
                });
            });
    }

    fn editor(&mut self, context: &egui::Context) {
        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin::symmetric(20, 14)))
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    icon(ui, Icon::Brand, 12.0, ACCENT);
                    if let Some(audio) = self.audio() {
                        ui.label(
                            RichText::new(&audio.name)
                                .size(13.0)
                                .strong()
                                .color(TEXT),
                        );
                        ui.label(
                            RichText::new(channel_label(audio.channel_count()))
                                .size(9.0)
                                .strong()
                                .color(MUTED),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{:.1} kHz", audio.sample_rate as f32 / 1000.0))
                                    .size(10.0)
                                    .color(MUTED),
                            );
                            ui.add_space(12.0);
                            ui.label(
                                RichText::new(format_time(audio.duration))
                                    .size(11.0)
                                    .monospace()
                                    .color(TEXT),
                            );
                        });
                    } else {
                        ui.label(
                            RichText::new("Demo waveform — import audio to edit")
                                .size(13.0)
                                .strong()
                                .color(TEXT),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new("00:42").size(11.0).monospace().color(TEXT));
                        });
                    }
                });
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.selectable_label(self.stereo_mode, "Stereo").clicked() {
                        self.select_stereo();
                    }
                    if ui
                        .selectable_label(!self.stereo_mode && self.active_channel == 0, "L")
                        .clicked()
                    {
                        self.select_channel(0);
                    }
                    if ui
                        .selectable_label(!self.stereo_mode && self.active_channel == 1, "R")
                        .clicked()
                    {
                        self.select_channel(1);
                    }
                    if ui.button("L → R").clicked() {
                        self.copy_channel(0, 1);
                    }
                    if ui.button("R → L").clicked() {
                        self.copy_channel(1, 0);
                    }
                    if ui.button("Copy").clicked() {
                        self.copy_selection();
                    }
                    if ui.button("Cut").clicked() {
                        self.cut_selection();
                    }
                    if ui.button("Paste").clicked() {
                        self.paste_selection();
                    }
                    if ui.button("Silence").clicked() {
                        self.silence_selection();
                    }
                    if ui.button("Delete").clicked() {
                        self.delete_selection();
                    }
                });
                ui.add_space(6.0);
                Frame::new().fill(PANEL).stroke(Stroke::new(1.0_f32, BORDER)).corner_radius(10.0).show(ui, |ui| {
                    ui.set_min_height(525.0);
                    ui.set_width(ui.available_width());
                    let width = ui.available_width();
                    let timeline_height = 35.0;
                    let waveform_height = (ui.available_height() - timeline_height - 50.0).max(260.0);
                    let (timeline_rect, _) = ui.allocate_exact_size(Vec2::new(width, timeline_height), Sense::hover());
                    let visible_span = self.visible_span();
                    draw_timeline(
                        ui,
                        timeline_rect,
                        self.duration(),
                        self.view_start,
                        visible_span,
                    );
                    ui.add_space(4.0);

                    let (wave_rect, response) = ui.allocate_exact_size(
                        Vec2::new(width, waveform_height),
                        Sense::click_and_drag(),
                    );
                    if response.hovered() {
                        let scroll = context.input(|input| {
                            let scroll = input.smooth_scroll_delta;
                            if input.modifiers.shift && scroll.y != 0.0 {
                                scroll.y
                            } else if scroll.x != 0.0 {
                                scroll.x
                            } else {
                                0.0
                            }
                        });
                        if scroll != 0.0 {
                            self.scroll_timeline(-scroll / wave_rect.width() * visible_span);
                        }
                    }
                    let pointer = response.interact_pointer_pos();
                    if response.drag_started() {
                        response.request_focus();
                        if let Some(pos) = pointer {
                            if !self.stereo_mode {
                                let channel = usize::from(pos.y >= wave_rect.center().y);
                                self.select_channel(channel);
                            }
                            let gain_lane = if self.stereo_mode {
                                usize::from(pos.y >= wave_rect.center().y)
                            } else {
                                self.active_channel
                            };
                            self.gain_drag_lane = gain_lane;
                            let gain_lane_center = wave_rect.top()
                                + wave_rect.height() * (gain_lane as f32 + 0.5) * 0.5;
                            let selection_center = (self.selection[0] + self.selection[1]) * 0.5;
                            let handle_x = x_from_time(
                                selection_center,
                                wave_rect,
                                self.view_start,
                                visible_span,
                            );
                            self.gain_dragging = (pos.x - handle_x).abs() < 24.0
                                && (pos.y - gain_handle_y(self.gain_db, gain_lane_center)).abs()
                                    < 25.0;
                            if !self.gain_dragging {
                                let t = time_from_x(pos.x, wave_rect, self.view_start, visible_span);
                                self.selection = [t, t];
                                self.channel_selections[self.active_channel] = self.selection;
                                self.gain_db = self.gain_envelope.gain_at(t);
                            }
                        }
                        if self.gain_dragging {
                            self.save_edit();
                        }
                    }
                    if response.dragged() {
                        if let Some(pos) = pointer {
                            if self.gain_dragging {
                                let lane_center = wave_rect.top()
                                    + wave_rect.height()
                                        * (self.gain_drag_lane as f32 + 0.5)
                                        * 0.5;
                                let delta = gain_delta_from_pointer(lane_center, pos.y);
                                self.gain_db = delta.clamp(-24.0, 18.0);
                                self.gain_envelope.set_region(
                                    self.selection[0],
                                    self.selection[1],
                                    self.gain_db,
                                );
                                self.save_active_channel_state();
                                if let Some(playback) = &self.playback {
                                playback.set_gain_envelopes(
                                    self.envelopes_for_active_track(),
                                );
                                }
                                self.notice = format!("Gain {:+.1} dB", self.gain_db);
                            } else {
                                let t = time_from_x(pos.x, wave_rect, self.view_start, visible_span);
                                self.selection[1] = t;
                                self.channel_selections[self.active_channel] = self.selection;
                                if self.stereo_mode {
                                    self.channel_selections = [self.selection; 2];
                                }
                                self.gain_db = self
                                    .gain_envelope
                                    .gain_at((self.selection[0] + self.selection[1]) * 0.5);
                            }
                        }
                    }
                    if response.drag_stopped() {
                        self.gain_dragging = false;
                    }
                    if response.clicked() {
                        response.request_focus();
                        if let Some(pos) = pointer {
                            if !self.stereo_mode {
                                self.select_channel(usize::from(pos.y >= wave_rect.center().y));
                            }
                            self.playhead =
                                time_from_x(pos.x, wave_rect, self.view_start, visible_span);
                            if let Some(playback) = &self.playback {
                                playback.seek_frame(
                                    (self.playhead * playback.source_frames() as f32) as u64,
                                );
                            }
                        }
                    }
                    self.waveform_focused = response.has_focus();
                    let channel_envelopes = self.envelopes_for_active_track();
                    let display_envelopes = std::array::from_fn(|channel| {
                        channel_envelopes
                            .get(channel)
                            .cloned()
                            .unwrap_or_default()
                    });
                    draw_waveform(
                        ui,
                        wave_rect,
                        self.channel_selections,
                        self.active_channel,
                        self.stereo_mode,
                        self.gain_db,
                        self.playhead,
                        self.playing,
                        self.audio(),
                        &display_envelopes,
                        self.view_start,
                        visible_span,
                    );
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("L").size(9.0).strong().color(MUTED));
                        ui.add_space(6.0);
                        ui.label(RichText::new("R").size(9.0).strong().color(MUTED));
                        ui.add_space(14.0);
                        ui.label(RichText::new("DRAG A REGION TO SELECT  ·  DRAG THE CENTER HANDLE TO ADJUST GAIN").size(9.0).color(MUTED));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new("−∞").size(9.0).color(MUTED));
                            ui.label(RichText::new("0 dB").size(9.0).color(MUTED));
                            ui.label(RichText::new("PEAK").size(8.0).strong().color(MUTED));
                        });
                    });
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("TRACKS").size(9.0).strong().color(MUTED));
                    ui.add_space(6.0);
                    ui.label(RichText::new("1").size(10.0).strong().color(ACCENT));
                    ui.label(
                        RichText::new(
                            self.audio()
                                .map_or("Demo waveform", |audio| audio.name.as_str()),
                        )
                        .size(10.0)
                        .color(MUTED),
                    );
                    let track_names: Vec<_> =
                        self.tracks.iter().map(|track| track.name.clone()).collect();
                    for (index, name) in track_names.iter().enumerate() {
                        let selected = index == self.selected_track;
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(format!("{}  {name}", index + 1))
                                        .size(10.0)
                                        .color(if selected { ACCENT } else { MUTED }),
                                )
                                .fill(if selected { PANEL_ALT } else { PANEL })
                                .stroke(Stroke::new(
                                    1.0_f32,
                                    if selected { ACCENT } else { BORDER },
                                ))
                                .corner_radius(5.0),
                            )
                            .clicked()
                        {
                            self.select_track(index);
                        }
                    }
                });
            });
    }
}

impl eframe::App for RinaApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        context.set_visuals(egui::Visuals::dark());
        context.style_mut(|style| {
            style.spacing.item_spacing = Vec2::new(8.0, 7.0);
            style.visuals.widgets.inactive.bg_fill = PANEL_ALT;
            style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
            style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(40, 49, 61);
            style.visuals.widgets.active.bg_fill = Color32::from_rgb(51, 65, 77);
        });
        self.update_shortcuts(context);
        self.poll_import();
        self.poll_recording();
        self.poll_playback();
        self.poll_export();
        self.top_bar(context);
        self.toolbar(context);
        self.bottom_bar(context);
        self.side_panel(context);
        self.editor(context);
        if self.playing {
            context.request_repaint();
        }
        if self.recorder.is_some() {
            context.request_repaint_after(Duration::from_millis(16));
        } else if self.import_receiver.is_some() || self.export_receiver.is_some() {
            context.request_repaint_after(Duration::from_millis(100));
        }
    }
}

fn draw_timeline(ui: &egui::Ui, rect: Rect, duration: f32, view_start: f32, visible_span: f32) {
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, PANEL);
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0_f32, BORDER),
    );
    for index in 0..=8 {
        let x = rect.left() + rect.width() * index as f32 / 8.0;
        painter.line_segment(
            [
                Pos2::new(x, rect.bottom() - 8.0),
                Pos2::new(x, rect.bottom()),
            ],
            Stroke::new(1.0_f32, BORDER),
        );
        painter.text(
            Pos2::new(x + 4.0, rect.top() + 8.0),
            egui::Align2::LEFT_TOP,
            format_time((view_start + visible_span * index as f32 / 8.0) * duration),
            FontId::monospace(9.0),
            MUTED,
        );
    }
}

fn time_from_x(x: f32, rect: Rect, view_start: f32, visible_span: f32) -> f32 {
    (view_start + ((x - rect.left()) / rect.width()) * visible_span).clamp(0.0, 1.0)
}

fn x_from_time(time: f32, rect: Rect, view_start: f32, visible_span: f32) -> f32 {
    rect.left() + ((time - view_start) / visible_span) * rect.width()
}

fn selection_range(selection: [f32; 2]) -> Option<(f32, f32)> {
    let start = selection[0].min(selection[1]).clamp(0.0, 1.0);
    let end = selection[0].max(selection[1]).clamp(0.0, 1.0);
    (end > start).then_some((start, end))
}

fn selection_frame_range(selection: [f32; 2], frame_count: u64) -> Option<(u64, u64)> {
    let (start, end) = selection_range(selection)?;
    if frame_count == 0 {
        return None;
    }
    let start_frame = (start * frame_count as f32).floor() as u64;
    let end_frame = ((end * frame_count as f32).ceil() as u64).min(frame_count);
    (end_frame > start_frame).then_some((start_frame, end_frame))
}

fn nearest_zero_crossing(
    samples: &[f32],
    channels: usize,
    target_frame: usize,
    radius: usize,
) -> Option<usize> {
    if channels == 0 || samples.len() % channels != 0 {
        return None;
    }
    let frames = samples.len() / channels;
    if frames < 2 {
        return None;
    }
    if target_frame == 0 && (0..channels).any(|channel| samples[channel] == 0.0) {
        return Some(0);
    }
    if target_frame >= frames
        && (0..channels).any(|channel| samples[(frames - 1) * channels + channel] == 0.0)
    {
        return Some(frames);
    }
    let first = target_frame.saturating_sub(radius).max(1);
    let last = target_frame.saturating_add(radius).min(frames - 1);
    (first..=last)
        .filter_map(|frame| {
            let crosses_zero = (0..channels).any(|channel| {
                let previous = samples[(frame - 1) * channels + channel];
                let current = samples[frame * channels + channel];
                previous == 0.0
                    || current == 0.0
                    || previous.is_sign_negative() != current.is_sign_negative()
            });
            crosses_zero.then(|| {
                let amplitude = (0..channels)
                    .map(|channel| {
                        samples[(frame - 1) * channels + channel].abs()
                            + samples[frame * channels + channel].abs()
                    })
                    .sum::<f32>();
                (frame.abs_diff(target_frame), amplitude, frame)
            })
        })
        .min_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.total_cmp(&right.1))
        })
        .map(|(_, _, frame)| frame)
}

fn interleave_channel(samples: &[f32], channels: usize, channel: usize) -> Vec<f32> {
    if channels == 0 || channel >= channels {
        return Vec::new();
    }
    samples
        .chunks_exact(channels)
        .map(|frame| frame[channel])
        .collect()
}

fn write_channel(samples: &mut [f32], channels: usize, channel: usize, source: &[f32]) {
    if channels == 0 || channel >= channels {
        return;
    }
    for (frame, sample) in samples
        .chunks_exact_mut(channels)
        .zip(source.iter().copied())
    {
        frame[channel] = sample;
    }
}

fn draw_waveform(
    ui: &egui::Ui,
    rect: Rect,
    selections: [[f32; 2]; 2],
    active_channel: usize,
    stereo_mode: bool,
    gain_db: f32,
    playhead: f32,
    playing: bool,
    audio: Option<&AudioData>,
    gain_envelopes: &[GainEnvelope; 2],
    view_start: f32,
    visible_span: f32,
) {
    let painter = ui.painter();
    painter.rect_filled(rect, 5.0, Color32::from_rgb(17, 22, 29));
    let view_end = view_start + visible_span;
    let split_y = rect.center().y;
    painter.line_segment(
        [
            Pos2::new(rect.left(), split_y),
            Pos2::new(rect.right(), split_y),
        ],
        Stroke::new(1.0_f32, BORDER),
    );
    for lane in 0..2 {
        let lane_rect = Rect::from_min_max(
            Pos2::new(rect.left(), rect.top() + rect.height() * lane as f32 * 0.5),
            Pos2::new(
                rect.right(),
                rect.top() + rect.height() * (lane + 1) as f32 * 0.5,
            ),
        );
        let selection = selections[lane];
        let start = selection[0].min(selection[1]);
        let end = selection[0].max(selection[1]);
        let has_visible_selection = end > view_start && start < view_end;
        let visible_start = if has_visible_selection {
            start.max(view_start)
        } else {
            view_start
        };
        let visible_end = if has_visible_selection {
            end.min(view_end)
        } else {
            view_start
        };
        let selected_rect = Rect::from_min_max(
            Pos2::new(
                x_from_time(visible_start, rect, view_start, visible_span),
                lane_rect.top(),
            ),
            Pos2::new(
                x_from_time(visible_end, rect, view_start, visible_span),
                lane_rect.bottom(),
            ),
        );
        if visible_start < visible_end {
            painter.rect_filled(
                selected_rect,
                0.0,
                if stereo_mode || lane == active_channel {
                    Color32::from_rgba_unmultiplied(105, 219, 190, 48)
                } else {
                    Color32::from_rgba_unmultiplied(105, 219, 190, 18)
                },
            );
        }
        let center = rect.top() + rect.height() * (lane as f32 + 0.5) * 0.5;
        let amplitude = rect.height() * 0.225;
        let step = (rect.width() / 900.0).max(1.0);
        let mut x = rect.left();
        while x < rect.right() {
            let t = (view_start + (x - rect.left()) / rect.width() * visible_span).clamp(0.0, 1.0);
            let (minimum, maximum) = if let Some(audio) = audio {
                audio
                    .peaks
                    .get(lane.min(audio.channel_count().saturating_sub(1)))
                    .filter(|channel| !channel.is_empty())
                    .map_or((0.0, 0.0), |channel| {
                        let first = (t * channel.len() as f32).floor() as usize;
                        let last = (((t + step / rect.width() * visible_span)
                            * channel.len() as f32)
                            .ceil() as usize)
                            .max(first + 1)
                            .min(channel.len());
                        channel[first.min(channel.len() - 1)..last]
                            .iter()
                            .fold((0.0_f32, 0.0_f32), |(min, max), sample| {
                                (min.min(sample[0]), max.max(sample[1]))
                            })
                    })
            } else {
                let envelope = (0.25
                    + 0.75 * ((t * 5.2).sin().abs() * 0.65 + (t * 12.0 + 1.2).cos().abs() * 0.35))
                    * (0.72 + 0.22 * (t * 1.7).sin());
                let detail = ((t * 123.0 + lane as f32 * 0.7).sin() * 0.55
                    + (t * 47.0 + 0.4).cos() * 0.3
                    + (t * 211.0 + lane as f32).sin() * 0.15)
                    .abs();
                let half = envelope * (0.35 + detail * 0.65);
                (-half, half)
            };
            let amplitude = amplitude * db_to_linear(gain_envelopes[lane].gain_at(t));
            let color =
                if has_visible_selection && x >= selected_rect.left() && x <= selected_rect.right()
                {
                    Color32::from_rgb(129, 234, 205)
                } else {
                    WAVE
                };
            painter.line_segment(
                [
                    Pos2::new(x, center - maximum * amplitude),
                    Pos2::new(x, center - minimum * amplitude),
                ],
                Stroke::new(step.min(2.0).max(1.0), color),
            );
            x += step;
        }
    }

    let gain_lanes = if stereo_mode {
        0..2
    } else {
        active_channel.min(1)..active_channel.min(1) + 1
    };
    for lane in gain_lanes {
        let selection = selections[lane];
        let start = selection[0].min(selection[1]);
        let end = selection[0].max(selection[1]);
        let has_visible_selection = end > view_start && start < view_end;
        if !has_visible_selection {
            continue;
        }
        let visible_start = start.max(view_start);
        let visible_end = end.min(view_end);
        let lane_rect = Rect::from_min_max(
            Pos2::new(rect.left(), rect.top() + rect.height() * lane as f32 * 0.5),
            Pos2::new(
                rect.right(),
                rect.top() + rect.height() * (lane + 1) as f32 * 0.5,
            ),
        );
        let selected_rect = Rect::from_min_max(
            Pos2::new(
                x_from_time(visible_start, rect, view_start, visible_span),
                lane_rect.top(),
            ),
            Pos2::new(
                x_from_time(visible_end, rect, view_start, visible_span),
                lane_rect.bottom(),
            ),
        );
        let gain_handle_x = x_from_time((start + end) * 0.5, rect, view_start, visible_span);
        let lane_center = lane_rect.center().y;
        let gain_y = gain_handle_y(gain_db, lane_center);
        let mut previous_point = None;
        let mut x = selected_rect.left();
        while x <= selected_rect.right() {
            let t = (view_start + (x - rect.left()) / rect.width() * visible_span).clamp(0.0, 1.0);
            let point = Pos2::new(
                x,
                gain_handle_y(gain_envelopes[lane].gain_at(t), lane_center),
            );
            if let Some(previous) = previous_point {
                painter.line_segment([previous, point], Stroke::new(2.0_f32, ORANGE));
            }
            previous_point = Some(point);
            x += 2.0;
        }
        painter.circle_filled(Pos2::new(gain_handle_x, gain_y), 7.0, ORANGE);
        painter.circle_stroke(
            Pos2::new(gain_handle_x, gain_y),
            7.0,
            Stroke::new(1.0_f32, TEXT),
        );
        if lane == active_channel
            && end > start + 0.01
            && gain_handle_x >= rect.left()
            && gain_handle_x <= rect.right()
        {
            painter.text(
                Pos2::new(gain_handle_x, gain_y - 19.0),
                egui::Align2::CENTER_BOTTOM,
                format!("{:+.1} dB", gain_db),
                FontId::monospace(10.0),
                ORANGE,
            );
        }
    }
    for (lane, label) in ["L", "R"].iter().enumerate() {
        painter.text(
            Pos2::new(
                rect.left() + 8.0,
                rect.top() + rect.height() * (lane as f32 + 0.5) * 0.5,
            ),
            egui::Align2::LEFT_CENTER,
            label,
            FontId::proportional(10.0),
            if stereo_mode || lane == active_channel {
                ACCENT
            } else {
                MUTED
            },
        );
    }

    let play_x = x_from_time(playhead, rect, view_start, visible_span);
    painter.line_segment(
        [
            Pos2::new(play_x, rect.top()),
            Pos2::new(play_x, rect.bottom()),
        ],
        Stroke::new(1.5_f32, Color32::from_rgb(242, 105, 122)),
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(play_x - 5.0, rect.top()),
            Pos2::new(play_x + 5.0, rect.top()),
            Pos2::new(play_x, rect.top() + 7.0),
        ],
        Color32::from_rgb(242, 105, 122),
        Stroke::NONE,
    ));
    if playing {
        painter.circle_filled(
            Pos2::new(rect.right() - 12.0, rect.top() + 12.0),
            3.0,
            ACCENT,
        );
    }
}

fn quick_action(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: &str,
    message: &str,
    notice: &mut String,
) -> bool {
    let response = Frame::new()
        .fill(PANEL_ALT)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(7.0)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                icon(ui, Icon::Action, 14.0, ACCENT);
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).size(11.0).strong().color(TEXT));
                    ui.label(RichText::new(subtitle).size(9.0).color(MUTED));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    icon(ui, Icon::Chevron, 16.0, MUTED);
                });
            });
        });
    let rect = response.response.rect;
    let clicked = ui
        .interact(rect, ui.id().with(title), Sense::click())
        .clicked();
    if clicked {
        *notice = message.to_owned();
    }
    clicked
}

fn time_edit(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(10.0).color(MUTED));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            Frame::new()
                .fill(PANEL_ALT)
                .corner_radius(5.0)
                .inner_margin(Margin::symmetric(7, 4))
                .show(ui, |ui| {
                    ui.label(RichText::new(value).size(10.0).monospace().color(TEXT));
                });
        });
    });
}

fn pill_button(ui: &mut egui::Ui, label: &str, prominent: bool) -> egui::Response {
    let (fill, text) = if prominent {
        (ACCENT, BG)
    } else {
        (PANEL_ALT, TEXT)
    };
    ui.add(
        egui::Button::new(RichText::new(label).size(11.0).strong().color(text))
            .fill(fill)
            .stroke(Stroke::new(
                1.0_f32,
                if prominent { ACCENT } else { BORDER },
            ))
            .corner_radius(7.0),
    )
}

fn icon(ui: &mut egui::Ui, kind: Icon, size: f32, color: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_icon(ui.painter(), rect, kind, color);
    response
}

fn tool_button(ui: &mut egui::Ui, kind: Icon, label: &str, active: bool) -> egui::Response {
    let width = label.len() as f32 * 7.0 + 34.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::click());
    let fill = if active {
        PANEL_ALT
    } else if response.hovered() {
        Color32::from_rgb(40, 49, 61)
    } else {
        BG
    };
    let color = if active { ACCENT } else { MUTED };
    ui.painter().rect(
        rect,
        7.0,
        fill,
        Stroke::new(1.0_f32, if active { BORDER } else { BG }),
        egui::StrokeKind::Inside,
    );
    paint_icon(
        ui.painter(),
        Rect::from_center_size(
            Pos2::new(rect.left() + 16.0, rect.center().y),
            Vec2::splat(15.0),
        ),
        kind,
        color,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 29.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.0),
        color,
    );
    response
}

fn icon_button(ui: &mut egui::Ui, kind: Icon, size: f32, fill: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let button_fill = if response.hovered() && fill != RECORDING_RED {
        Color32::from_rgb(40, 49, 61)
    } else {
        fill
    };
    ui.painter().rect_filled(rect, 6.0, button_fill);
    paint_icon(ui.painter(), rect.shrink(size * 0.22), kind, TEXT);
    response
}

fn transport_button(ui: &mut egui::Ui, kind: Icon) -> egui::Response {
    icon_button(ui, kind, 30.0, PANEL)
}

fn paint_icon(painter: &egui::Painter, rect: Rect, kind: Icon, color: Color32) {
    let center = rect.center();
    let size = rect.width().min(rect.height());
    let stroke = Stroke::new((size / 10.0).max(1.0), color);
    let line = |from, to| painter.line_segment([from, to], stroke);
    let triangle = |points| painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
    match kind {
        Icon::Select => {
            let points = [
                Pos2::new(rect.left() + size * 0.2, rect.top() + size * 0.1),
                Pos2::new(rect.left() + size * 0.2, rect.bottom() - size * 0.1),
                Pos2::new(rect.left() + size * 0.42, rect.bottom() - size * 0.34),
                Pos2::new(rect.left() + size * 0.56, rect.bottom() - size * 0.08),
            ];
            for segment in points.windows(2) {
                line(segment[0], segment[1]);
            }
            line(points[3], points[2]);
            line(points[2], points[1]);
        }
        Icon::Trim => {
            painter.circle_stroke(
                Pos2::new(rect.left() + size * 0.25, rect.bottom() - size * 0.22),
                size * 0.13,
                stroke,
            );
            painter.circle_stroke(
                Pos2::new(rect.left() + size * 0.25, rect.top() + size * 0.22),
                size * 0.13,
                stroke,
            );
            line(
                Pos2::new(rect.left() + size * 0.36, rect.top() + size * 0.27),
                Pos2::new(rect.right() - size * 0.13, rect.bottom() - size * 0.12),
            );
            line(
                Pos2::new(rect.left() + size * 0.36, rect.bottom() - size * 0.27),
                Pos2::new(rect.right() - size * 0.13, rect.top() + size * 0.12),
            );
        }
        Icon::Split => {
            line(
                Pos2::new(center.x, rect.top() + size * 0.12),
                Pos2::new(center.x, rect.bottom() - size * 0.12),
            );
            for y in [rect.top() + size * 0.28, rect.bottom() - size * 0.28] {
                line(
                    Pos2::new(rect.left() + size * 0.14, y),
                    Pos2::new(rect.right() - size * 0.14, y),
                );
            }
        }
        Icon::Draw => {
            line(
                Pos2::new(rect.left() + size * 0.2, rect.bottom() - size * 0.2),
                Pos2::new(rect.right() - size * 0.2, rect.top() + size * 0.2),
            );
            line(
                Pos2::new(rect.left() + size * 0.13, rect.bottom() - size * 0.13),
                Pos2::new(rect.left() + size * 0.28, rect.bottom() - size * 0.24),
            );
        }
        Icon::Undo | Icon::Redo => {
            let reverse = matches!(kind, Icon::Undo);
            let side = if reverse { -1.0 } else { 1.0 };
            let x = center.x + side * size * 0.28;
            line(
                Pos2::new(center.x - side * size * 0.28, center.y - size * 0.22),
                Pos2::new(x, center.y - size * 0.22),
            );
            line(
                Pos2::new(x, center.y - size * 0.22),
                Pos2::new(x, center.y + size * 0.16),
            );
            line(
                Pos2::new(x, center.y + size * 0.16),
                Pos2::new(center.x - side * size * 0.12, center.y + size * 0.16),
            );
            line(
                Pos2::new(center.x - side * size * 0.28, center.y - size * 0.22),
                Pos2::new(center.x - side * size * 0.08, center.y - size * 0.42),
            );
            line(
                Pos2::new(center.x - side * size * 0.28, center.y - size * 0.22),
                Pos2::new(center.x - side * size * 0.08, center.y - size * 0.02),
            );
        }
        Icon::ZoomIn | Icon::ZoomOut => {
            painter.circle_stroke(center - Vec2::splat(size * 0.08), size * 0.27, stroke);
            line(
                center + Vec2::splat(size * 0.12),
                Pos2::new(rect.right() - size * 0.08, rect.bottom() - size * 0.08),
            );
            line(
                Pos2::new(center.x - size * 0.23, center.y - size * 0.08),
                Pos2::new(center.x + size * 0.07, center.y - size * 0.08),
            );
            if matches!(kind, Icon::ZoomIn) {
                line(
                    Pos2::new(center.x - size * 0.08, center.y - size * 0.23),
                    Pos2::new(center.x - size * 0.08, center.y + size * 0.07),
                );
            }
        }
        Icon::More => {
            for x in [center.x - size * 0.3, center.x, center.x + size * 0.3] {
                painter.circle_filled(Pos2::new(x, center.y), size * 0.09, color);
            }
        }
        Icon::Previous | Icon::Next => {
            let previous = matches!(kind, Icon::Previous);
            let x = if previous {
                rect.left() + size * 0.25
            } else {
                rect.right() - size * 0.25
            };
            line(
                Pos2::new(x, rect.top() + size * 0.16),
                Pos2::new(x, rect.bottom() - size * 0.16),
            );
            if previous {
                triangle(vec![
                    Pos2::new(rect.right() - size * 0.18, rect.top() + size * 0.18),
                    Pos2::new(rect.left() + size * 0.38, center.y),
                    Pos2::new(rect.right() - size * 0.18, rect.bottom() - size * 0.18),
                ]);
            } else {
                triangle(vec![
                    Pos2::new(rect.left() + size * 0.18, rect.top() + size * 0.18),
                    Pos2::new(rect.right() - size * 0.38, center.y),
                    Pos2::new(rect.left() + size * 0.18, rect.bottom() - size * 0.18),
                ]);
            }
        }
        Icon::Play => {
            triangle(vec![
                Pos2::new(rect.left() + size * 0.25, rect.top() + size * 0.12),
                Pos2::new(rect.right() - size * 0.15, center.y),
                Pos2::new(rect.left() + size * 0.25, rect.bottom() - size * 0.12),
            ]);
        }
        Icon::Pause => {
            let bar_width = size * 0.2;
            painter.rect_filled(
                Rect::from_center_size(
                    Pos2::new(center.x - size * 0.17, center.y),
                    Vec2::new(bar_width, size * 0.65),
                ),
                1.0,
                color,
            );
            painter.rect_filled(
                Rect::from_center_size(
                    Pos2::new(center.x + size * 0.17, center.y),
                    Vec2::new(bar_width, size * 0.65),
                ),
                1.0,
                color,
            );
        }
        Icon::Stop => {
            painter.rect_filled(
                Rect::from_center_size(center, Vec2::splat(size * 0.52)),
                2.0,
                color,
            );
        }
        Icon::Record => {
            painter.circle_filled(center, size * 0.38, color);
        }
        Icon::Action => {
            line(
                Pos2::new(rect.left() + size * 0.18, rect.bottom() - size * 0.18),
                Pos2::new(rect.right() - size * 0.2, rect.top() + size * 0.2),
            );
            line(
                Pos2::new(rect.right() - size * 0.2, rect.top() + size * 0.2),
                Pos2::new(rect.right() - size * 0.2, rect.top() + size * 0.45),
            );
            line(
                Pos2::new(rect.right() - size * 0.2, rect.top() + size * 0.2),
                Pos2::new(rect.right() - size * 0.45, rect.top() + size * 0.2),
            );
        }
        Icon::Chevron => {
            line(
                Pos2::new(center.x - size * 0.15, center.y - size * 0.3),
                Pos2::new(center.x + size * 0.15, center.y),
            );
            line(
                Pos2::new(center.x + size * 0.15, center.y),
                Pos2::new(center.x - size * 0.15, center.y + size * 0.3),
            );
        }
        Icon::Brand => {
            painter.circle_stroke(center, size * 0.38, stroke);
            painter.circle_filled(center, size * 0.11, color);
        }
    }
}

fn channel_label(channels: usize) -> &'static str {
    match channels {
        1 => "MONO",
        2 => "STEREO",
        _ => "MULTICHANNEL",
    }
}

fn channel_label_single(channel: usize) -> &'static str {
    if channel == 0 { "L" } else { "R" }
}

fn gain_handle_y(gain_db: f32, center_y: f32) -> f32 {
    center_y - gain_db.clamp(-24.0, 18.0) * 4.0
}

fn gain_delta_from_pointer(lane_center: f32, pointer_y: f32) -> f32 {
    ((lane_center - pointer_y) / 4.0).clamp(-24.0, 18.0)
}

fn format_time(seconds: f32) -> String {
    let minutes = (seconds / 60.0).floor() as u32;
    let remaining = seconds - minutes as f32 * 60.0;
    format!("{minutes:02}:{remaining:05.2}")
}

#[cfg(test)]
mod history_tests {
    use super::*;

    fn stereo_app(samples: Arc<[f32]>) -> RinaApp {
        RinaApp {
            tracks: vec![AudioData {
                name: "stereo.wav".to_owned(),
                sample_rate: 48_000,
                peaks: vec![vec![[0.25, 0.75]], vec![[-1.0, -0.5]]],
                samples,
                duration: 2.0 / 48_000.0,
            }],
            track_envelopes: vec![vec![GainEnvelope::default(), GainEnvelope::default()]],
            ..RinaApp::default()
        }
    }

    #[test]
    fn undo_redo_restore_exact_interleaved_stereo_samples_and_gain() {
        let original_samples: Arc<[f32]> = Arc::from([0.25, -0.5, 0.75, -1.0]);
        let mut app = stereo_app(Arc::clone(&original_samples));
        app.selection = [0.25, 0.75];
        app.save_edit();

        let processed_samples: Arc<[f32]> = Arc::from([0.5, -1.0, 0.75, -1.0]);
        app.audio_mut().unwrap().samples = Arc::clone(&processed_samples);
        app.gain_envelope.set_region(0.25, 0.75, 6.0206);
        app.gain_db = 6.0206;

        app.undo();
        assert_eq!(
            app.audio().unwrap().samples.as_ref(),
            original_samples.as_ref()
        );
        assert_eq!(app.gain_envelope.gain_at(0.5), 0.0);
        assert_eq!(app.gain_db, 0.0);

        app.redo();
        assert_eq!(
            app.audio().unwrap().samples.as_ref(),
            processed_samples.as_ref()
        );
        assert!((app.gain_envelope.gain_at(0.5) - 6.0206).abs() < 0.001);
        assert!((app.gain_db - 6.0206).abs() < 0.001);
    }

    #[test]
    fn normalize_is_undoable_and_redoable_without_changing_source_samples() {
        let source: Arc<[f32]> = Arc::from([0.2, -0.4, 0.8, -0.5]);
        let mut app = stereo_app(Arc::clone(&source));
        app.selection = [0.0, 1.0];
        app.gain_envelope.set_region(0.0, 1.0, 3.0);
        app.save_active_channel_state();
        let previous_envelope = app.gain_envelope.clone();

        app.normalize();
        let normalized_envelope = app.gain_envelope.clone();
        assert_ne!(normalized_envelope, previous_envelope);

        app.undo();
        assert_eq!(app.gain_envelope, previous_envelope);
        assert_eq!(app.audio().unwrap().samples.as_ref(), source.as_ref());

        app.redo();
        assert_eq!(app.gain_envelope, normalized_envelope);
        assert_eq!(app.audio().unwrap().samples.as_ref(), source.as_ref());
    }

    #[test]
    fn eq_and_compressor_edits_are_undoable_and_redoable() {
        let source: Arc<[f32]> = Arc::from([0.1, 0.1, 0.8, -0.8, 0.9, -0.9, 0.1, 0.1]);
        let mut app = stereo_app(Arc::clone(&source));
        app.selection = [0.25, 0.75];
        app.equalizer.low_db = 12.0;
        app.apply_eq();
        let eq_samples = Arc::clone(&app.audio().unwrap().samples);
        assert_ne!(eq_samples.as_ref(), source.as_ref());
        app.undo();
        assert_eq!(app.audio().unwrap().samples.as_ref(), source.as_ref());
        app.redo();
        assert_eq!(app.audio().unwrap().samples.as_ref(), eq_samples.as_ref());

        app.selection = [0.25, 0.75];
        app.compressor.threshold_db = -12.0;
        app.compressor.ratio = 4.0;
        app.apply_compressor();
        let compressed_samples = Arc::clone(&app.audio().unwrap().samples);
        assert_ne!(compressed_samples.as_ref(), eq_samples.as_ref());
        app.undo();
        assert_eq!(app.audio().unwrap().samples.as_ref(), eq_samples.as_ref());
        app.redo();
        assert_eq!(
            app.audio().unwrap().samples.as_ref(),
            compressed_samples.as_ref()
        );
    }

    #[test]
    fn zoom_changes_visible_time_range_and_scroll_clamps_to_file_bounds() {
        let mut app = RinaApp {
            selection: [0.26, 0.26],
            ..RinaApp::default()
        };
        app.set_zoom(4.0);
        assert_eq!(app.visible_span(), 0.25);
        assert!((app.view_start - 0.135).abs() < 0.001);
        app.scroll_timeline(0.1);
        assert!((app.view_start - 0.235).abs() < 0.001);
        app.scroll_timeline(2.0);
        assert_eq!(app.view_start, 0.75);
        app.set_zoom(0.0);
        assert_eq!(app.zoom, 1.0);
        assert_eq!(app.visible_span(), 1.0);
        assert_eq!(app.view_start, 0.0);
    }

    #[test]
    fn zoom_uses_selection_center_without_changing_selection_or_playhead() {
        let mut app = RinaApp {
            selection: [0.4, 0.6],
            playhead: 0.1,
            ..RinaApp::default()
        };
        app.set_zoom(4.0);

        assert!((app.view_start - 0.375).abs() < 0.001);
        assert_eq!(app.selection, [0.4, 0.6]);
        assert_eq!(app.playhead, 0.1);
    }

    #[test]
    fn panning_moves_by_ten_percent_of_view_and_preserves_selection_and_cursor() {
        let mut app = RinaApp {
            selection: [0.4, 0.6],
            playhead: 0.5,
            ..RinaApp::default()
        };
        app.set_zoom(4.0);
        let original_start = app.view_start;

        app.pan_timeline(1.0);
        assert!((app.view_start - (original_start + 0.025)).abs() < 0.001);
        app.pan_timeline(-1.0);
        assert!((app.view_start - original_start).abs() < 0.001);
        assert_eq!(app.selection, [0.4, 0.6]);
        assert_eq!(app.playhead, 0.5);
    }

    #[test]
    fn panning_clamps_to_the_start_and_end_of_audio() {
        let mut app = RinaApp {
            selection: [0.5, 0.5],
            playhead: 0.05,
            ..RinaApp::default()
        };
        app.set_zoom(4.0);

        app.pan_timeline(-1.0);
        assert_eq!(app.view_start, 0.0);
        app.pan_timeline(100.0);
        assert_eq!(app.view_start, 0.75);
    }

    #[test]
    fn stopping_playback_preserves_the_time_selection() {
        let mut app = RinaApp {
            selection: [0.25, 0.75],
            ..RinaApp::default()
        };

        app.stop_playback();

        assert_eq!(app.selection, [0.25, 0.75]);
        assert_eq!(app.playhead, 0.0);
    }

    #[test]
    fn selection_frame_range_normalizes_reversed_selection_and_ignores_empty_ranges() {
        assert_eq!(selection_frame_range([0.75, 0.25], 100), Some((25, 75)));
        assert_eq!(selection_frame_range([0.5, 0.5], 100), None);
        assert_eq!(selection_frame_range([0.0, 1.0], 100), Some((0, 100)));
    }

    #[test]
    fn selecting_recorded_track_keeps_track_audio_and_gain_independent() {
        let mut app = stereo_app(Arc::from([0.1, 0.2, 0.3, 0.4]));
        app.tracks.push(AudioData {
            name: "Recording".to_owned(),
            sample_rate: 48_000,
            peaks: vec![vec![[0.0, 0.0]], vec![[0.0, 0.0]]],
            samples: Arc::from([0.5, 0.6, 0.7, 0.8]),
            duration: 2.0 / 48_000.0,
        });
        app.track_envelopes
            .push(vec![GainEnvelope::default(), GainEnvelope::default()]);
        app.gain_envelope.set_region(0.0, 1.0, 3.0);

        app.select_track(1);
        assert_eq!(app.audio().unwrap().name, "Recording");
        assert_eq!(app.gain_envelope.gain_at(0.5), 0.0);
        app.gain_envelope.set_region(0.0, 1.0, -6.0);
        app.select_track(0);

        assert_eq!(app.gain_envelope.gain_at(0.5), 3.0);
        assert_eq!(app.audio().unwrap().samples.as_ref(), [0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn fade_buttons_apply_to_selection_and_can_be_undone() {
        let source: Arc<[f32]> = Arc::from([0.8, 0.8, 0.8, 0.8, 0.8, 0.8, 0.8, 0.8]);
        let mut app = stereo_app(Arc::clone(&source));
        app.stereo_mode = false;
        app.selection = [0.25, 0.75];

        app.apply_fade_in();
        assert_eq!(app.audio().unwrap().samples[0], source[0]);
        assert_eq!(&app.audio().unwrap().samples[2..4], &[0.0, 0.8]);
        assert_eq!(&app.audio().unwrap().samples[4..6], &[0.8, 0.8]);
        assert_eq!(&app.audio().unwrap().samples[6..], &source[6..]);
        app.undo();
        assert_eq!(app.audio().unwrap().samples.as_ref(), source.as_ref());

        app.apply_fade_out();
        assert_eq!(&app.audio().unwrap().samples[2..4], &[0.8, 0.8]);
        assert_eq!(&app.audio().unwrap().samples[4..6], &[0.0, 0.8]);
    }

    #[test]
    fn normalize_and_effects_only_modify_the_active_channel_and_undo_restores_it() {
        let source: Arc<[f32]> = Arc::from([0.1, 0.9, 0.2, -0.8, 0.3, 0.7, 0.4, -0.6]);
        let mut app = stereo_app(Arc::clone(&source));
        app.active_channel = 1;
        app.stereo_mode = false;
        app.selection = [0.0, 1.0];
        app.channel_selections[1] = app.selection;
        let left_before = interleave_channel(&source, 2, 0);

        app.normalize();
        assert_eq!(
            interleave_channel(&app.audio().unwrap().samples, 2, 0),
            left_before
        );
        assert!(app.gain_envelope.gain_at(0.5).abs() > 0.05);
        app.undo();
        assert_eq!(app.audio().unwrap().samples.as_ref(), source.as_ref());
        assert_eq!(app.gain_envelope, GainEnvelope::default());

        app.equalizer.high_db = 12.0;
        app.apply_eq();
        assert_eq!(
            interleave_channel(&app.audio().unwrap().samples, 2, 0),
            left_before
        );
        assert_ne!(
            interleave_channel(&app.audio().unwrap().samples, 2, 1),
            interleave_channel(&source, 2, 1)
        );
        app.undo();
        assert_eq!(app.audio().unwrap().samples.as_ref(), source.as_ref());
    }

    #[test]
    fn channel_copy_respects_selection_and_undo_preserves_both_channels() {
        let source: Arc<[f32]> = Arc::from([0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6]);
        let mut app = stereo_app(Arc::clone(&source));
        app.selection = [0.25, 0.75];

        app.copy_channel(0, 1);
        assert_eq!(&app.audio().unwrap().samples[0..2], &source[0..2]);
        assert_eq!(&app.audio().unwrap().samples[2..6], &[0.2, 0.2, 0.3, 0.3]);
        assert_eq!(&app.audio().unwrap().samples[6..], &source[6..]);
        app.undo();
        assert_eq!(app.audio().unwrap().samples.as_ref(), source.as_ref());

        app.selection = [0.0, 1.0];
        app.select_channel(0);
        app.copy_channel(0, 1);
        assert_eq!(
            interleave_channel(&app.audio().unwrap().samples, 2, 1),
            interleave_channel(&source, 2, 0)
        );
    }

    #[test]
    fn cut_and_paste_only_change_the_active_channel() {
        let source: Arc<[f32]> = Arc::from([0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6]);
        let mut app = stereo_app(Arc::clone(&source));
        app.active_channel = 1;
        app.stereo_mode = false;
        app.selection = [0.25, 0.75];

        app.cut_selection();
        assert_eq!(
            interleave_channel(&app.audio().unwrap().samples, 2, 0),
            interleave_channel(&source, 2, 0)
        );
        assert_eq!(&app.audio().unwrap().samples[2..6], &[0.2, 0.0, 0.3, 0.0]);
        app.undo();
        assert_eq!(app.audio().unwrap().samples.as_ref(), source.as_ref());
        app.paste_selection();
        assert_eq!(app.audio().unwrap().samples.as_ref(), source.as_ref());
    }

    #[test]
    fn deleting_in_stereo_mode_removes_selected_time_and_undo_restores_duration() {
        let source: Arc<[f32]> = Arc::from([0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6]);
        let mut app = stereo_app(Arc::clone(&source));
        app.selection = [0.25, 0.75];

        app.delete_selection();

        assert_eq!(app.audio().unwrap().samples.as_ref(), [0.1, 0.9, 0.4, 0.6]);
        assert!((app.duration() - 2.0 / 48_000.0).abs() < f32::EPSILON);
        assert_eq!(app.channel_selections, [[0.5, 0.5], [0.5, 0.5]]);
        app.undo();
        assert_eq!(app.audio().unwrap().samples.as_ref(), source.as_ref());
        assert!((app.duration() - 4.0 / 48_000.0).abs() < f32::EPSILON);
    }

    #[test]
    fn delete_key_deletes_selection_when_waveform_has_focus() {
        let mut app = stereo_app(Arc::from([0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6]));
        app.waveform_focused = true;
        app.selection = [0.25, 0.75];
        let context = egui::Context::default();

        let _ = context.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Delete,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |context| app.update_shortcuts(context),
        );

        assert_eq!(app.audio().unwrap().samples.as_ref(), [0.1, 0.9, 0.4, 0.6]);
    }

    #[test]
    fn deleting_in_single_channel_mode_silences_without_shortening_audio() {
        let source: Arc<[f32]> = Arc::from([0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6]);
        let mut app = stereo_app(Arc::clone(&source));
        app.stereo_mode = false;
        app.active_channel = 1;
        app.selection = [0.25, 0.75];

        app.delete_selection();

        assert_eq!(
            app.audio().unwrap().samples.as_ref(),
            [0.1, 0.9, 0.2, 0.0, 0.3, 0.0, 0.4, 0.6]
        );
        assert!((app.duration() - 4.0 / 48_000.0).abs() < f32::EPSILON);
    }

    #[test]
    fn zero_crossing_search_finds_nearest_stereo_frame() {
        let samples = [0.6, 0.5, 0.2, 0.1, -0.1, -0.2, -0.5, -0.6];
        assert_eq!(nearest_zero_crossing(&samples, 2, 2, 2), Some(2));
    }

    #[test]
    fn stereo_delete_snaps_selection_edges_to_nearby_zero_crossings() {
        let mut mono_frames = vec![0.5_f32; 100];
        mono_frames[28..72].fill(-0.5);
        let source: Arc<[f32]> = mono_frames
            .iter()
            .flat_map(|&sample| [sample, sample])
            .collect::<Vec<_>>()
            .into();
        let mut app = stereo_app(Arc::clone(&source));
        app.tracks[0].sample_rate = 1000;
        app.tracks[0].duration = 0.1;
        app.selection = [0.3, 0.7];
        app.snap_enabled = true;

        app.delete_selection();

        assert_eq!(app.audio().unwrap().samples.len(), 56 * 2);
        assert!(
            app.audio()
                .unwrap()
                .samples
                .iter()
                .all(|sample| *sample == 0.5)
        );
    }

    #[test]
    fn copying_and_pasting_selected_channel_works_at_selection_or_cursor() {
        let source: Arc<[f32]> = Arc::from([0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6]);
        let mut app = stereo_app(Arc::clone(&source));
        app.stereo_mode = false;
        app.active_channel = 1;
        app.selection = [0.0, 0.5];

        assert!(app.copy_selection());
        app.selection = [0.5, 0.75];
        app.playhead = 0.5;
        app.paste_selection();

        assert_eq!(
            interleave_channel(&app.audio().unwrap().samples, 2, 0),
            interleave_channel(&source, 2, 0)
        );
        assert_eq!(
            interleave_channel(&app.audio().unwrap().samples, 2, 1),
            [0.9, 0.8, 0.9, 0.8]
        );
        app.undo();
        assert_eq!(app.audio().unwrap().samples.as_ref(), source.as_ref());
    }

    #[test]
    fn paste_at_end_extends_audio_and_keeps_stereo_frame_alignment() {
        let mut app = stereo_app(Arc::from([0.1, 0.9, 0.2, 0.8]));
        app.selection = [0.0, 1.0];
        app.copy_selection();
        app.playhead = 1.0;
        app.selection = [1.0, 1.0];

        app.paste_selection();

        assert_eq!(
            app.audio().unwrap().samples.as_ref(),
            [0.1, 0.9, 0.2, 0.8, 0.1, 0.9, 0.2, 0.8]
        );
        assert!((app.audio().unwrap().duration - 4.0 / 48_000.0).abs() < f32::EPSILON);
    }

    #[test]
    fn selecting_l_and_r_keeps_separate_time_ranges() {
        let mut app = stereo_app(Arc::from([0.1, 0.2, 0.3, 0.4]));
        app.selection = [0.1, 0.3];
        app.save_active_channel_state();

        app.select_channel(1);
        app.selection = [0.6, 0.9];
        app.save_active_channel_state();
        app.select_channel(0);

        assert_eq!(app.selection, [0.1, 0.3]);
        app.select_channel(1);
        assert_eq!(app.selection, [0.6, 0.9]);
    }

    #[test]
    fn stereo_is_the_default_and_select_all_updates_both_channel_ranges() {
        let mut app = stereo_app(Arc::from([0.1, 0.2, 0.3, 0.4]));
        assert!(app.stereo_mode);

        app.select_all();

        assert_eq!(app.selection, [0.0, 1.0]);
        assert_eq!(app.channel_selections, [[0.0, 1.0], [0.0, 1.0]]);
        app.select_channel(1);
        assert!(!app.stereo_mode);
        assert_eq!(app.selection, [0.0, 1.0]);
    }

    #[test]
    fn gain_drag_uses_the_selected_lane_center() {
        let upper_lane_center = 100.0;
        let lower_lane_center = 300.0;

        assert_eq!(
            gain_delta_from_pointer(upper_lane_center, upper_lane_center),
            0.0
        );
        assert_eq!(
            gain_delta_from_pointer(lower_lane_center, lower_lane_center),
            0.0
        );
        assert_eq!(
            gain_delta_from_pointer(lower_lane_center, lower_lane_center - 40.0),
            10.0
        );
    }

    #[test]
    fn stereo_dsp_processes_both_channels_without_collapsing_stereo_data() {
        let source: Arc<[f32]> = Arc::from([0.1, 0.9, 0.2, -0.8, 0.3, 0.7, 0.4, -0.6]);
        let mut app = stereo_app(Arc::clone(&source));
        app.selection = [0.0, 1.0];
        app.equalizer.high_db = 12.0;

        app.apply_eq();

        assert_eq!(app.audio().unwrap().channel_count(), 2);
        assert_ne!(
            interleave_channel(&app.audio().unwrap().samples, 2, 0),
            interleave_channel(&source, 2, 0)
        );
        assert_ne!(
            interleave_channel(&app.audio().unwrap().samples, 2, 1),
            interleave_channel(&source, 2, 1)
        );
        app.undo();
        assert_eq!(app.audio().unwrap().samples.as_ref(), source.as_ref());
    }
}
