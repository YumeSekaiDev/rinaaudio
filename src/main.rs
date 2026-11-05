use eframe::egui::{
    self, Align, Color32, FontId, Frame, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, Vec2,
};
use std::{
    sync::mpsc::{self, Receiver, TryRecvError},
    time::Duration,
};

mod audio;
mod editing;
mod playback;

use audio::{AudioData, decode_audio};
use editing::{GainEnvelope, db_to_linear};
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
}

struct RinaApp {
    audio: Option<AudioData>,
    playback: Option<Playback>,
    import_receiver: Option<Receiver<Result<AudioData, String>>>,
    gain_envelope: GainEnvelope,
    selection: [f32; 2],
    gain_db: f32,
    gain_dragging: bool,
    playing: bool,
    zoom: f32,
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
            gain_envelope: GainEnvelope::default(),
            selection: [0.18, 0.63],
            gain_db: 0.0,
            gain_dragging: false,
            playing: false,
            zoom: 1.0,
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
        self.undo.push(EditState {
            gain_db: self.gain_db,
            gain_envelope: self.gain_envelope.clone(),
            selection: self.selection,
        });
        self.redo.clear();
    }

    fn restore(&mut self, state: EditState) {
        self.gain_db = state.gain_db;
        self.selection = state.selection;
        self.gain_envelope = state.gain_envelope;
        if let Some(playback) = &self.playback {
            playback.set_gain_envelope(self.gain_envelope.clone());
        }
    }

    fn undo(&mut self) {
        if let Some(state) = self.undo.pop() {
            self.redo.push(EditState {
                gain_db: self.gain_db,
                gain_envelope: self.gain_envelope.clone(),
                selection: self.selection,
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
            });
            self.restore(state);
            self.notice = "Edit restored".to_owned();
        }
    }

    fn duration(&self) -> f32 {
        self.audio.as_ref().map_or(42.0, |audio| audio.duration)
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
                self.zoom = (self.zoom * 1.2).min(8.0);
            } else if input.key_pressed(egui::Key::Minus) {
                self.zoom = (self.zoom / 1.2).max(0.5);
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
                            self.notice = "Export is not connected to an audio backend".to_owned();
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
                        ui.label(RichText::new("100%").size(11.0).monospace().color(MUTED));
                        ui.add_space(6.0);
                        if ui.small_button("−").clicked() {
                            self.zoom = (self.zoom / 1.2).max(0.5);
                        }
                        if ui.small_button("+").clicked() {
                            self.zoom = (self.zoom * 1.2).min(8.0);
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
                                self.undo.push(EditState {
                                    gain_db: previous_gain,
                                    gain_envelope: self.gain_envelope.clone(),
                                    selection: self.selection,
                                });
                                self.redo.clear();
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
                quick_action(
                    ui,
                    "Normalize",
                    "Set peak to −1 dB",
                    "Normalize preview only — audio backend not connected",
                    &mut self.notice,
                );
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
                    for label in ["EQ", "Denoise", "Gate", "Compress", "Limit"] {
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
                    draw_timeline(ui, timeline_rect, self.duration());
                    ui.add_space(4.0);

                    let (wave_rect, response) = ui.allocate_exact_size(
                        Vec2::new(width, waveform_height),
                        Sense::click_and_drag(),
                    );
                    let pointer = response.interact_pointer_pos();
                    if response.drag_started() {
                        self.save_edit();
                        if let Some(pos) = pointer {
                            let handle_x = wave_rect.left() + self.selection[0] * wave_rect.width()
                                + (self.selection[1] - self.selection[0]) * wave_rect.width() * 0.5;
                            self.gain_dragging = (pos.x - handle_x).abs() < 24.0
                                && (pos.y - gain_handle_y(self.gain_db, wave_rect.center().y)).abs()
                                    < 25.0;
                            if !self.gain_dragging {
                                let t = ((pos.x - wave_rect.left()) / wave_rect.width()).clamp(0.0, 1.0);
                                self.selection = [t, t];
                                self.gain_db = self.gain_envelope.gain_at(t);
                            }
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
                                let t = ((pos.x - wave_rect.left()) / wave_rect.width()).clamp(0.0, 1.0);
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
                            self.playhead = ((pos.x - wave_rect.left()) / wave_rect.width()).clamp(0.0, 1.0);
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
                        self.zoom,
                        self.playing,
                        self.audio.as_ref(),
                        &self.gain_envelope,
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
        self.top_bar(context);
        self.toolbar(context);
        self.bottom_bar(context);
        self.side_panel(context);
        self.editor(context);
        if self.playing {
            context.request_repaint();
        }
        if self.import_receiver.is_some() {
            context.request_repaint_after(Duration::from_millis(100));
        }
    }
}

fn draw_timeline(ui: &egui::Ui, rect: Rect, duration: f32) {
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
            format_time(index as f32 * duration / 8.0),
            FontId::monospace(9.0),
            MUTED,
        );
    }
}

fn draw_waveform(
    ui: &egui::Ui,
    rect: Rect,
    selection: [f32; 2],
    gain_db: f32,
    playhead: f32,
    zoom: f32,
    playing: bool,
    audio: Option<&AudioData>,
    gain_envelope: &GainEnvelope,
) {
    let painter = ui.painter();
    painter.rect_filled(rect, 5.0, Color32::from_rgb(17, 22, 29));
    let start = selection[0].min(selection[1]);
    let end = selection[0].max(selection[1]);
    let selected_rect = Rect::from_min_max(
        Pos2::new(rect.left() + start * rect.width(), rect.top()),
        Pos2::new(
            rect.left() + end.max(start + 0.001) * rect.width(),
            rect.bottom(),
        ),
    );
    painter.rect_filled(
        selected_rect,
        0.0,
        Color32::from_rgba_unmultiplied(105, 219, 190, 22),
    );

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
        let step = (rect.width() / (900.0 * zoom)).max(1.1);
        let mut x = rect.left() + step * 0.5;
        while x < rect.right() {
            let t = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let (minimum, maximum) = if let Some(audio) = audio {
                let channel = &audio.peaks[lane.min(audio.channel_count() - 1)];
                let first = (t * channel.len() as f32).floor() as usize;
                let last = (((t + step / rect.width()) * channel.len() as f32).ceil() as usize)
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
            let color = if x >= selected_rect.left() && x <= selected_rect.right() {
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

    let gain_handle_x = rect.left() + (start + end) * 0.5 * rect.width();
    let gain_y = gain_handle_y(gain_db, rect.center().y);
    let mut previous_point = None;
    let mut x = selected_rect.left();
    while x <= selected_rect.right() {
        let t = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let point = Pos2::new(x, gain_handle_y(gain_envelope.gain_at(t), rect.center().y));
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
    if end > start + 0.01 {
        painter.text(
            Pos2::new(gain_handle_x, gain_y - 19.0),
            egui::Align2::CENTER_BOTTOM,
            format!("{:+.1} dB", gain_db),
            FontId::monospace(10.0),
            ORANGE,
        );
    }

    let play_x = rect.left() + playhead * rect.width();
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
) {
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
    if ui
        .interact(rect, ui.id().with(title), Sense::click())
        .clicked()
    {
        *notice = message.to_owned();
    }
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
