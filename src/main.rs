use eframe::egui::{
    self, Align, Color32, FontId, Frame, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, Vec2,
};

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

#[derive(Clone, Copy)]
struct EditState {
    gain_db: f32,
    selection: [f32; 2],
}

struct RinaApp {
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
            selection: self.selection,
        });
        self.redo.clear();
    }

    fn restore(&mut self, state: EditState) {
        self.gain_db = state.gain_db;
        self.selection = state.selection;
    }

    fn undo(&mut self) {
        if let Some(state) = self.undo.pop() {
            self.redo.push(EditState {
                gain_db: self.gain_db,
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
                selection: self.selection,
            });
            self.restore(state);
            self.notice = "Edit restored".to_owned();
        }
    }

    fn update_shortcuts(&mut self, context: &egui::Context) {
        context.input(|input| {
            if input.modifiers.command && input.key_pressed(egui::Key::Z) {
                self.undo();
            } else if input.modifiers.command && input.key_pressed(egui::Key::Y) {
                self.redo();
            } else if input.key_pressed(egui::Key::Space) {
                self.playing = !self.playing;
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
                            self.notice = "File loading is not connected yet".to_owned();
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
                                (self.selection[1] - self.selection[0]) * 42.0
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
                                    selection: self.selection,
                                });
                                self.redo.clear();
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
                time_edit(ui, "Start", &format_time(self.selection[0] * 42.0));
                time_edit(ui, "End", &format_time(self.selection[1] * 42.0));
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
                        RichText::new("00:12.48")
                            .size(13.0)
                            .monospace()
                            .strong()
                            .color(TEXT),
                    );
                    ui.add_space(10.0);
                    transport_button(ui, "⏮");
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
                        self.playing = !self.playing;
                        self.notice = if self.playing {
                            "Playing preview".into()
                        } else {
                            "Playback stopped".into()
                        };
                    }
                    transport_button(ui, "⏹");
                    transport_button(ui, "⏭");
                    ui.add_space(13.0);
                    ui.label(
                        RichText::new("●")
                            .size(10.0)
                            .color(Color32::from_rgb(235, 107, 111)),
                    );
                    ui.label(RichText::new("REC").size(10.0).strong().color(MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new("48 kHz  ·  24-bit  ·  Stereo")
                                .size(10.0)
                                .color(MUTED),
                        );
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
                    ui.label(RichText::new("Room tone — close mic.wav").size(13.0).strong().color(TEXT));
                    ui.label(RichText::new("STEREO").size(9.0).strong().color(MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("44.1 kHz  /  16-bit").size(10.0).color(MUTED));
                        ui.add_space(12.0);
                        ui.label(RichText::new("03:42").size(11.0).monospace().color(TEXT));
                    });
                });
                ui.add_space(10.0);
                Frame::new().fill(PANEL).stroke(Stroke::new(1.0_f32, BORDER)).corner_radius(10.0).show(ui, |ui| {
                    ui.set_min_height(525.0);
                    ui.set_width(ui.available_width());
                    let width = ui.available_width();
                    let timeline_height = 35.0;
                    let waveform_height = (ui.available_height() - timeline_height - 50.0).max(260.0);
                    let (timeline_rect, _) = ui.allocate_exact_size(Vec2::new(width, timeline_height), Sense::hover());
                    draw_timeline(ui, timeline_rect);
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
                                && (pos.y - wave_rect.center().y).abs() < 25.0;
                            if !self.gain_dragging {
                                let t = ((pos.x - wave_rect.left()) / wave_rect.width()).clamp(0.0, 1.0);
                                self.selection = [t, t];
                            }
                        }
                    }
                    if response.dragged() {
                        if let Some(pos) = pointer {
                            if self.gain_dragging {
                                let delta = (wave_rect.center().y - pos.y) / 5.0;
                                self.gain_db = delta.clamp(-24.0, 18.0);
                                self.notice = format!("Gain {:+.1} dB", self.gain_db);
                            } else {
                                let t = ((pos.x - wave_rect.left()) / wave_rect.width()).clamp(0.0, 1.0);
                                self.selection[1] = t;
                            }
                        }
                    }
                    if response.drag_stopped() {
                        self.gain_dragging = false;
                    }
                    if response.clicked() {
                        if let Some(pos) = pointer {
                            self.playhead = ((pos.x - wave_rect.left()) / wave_rect.width()).clamp(0.0, 1.0);
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
                    ui.label(RichText::new("Room tone — close mic.wav").size(10.0).color(MUTED));
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
        self.top_bar(context);
        self.toolbar(context);
        self.bottom_bar(context);
        self.side_panel(context);
        self.editor(context);
        if self.playing {
            self.playhead = (self.playhead + 0.0015).fract();
            context.request_repaint();
        }
    }
}

fn draw_timeline(ui: &egui::Ui, rect: Rect) {
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
            format_time(index as f32 * 5.25),
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
        let amplitude = rect.height() * 0.205 * (1.0 + gain_db / 30.0).clamp(0.2, 1.5);
        let step = (rect.width() / (900.0 * zoom)).max(1.1);
        let mut x = rect.left() + step * 0.5;
        while x < rect.right() {
            let t = (x - rect.left()) / rect.width() * zoom;
            let envelope = (0.25
                + 0.75 * ((t * 5.2).sin().abs() * 0.65 + (t * 12.0 + 1.2).cos().abs() * 0.35))
                * (0.72 + 0.22 * (t * 1.7).sin());
            let detail = ((t * 123.0 + lane as f32 * 0.7).sin() * 0.55
                + (t * 47.0 + 0.4).cos() * 0.3
                + (t * 211.0 + lane as f32).sin() * 0.15)
                .abs();
            let half = amplitude * envelope * (0.35 + detail * 0.65);
            let color = if x >= selected_rect.left() && x <= selected_rect.right() {
                Color32::from_rgb(129, 234, 205)
            } else {
                WAVE
            };
            painter.line_segment(
                [Pos2::new(x, center - half), Pos2::new(x, center + half)],
                Stroke::new(step.min(2.0).max(1.0), color),
            );
            x += step;
        }
    }

    let gain_handle_x = rect.left() + (start + end) * 0.5 * rect.width();
    let gain_y = rect.center().y - (gain_db / 18.0).clamp(-1.0, 1.0) * 20.0;
    painter.line_segment(
        [
            Pos2::new(selected_rect.left(), gain_y),
            Pos2::new(selected_rect.right(), gain_y),
        ],
        Stroke::new(2.0_f32, ORANGE),
    );
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

fn transport_button(ui: &mut egui::Ui, label: &str) {
    ui.add(
        egui::Button::new(RichText::new(label).size(14.0).color(TEXT))
            .fill(PANEL)
            .stroke(Stroke::NONE)
            .corner_radius(6.0)
            .min_size(Vec2::new(28.0, 30.0)),
    );
}

fn format_time(seconds: f32) -> String {
    let minutes = (seconds / 60.0).floor() as u32;
    let remaining = seconds - minutes as f32 * 60.0;
    format!("{minutes:02}:{remaining:05.2}")
}
