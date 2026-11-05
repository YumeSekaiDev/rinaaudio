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

use audio::{AudioData, decode_audio, export_wav};
use dsp::{Compressor, Equalizer, apply_compressor, apply_equalizer};
use editing::{GainEnvelope, db_to_linear, normalize_region};
use playback::Playback;

const BG: Color32 = Color32::from_rgb(15, 18, 24);
const PANEL: Color32 = Color32::from_rgb(21, 25, 33);
const PANEL_ALT: Color32 = Color32::from_rgb(27, 32, 42);
const BORDER: Color32 = Color32::from_rgb(43, 50, 63);
const TEXT: Color32 = Color32::from_rgb(231, 236, 243);
const MUTED: Color32 = Color32::from_rgb(134, 145, 162);
const ACCENT: Color32 = Color32::from_rgb(105, 219, 190);
const WAVE: Color32 = Color32::from_rgb(84, 191, 170);
const ORANGE: Color32 = Color32::from_rgb(244, 173, 100);

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
    selection: [f32; 2],
    gain_envelope: GainEnvelope,
    samples: Option<Arc<[f32]>>,
    peaks: Option<Vec<Vec<[f32; 2]>>>,
}

#[derive(Clone, Copy, PartialEq)]
enum EffectPanel {
    Equalizer,
    Compressor,
}

struct RinaApp {
    audio: Option<AudioData>,
    playback: Option<Playback>,
    import_receiver: Option<Receiver<Result<AudioData, String>>>,
    export_receiver: Option<Receiver<Result<(), String>>>,
    gain_envelope: GainEnvelope,
    effect_panel: Option<EffectPanel>,
    equalizer: Equalizer,
    compressor: Compressor,
    selection: [f32; 2],
    gain_db: f32,
    gain_dragging: bool,
    playing: bool,
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
            audio: None,
            playback: None,
            import_receiver: None,
            export_receiver: None,
            gain_envelope: GainEnvelope::default(),
            effect_panel: None,
            equalizer: Equalizer::default(),
            compressor: Compressor::default(),
            selection: [0.18, 0.63],
            gain_db: 0.0,
            gain_dragging: false,
            playing: false,
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
    fn save_edit(&mut self) {
        self.save_edit_with_gain(self.gain_db);
    }

    fn save_edit_with_gain(&mut self, gain_db: f32) {
        self.undo.push(EditState {
            gain_db,
            gain_envelope: self.gain_envelope.clone(),
            selection: self.selection,
            samples: self.audio.as_ref().map(|audio| Arc::clone(&audio.samples)),
            peaks: self.audio.as_ref().map(|audio| audio.peaks.clone()),
        });
        self.redo.clear();
    }

    fn restore(&mut self, state: EditState) {
        self.gain_db = state.gain_db;
        self.selection = state.selection;
        self.gain_envelope = state.gain_envelope;
        if let (Some(audio), Some(samples)) = (&mut self.audio, state.samples) {
            audio.samples = samples;
        }
        if let (Some(audio), Some(peaks)) = (&mut self.audio, state.peaks) {
            audio.peaks = peaks;
        }
        if let Some(playback) = &self.playback {
            playback.set_gain_envelope(self.gain_envelope.clone());
            if let Some(audio) = &self.audio {
                playback.set_samples(Arc::clone(&audio.samples));
            }
        }
    }

    fn undo(&mut self) {
        if let Some(state) = self.undo.pop() {
            self.redo.push(EditState {
                gain_db: self.gain_db,
                gain_envelope: self.gain_envelope.clone(),
                selection: self.selection,
                samples: self.audio.as_ref().map(|audio| Arc::clone(&audio.samples)),
                peaks: self.audio.as_ref().map(|audio| audio.peaks.clone()),
            });
            self.restore(state);
            self.notice = "Edit undone".to_owned();
        }
    }

    fn redo(&mut self) {
        if let Some(state) = self.redo.pop() {
            self.undo.push(EditState {
                gain_db: self.gain_db,
                gain_envelope: self.gain_envelope.clone(),
                selection: self.selection,
                samples: self.audio.as_ref().map(|audio| Arc::clone(&audio.samples)),
                peaks: self.audio.as_ref().map(|audio| audio.peaks.clone()),
            });
            self.restore(state);
            self.notice = "Edit restored".to_owned();
        }
    }

    fn duration(&self) -> f32 {
        self.audio.as_ref().map_or(42.0, |audio| audio.duration)
    }

    fn visible_span(&self) -> f32 {
        (1.0 / self.zoom).min(1.0)
    }

    fn set_zoom(&mut self, zoom: f32) {
        let center = self.view_start + self.visible_span() * 0.5;
        self.zoom = zoom.clamp(1.0, 32.0);
        let span = self.visible_span();
        self.view_start = (center - span * 0.5).clamp(0.0, 1.0 - span);
    }

    fn scroll_timeline(&mut self, delta: f32) {
        let span = self.visible_span();
        self.view_start = (self.view_start + delta).clamp(0.0, 1.0 - span);
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
            let Some(audio) = &self.audio else {
                self.notice = "Import an audio file before playback".to_owned();
                return;
            };
            match Playback::new(
                audio.samples.clone(),
                audio.sample_rate,
                audio.channel_count(),
                self.gain_envelope.clone(),
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
        let Some(audio) = &self.audio else {
            self.notice = "Import an audio file before normalizing".to_owned();
            return;
        };
        let mut envelope = self.gain_envelope.clone();
        match normalize_region(
            &audio.samples,
            audio.channel_count(),
            self.selection,
            -1.0,
            &mut envelope,
        ) {
            Ok(gain_delta_db) => {
                self.save_edit();
                self.gain_envelope = envelope;
                let midpoint = (self.selection[0] + self.selection[1]) * 0.5;
                self.gain_db = self.gain_envelope.gain_at(midpoint);
                if let Some(playback) = &self.playback {
                    playback.set_gain_envelope(self.gain_envelope.clone());
                }
                self.notice = format!("Normalized to −1 dBFS ({gain_delta_db:+.1} dB)");
            }
            Err(error) => self.notice = error.to_owned(),
        }
    }

    fn apply_eq(&mut self) {
        let Some(audio) = &self.audio else {
            self.notice = "Import an audio file before applying EQ".to_owned();
            return;
        };
        let mut samples = audio.samples.to_vec();
        if let Err(error) = apply_equalizer(
            &mut samples,
            audio.sample_rate,
            audio.channel_count(),
            self.selection,
            self.equalizer,
        ) {
            self.notice = error.to_owned();
            return;
        }
        self.save_edit();
        if let Some(audio) = &mut self.audio {
            audio.samples = samples.into();
            audio.rebuild_peaks();
            if let Some(playback) = &self.playback {
                playback.set_samples(Arc::clone(&audio.samples));
            }
        }
        self.notice = "EQ applied to selected region".to_owned();
    }

    fn apply_compressor(&mut self) {
        let Some(audio) = &self.audio else {
            self.notice = "Import an audio file before compressing".to_owned();
            return;
        };
        let mut samples = audio.samples.to_vec();
        if let Err(error) = apply_compressor(
            &mut samples,
            audio.channel_count(),
            self.selection,
            self.compressor,
        ) {
            self.notice = error.to_owned();
            return;
        }
        self.save_edit();
        if let Some(audio) = &mut self.audio {
            audio.samples = samples.into();
            audio.rebuild_peaks();
            if let Some(playback) = &self.playback {
                playback.set_samples(Arc::clone(&audio.samples));
            }
        }
        self.notice = "Compression applied to selected region".to_owned();
    }

    fn export_audio(&mut self) {
        if self.export_receiver.is_some() {
            self.notice = "Audio export is already in progress".to_owned();
            return;
        }
        let Some(audio) = &self.audio else {
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
        let envelope = self.gain_envelope.clone();
        let (sender, receiver) = mpsc::channel();
        self.export_receiver = Some(receiver);
        self.notice = "Exporting processed WAV…".to_owned();
        if let Err(error) = std::thread::Builder::new()
            .name("rina-audio-export".to_owned())
            .spawn(move || {
                let result = export_wav(&path, &samples, sample_rate, channels, &envelope);
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
                    self.audio = Some(audio);
                    self.playback = None;
                    self.selection = [0.0, 1.0];
                    self.gain_db = 0.0;
                    self.gain_envelope = GainEnvelope::default();
                    self.playhead = 0.0;
                    self.zoom = 1.0;
                    self.view_start = 0.0;
                    self.undo.clear();
                    self.redo.clear();
                    self.playing = false;
                    self.notice = "Audio imported".to_owned();
                }
                Err(error) => self.notice = format!("Import failed: {error}"),
            }
        }
    }

    fn update_shortcuts(&mut self, context: &egui::Context) {
        context.input(|input| {
            if input.modifiers.command && input.key_pressed(egui::Key::O) {
                self.open_audio();
            } else if input.modifiers.command && input.key_pressed(egui::Key::Z) {
                self.undo();
            } else if input.modifiers.command && input.key_pressed(egui::Key::Y) {
                self.redo();
            } else if input.key_pressed(egui::Key::Space) {
                self.toggle_playback();
            } else if input.key_pressed(egui::Key::Plus) || input.key_pressed(egui::Key::Equals) {
                self.set_zoom(self.zoom * 1.2);
            } else if input.key_pressed(egui::Key::Minus) {
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
                    ui.label(RichText::new("◉").size(23.0).strong().color(ACCENT));
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
                    ui.label(RichText::new("•").color(ACCENT));
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
                        ui.label(RichText::new("⋯").size(22.0).color(MUTED));
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
                    for (tool, icon) in [
                        ("Select", "↖"),
                        ("Trim", "⌁"),
                        ("Split", "⫶"),
                        ("Draw", "✎"),
                    ] {
                        let active = self.active_tool == tool;
                        let button = egui::Button::new(
                            RichText::new(format!("{icon}  {tool}"))
                                .size(12.0)
                                .color(if active { ACCENT } else { MUTED }),
                        )
                        .fill(if active { PANEL_ALT } else { BG })
                        .stroke(Stroke::new(1.0_f32, if active { BORDER } else { BG }))
                        .corner_radius(7.0);
                        if ui.add(button).clicked() {
                            self.active_tool = tool;
                            self.notice = format!("{tool} tool selected");
                        }
                    }
                    ui.add_space(14.0);
                    ui.separator();
                    ui.add_space(7.0);
                    let undo = ui.add_enabled(
                        !self.undo.is_empty(),
                        egui::Button::new(RichText::new("↶").size(20.0).color(MUTED))
                            .fill(BG)
                            .stroke(Stroke::NONE),
                    );
                    if undo.clicked() {
                        self.undo();
                    }
                    let redo = ui.add_enabled(
                        !self.redo.is_empty(),
                        egui::Button::new(RichText::new("↷").size(20.0).color(MUTED))
                            .fill(BG)
                            .stroke(Stroke::NONE),
                    );
                    if redo.clicked() {
                        self.redo();
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("−").clicked() {
                            self.set_zoom(self.zoom / 1.2);
                        }
                        if ui.small_button("+").clicked() {
                            self.set_zoom(self.zoom * 1.2);
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
                            if let Some(playback) = &self.playback {
                                playback.set_gain_envelope(self.gain_envelope.clone());
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
                quick_action(
                    ui,
                    "Fade in",
                    "Shape the start",
                    "Fade-in preview only — audio backend not connected",
                    &mut self.notice,
                );
                quick_action(
                    ui,
                    "Fade out",
                    "Shape the end",
                    "Fade-out preview only — audio backend not connected",
                    &mut self.notice,
                );
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
                    for label in ["Denoise", "Gate", "Limit"] {
                        if ui
                            .add(
                                egui::Button::new(RichText::new(label).size(10.0).color(TEXT))
                                    .fill(PANEL_ALT)
                                    .stroke(Stroke::new(1.0_f32, BORDER))
                                    .corner_radius(6.0),
                            )
                            .clicked()
                        {
                            self.notice =
                                format!("{label} preview only — audio backend not connected");
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
                    if transport_button(ui, "⏮").clicked() {
                        if let Some(playback) = &self.playback {
                            playback.seek_frame(0);
                        }
                    }
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(if self.playing { "Ⅱ" } else { "▶" })
                                    .size(15.0)
                                    .color(BG),
                            )
                            .fill(ACCENT)
                            .corner_radius(20.0)
                            .min_size(Vec2::splat(34.0)),
                        )
                        .clicked()
                    {
                        self.toggle_playback();
                    }
                    if transport_button(ui, "⏹").clicked() {
                        self.stop_playback();
                    }
                    if transport_button(ui, "⏭").clicked() {
                        if let Some(playback) = &self.playback {
                            playback.seek_frame(playback.source_frames());
                        }
                    }
                    ui.add_space(13.0);
                    ui.label(
                        RichText::new("●")
                            .size(10.0)
                            .color(Color32::from_rgb(235, 107, 111)),
                    );
                    ui.label(RichText::new("REC").size(10.0).strong().color(MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let audio_info = self.audio.as_ref().map_or_else(
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
                    ui.label(RichText::new("◉").size(12.0).color(ACCENT));
                    if let Some(audio) = &self.audio {
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
                        if let Some(pos) = pointer {
                            let selection_center = (self.selection[0] + self.selection[1]) * 0.5;
                            let handle_x = x_from_time(
                                selection_center,
                                wave_rect,
                                self.view_start,
                                visible_span,
                            );
                            self.gain_dragging = (pos.x - handle_x).abs() < 24.0
                                && (pos.y - gain_handle_y(self.gain_db, wave_rect.center().y)).abs()
                                    < 25.0;
                            if !self.gain_dragging {
                                let t = time_from_x(pos.x, wave_rect, self.view_start, visible_span);
                                self.selection = [t, t];
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
                                let delta = (wave_rect.center().y - pos.y) / 4.0;
                                self.gain_db = delta.clamp(-24.0, 18.0);
                                self.gain_envelope.set_region(
                                    self.selection[0],
                                    self.selection[1],
                                    self.gain_db,
                                );
                                if let Some(playback) = &self.playback {
                                    playback.set_gain_envelope(self.gain_envelope.clone());
                                }
                                self.notice = format!("Gain {:+.1} dB", self.gain_db);
                            } else {
                                let t = time_from_x(pos.x, wave_rect, self.view_start, visible_span);
                                self.selection[1] = t;
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
                        if let Some(pos) = pointer {
                            self.playhead =
                                time_from_x(pos.x, wave_rect, self.view_start, visible_span);
                            if let Some(playback) = &self.playback {
                                playback.seek_frame(
                                    (self.playhead * playback.source_frames() as f32) as u64,
                                );
                            }
                        }
                    }
                    draw_waveform(
                        ui,
                        wave_rect,
                        self.selection,
                        self.gain_db,
                        self.playhead,
                        self.playing,
                        self.audio.as_ref(),
                        &self.gain_envelope,
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
                            self.audio
                                .as_ref()
                                .map_or("Demo waveform", |audio| audio.name.as_str()),
                        )
                        .size(10.0)
                        .color(MUTED),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("＋ Add track").size(10.0).color(MUTED));
                    });
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
        if self.import_receiver.is_some() || self.export_receiver.is_some() {
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

fn draw_waveform(
    ui: &egui::Ui,
    rect: Rect,
    selection: [f32; 2],
    gain_db: f32,
    playhead: f32,
    playing: bool,
    audio: Option<&AudioData>,
    gain_envelope: &GainEnvelope,
    view_start: f32,
    visible_span: f32,
) {
    let painter = ui.painter();
    painter.rect_filled(rect, 5.0, Color32::from_rgb(17, 22, 29));
    let start = selection[0].min(selection[1]);
    let end = selection[0].max(selection[1]);
    let view_end = view_start + visible_span;
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
            rect.top(),
        ),
        Pos2::new(
            x_from_time(visible_end, rect, view_start, visible_span),
            rect.bottom(),
        ),
    );
    if visible_start < visible_end {
        painter.rect_filled(
            selected_rect,
            0.0,
            Color32::from_rgba_unmultiplied(105, 219, 190, 22),
        );
    }

    let split_y = rect.center().y;
    painter.line_segment(
        [
            Pos2::new(rect.left(), split_y),
            Pos2::new(rect.right(), split_y),
        ],
        Stroke::new(1.0_f32, BORDER),
    );
    for lane in 0..2 {
        let center = rect.top() + rect.height() * (lane as f32 + 0.5) * 0.5;
        let amplitude = rect.height() * 0.225;
        let step = (rect.width() / 900.0).max(1.0);
        let mut x = rect.left();
        while x < rect.right() {
            let t = (view_start + (x - rect.left()) / rect.width() * visible_span).clamp(0.0, 1.0);
            let (minimum, maximum) = if let Some(audio) = audio {
                let channel = &audio.peaks[lane.min(audio.channel_count() - 1)];
                let first = (t * channel.len() as f32).floor() as usize;
                let last =
                    (((t + step / rect.width() * visible_span) * channel.len() as f32).ceil()
                        as usize)
                        .max(first + 1)
                        .min(channel.len());
                channel[first.min(channel.len() - 1)..last]
                    .iter()
                    .fold((0.0_f32, 0.0_f32), |(min, max), sample| {
                        (min.min(sample[0]), max.max(sample[1]))
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
            let amplitude = amplitude * db_to_linear(gain_envelope.gain_at(t));
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

    let gain_handle_x = x_from_time((start + end) * 0.5, rect, view_start, visible_span);
    let gain_y = gain_handle_y(gain_db, rect.center().y);
    let mut previous_point = None;
    if has_visible_selection {
        let mut x = selected_rect.left();
        while x <= selected_rect.right() {
            let t = (view_start + (x - rect.left()) / rect.width() * visible_span).clamp(0.0, 1.0);
            let point = Pos2::new(x, gain_handle_y(gain_envelope.gain_at(t), rect.center().y));
            if let Some(previous) = previous_point {
                painter.line_segment([previous, point], Stroke::new(2.0_f32, ORANGE));
            }
            previous_point = Some(point);
            x += 2.0;
        }
    }
    if has_visible_selection {
        painter.circle_filled(Pos2::new(gain_handle_x, gain_y), 7.0, ORANGE);
        painter.circle_stroke(
            Pos2::new(gain_handle_x, gain_y),
            7.0,
            Stroke::new(1.0_f32, TEXT),
        );
    }
    if end > start + 0.01 && gain_handle_x >= rect.left() && gain_handle_x <= rect.right() {
        painter.text(
            Pos2::new(gain_handle_x, gain_y - 19.0),
            egui::Align2::CENTER_BOTTOM,
            format!("{:+.1} dB", gain_db),
            FontId::monospace(10.0),
            ORANGE,
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
                ui.label(RichText::new("↗").size(14.0).color(ACCENT));
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).size(11.0).strong().color(TEXT));
                    ui.label(RichText::new(subtitle).size(9.0).color(MUTED));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new("›").size(16.0).color(MUTED));
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

fn transport_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).size(14.0).color(TEXT))
            .fill(PANEL)
            .stroke(Stroke::NONE)
            .corner_radius(6.0)
            .min_size(Vec2::new(28.0, 30.0)),
    )
}

fn channel_label(channels: usize) -> &'static str {
    match channels {
        1 => "MONO",
        2 => "STEREO",
        _ => "MULTICHANNEL",
    }
}

fn gain_handle_y(gain_db: f32, center_y: f32) -> f32 {
    center_y - gain_db.clamp(-24.0, 18.0) * 4.0
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
            audio: Some(AudioData {
                name: "stereo.wav".to_owned(),
                sample_rate: 48_000,
                peaks: vec![vec![[0.25, 0.75]], vec![[-1.0, -0.5]]],
                samples,
                duration: 2.0 / 48_000.0,
            }),
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
        app.audio.as_mut().unwrap().samples = Arc::clone(&processed_samples);
        app.gain_envelope.set_region(0.25, 0.75, 6.0206);
        app.gain_db = 6.0206;

        app.undo();
        assert_eq!(
            app.audio.as_ref().unwrap().samples.as_ref(),
            original_samples.as_ref()
        );
        assert_eq!(app.gain_envelope.gain_at(0.5), 0.0);
        assert_eq!(app.gain_db, 0.0);

        app.redo();
        assert_eq!(
            app.audio.as_ref().unwrap().samples.as_ref(),
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
        let previous_envelope = app.gain_envelope.clone();

        app.normalize();
        let normalized_envelope = app.gain_envelope.clone();
        assert_ne!(normalized_envelope, previous_envelope);

        app.undo();
        assert_eq!(app.gain_envelope, previous_envelope);
        assert_eq!(
            app.audio.as_ref().unwrap().samples.as_ref(),
            source.as_ref()
        );

        app.redo();
        assert_eq!(app.gain_envelope, normalized_envelope);
        assert_eq!(
            app.audio.as_ref().unwrap().samples.as_ref(),
            source.as_ref()
        );
    }

    #[test]
    fn eq_and_compressor_edits_are_undoable_and_redoable() {
        let source: Arc<[f32]> = Arc::from([0.1, 0.1, 0.8, -0.8, 0.9, -0.9, 0.1, 0.1]);
        let mut app = stereo_app(Arc::clone(&source));
        app.selection = [0.25, 0.75];
        app.equalizer.low_db = 12.0;
        app.apply_eq();
        let eq_samples = Arc::clone(&app.audio.as_ref().unwrap().samples);
        assert_ne!(eq_samples.as_ref(), source.as_ref());
        app.undo();
        assert_eq!(
            app.audio.as_ref().unwrap().samples.as_ref(),
            source.as_ref()
        );
        app.redo();
        assert_eq!(
            app.audio.as_ref().unwrap().samples.as_ref(),
            eq_samples.as_ref()
        );

        app.selection = [0.25, 0.75];
        app.compressor.threshold_db = -12.0;
        app.compressor.ratio = 4.0;
        app.apply_compressor();
        let compressed_samples = Arc::clone(&app.audio.as_ref().unwrap().samples);
        assert_ne!(compressed_samples.as_ref(), eq_samples.as_ref());
        app.undo();
        assert_eq!(
            app.audio.as_ref().unwrap().samples.as_ref(),
            eq_samples.as_ref()
        );
        app.redo();
        assert_eq!(
            app.audio.as_ref().unwrap().samples.as_ref(),
            compressed_samples.as_ref()
        );
    }

    #[test]
    fn zoom_changes_visible_time_range_and_scroll_clamps_to_file_bounds() {
        let mut app = RinaApp::default();
        app.set_zoom(4.0);
        assert_eq!(app.visible_span(), 0.25);
        assert!(app.view_start <= 0.75);
        app.scroll_timeline(0.1);
        assert!((app.view_start - 0.475).abs() < 0.001);
        app.scroll_timeline(2.0);
        assert_eq!(app.view_start, 0.75);
        app.set_zoom(0.0);
        assert_eq!(app.zoom, 1.0);
        assert_eq!(app.visible_span(), 1.0);
        assert_eq!(app.view_start, 0.0);
    }
}
