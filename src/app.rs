use eframe::egui::{self, Color32, RichText};

use crate::{
    model::{Project, TrackKind, Waveform},
    piano_roll,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Arrangement,
    PianoRoll,
}

#[derive(Debug)]
enum ClipDrag {
    Move {
        track_id: u64,
        clip_id: u64,
        origin_x: f32,
        original_start: u16,
    },
    Resize {
        track_id: u64,
        clip_id: u64,
        origin_x: f32,
        original_length: u16,
    },
}

pub struct DawApp {
    project: Project,
    selected_track: Option<u64>,
    view: View,
    playing: bool,
    piano_roll: piano_roll::PianoRoll,
    selected_clip: Option<(u64, u64)>,
    clip_drag: Option<ClipDrag>,
}

impl DawApp {
    pub fn new(context: &eframe::CreationContext<'_>) -> Self {
        context.egui_ctx.set_visuals(egui::Visuals::dark());
        Self {
            project: Project::default(),
            selected_track: Some(1),
            view: View::Arrangement,
            playing: false,
            piano_roll: piano_roll::PianoRoll::default(),
            selected_clip: None,
            clip_drag: None,
        }
    }

    fn selected_track_mut(&mut self) -> Option<&mut crate::model::Track> {
        let selected = self.selected_track?;
        self.project
            .tracks
            .iter_mut()
            .find(|track| track.id == selected)
    }

    fn add_dropped_samples(&mut self, context: &egui::Context) {
        let paths = context.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect::<Vec<_>>()
        });
        for path in paths {
            self.selected_track = Some(self.project.add_sample(path));
            self.view = View::Arrangement;
        }
    }

    fn top_bar(&mut self, root: &mut egui::Ui) {
        egui::Panel::top("transport").show(root, |ui| {
            ui.horizontal(|ui| {
                ui.heading("DON'T TRACK ME");
                ui.separator();
                if ui
                    .button(if self.playing { "■ Stop" } else { "▶ Play" })
                    .clicked()
                {
                    self.playing = !self.playing;
                }
                ui.button("● Record")
                    .on_hover_text("Audio recording is not implemented yet");
                ui.separator();
                ui.label("BPM");
                ui.add(
                    egui::DragValue::new(&mut self.project.bpm)
                        .range(20.0..=300.0)
                        .speed(0.5),
                );
                ui.separator();
                ui.selectable_value(&mut self.view, View::Arrangement, "Arrangement");
                let piano_enabled = self
                    .selected_track_mut()
                    .is_some_and(|track| matches!(track.kind, TrackKind::Instrument { .. }));
                ui.add_enabled_ui(piano_enabled, |ui| {
                    ui.selectable_value(&mut self.view, View::PianoRoll, "Piano roll");
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label("Drop audio files anywhere to create sample tracks");
                });
            });
        });
    }

    fn track_list(&mut self, root: &mut egui::Ui) {
        egui::Panel::left("tracks")
            .default_size(245.0)
            .min_size(220.0)
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Tracks");
                    if ui.button("+ Instrument").clicked() {
                        self.selected_track = Some(self.project.add_instrument());
                    }
                });
                ui.separator();

                for track in &mut self.project.tracks {
                    let selected = self.selected_track == Some(track.id);
                    let icon = match track.kind {
                        TrackKind::Instrument { .. } => "⌁",
                        TrackKind::Sample { .. } => "▰",
                    };
                    egui::Frame::new()
                        .fill(if selected {
                            Color32::from_rgb(47, 65, 72)
                        } else {
                            Color32::TRANSPARENT
                        })
                        .corner_radius(5.0)
                        .inner_margin(6.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if ui
                                    .selectable_label(selected, format!("{icon}  {}", track.name))
                                    .clicked()
                                {
                                    self.selected_track = Some(track.id);
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.toggle_value(&mut track.solo, "S");
                                        ui.toggle_value(&mut track.muted, "M");
                                    },
                                );
                            });
                        });
                    ui.add_space(3.0);
                }

                if self.project.tracks.is_empty() {
                    ui.weak("Add an instrument or drop an audio file here.");
                }
            });
    }

    fn arrangement(&mut self, ui: &mut egui::Ui) {
        const STEPS: u16 = 128;
        const STEP_WIDTH: f32 = 24.0;
        const TRACK_HEIGHT: f32 = 58.0;
        const HANDLE_WIDTH: f32 = 7.0;

        let (duplicate, delete) = ui.input(|input| {
            (
                input.modifiers.command && input.key_pressed(egui::Key::D),
                input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace),
            )
        });
        if let Some((track_id, clip_id)) = self.selected_clip {
            if delete
                && let Some(track) = self
                    .project
                    .tracks
                    .iter_mut()
                    .find(|track| track.id == track_id)
            {
                track.clips.retain(|clip| clip.id != clip_id);
                self.selected_clip = None;
            } else if duplicate
                && let Some(track) = self
                    .project
                    .tracks
                    .iter_mut()
                    .find(|track| track.id == track_id)
                && let Some(clip) = track.clips.iter().find(|clip| clip.id == clip_id).cloned()
            {
                let start = (clip.start_step + clip.length_steps).min(STEPS - 1);
                let id = track.add_clip(start, clip.length_steps.min(STEPS - start));
                self.selected_clip = Some((track_id, id));
            }
        }

        ui.horizontal(|ui| {
            ui.heading("Arrangement");
            ui.separator();
            ui.label("8 bars · 4/4");
            ui.separator();
            ui.weak("Drag clips to move · right edge to resize · double-click pattern to edit · Ctrl/Cmd+D to duplicate");
        });
        ui.add_space(8.0);

        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            ui.set_min_width(180.0 + STEP_WIDTH * f32::from(STEPS));
            ui.horizontal(|ui| {
                ui.add_sized([170.0, 24.0], egui::Label::new(""));
                let (header, _) = ui.allocate_exact_size(
                    egui::vec2(STEP_WIDTH * f32::from(STEPS), 24.0),
                    egui::Sense::hover(),
                );
                for bar in 0..8 {
                    let x = header.left() + bar as f32 * STEP_WIDTH * 16.0;
                    ui.painter().text(
                        egui::pos2(x + 5.0, header.center().y),
                        egui::Align2::LEFT_CENTER,
                        format!("{}", bar + 1),
                        egui::FontId::monospace(12.0),
                        Color32::LIGHT_GRAY,
                    );
                }
            });

            for track in &mut self.project.tracks {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(170.0);
                        ui.label(&track.name);
                        if matches!(track.kind, TrackKind::Instrument { .. })
                            && ui.small_button("+ Pattern clip").clicked()
                        {
                            let start = track
                                .clips
                                .iter()
                                .map(|clip| clip.start_step + clip.length_steps)
                                .max()
                                .unwrap_or(0);
                            if start < STEPS {
                                let id = track.add_clip(start, 16_u16.min(STEPS - start));
                                self.selected_clip = Some((track.id, id));
                            }
                        }
                    });
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(STEP_WIDTH * f32::from(STEPS), TRACK_HEIGHT),
                        egui::Sense::click_and_drag(),
                    );
                    ui.painter()
                        .rect_filled(rect, 3.0, Color32::from_rgb(31, 35, 42));
                    for step in 0..=STEPS {
                        let x = rect.left() + f32::from(step) * STEP_WIDTH;
                        ui.painter().line_segment(
                            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                            egui::Stroke::new(
                                if step % 16 == 0 {
                                    1.5
                                } else if step % 4 == 0 {
                                    1.0
                                } else {
                                    0.5
                                },
                                Color32::from_gray(if step % 16 == 0 { 70 } else { 48 }),
                            ),
                        );
                    }

                    let clip_rect = |start: u16, length: u16| {
                        egui::Rect::from_min_size(
                            egui::pos2(
                                rect.left() + f32::from(start) * STEP_WIDTH + 1.0,
                                rect.top() + 6.0,
                            ),
                            egui::vec2(f32::from(length) * STEP_WIDTH - 2.0, TRACK_HEIGHT - 12.0),
                        )
                    };
                    if response.clicked()
                        && let Some(pointer) = response.interact_pointer_pos()
                    {
                        self.selected_clip = track
                            .clips
                            .iter()
                            .rev()
                            .find(|clip| {
                                clip_rect(clip.start_step, clip.length_steps).contains(pointer)
                            })
                            .map(|clip| (track.id, clip.id));
                    }
                    if response.double_clicked()
                        && let Some(pointer) = response.interact_pointer_pos()
                        && matches!(track.kind, TrackKind::Instrument { .. })
                        && track.clips.iter().any(|clip| {
                            clip_rect(clip.start_step, clip.length_steps).contains(pointer)
                        })
                    {
                        self.selected_track = Some(track.id);
                        self.view = View::PianoRoll;
                    }
                    if response.drag_started()
                        && let Some(pointer) = ui.input(|input| input.pointer.press_origin())
                        && let Some(clip) = track.clips.iter().rev().find(|clip| {
                            clip_rect(clip.start_step, clip.length_steps).contains(pointer)
                        })
                    {
                        self.selected_clip = Some((track.id, clip.id));
                        let clip_area = clip_rect(clip.start_step, clip.length_steps);
                        self.clip_drag = Some(if pointer.x >= clip_area.right() - HANDLE_WIDTH {
                            ClipDrag::Resize {
                                track_id: track.id,
                                clip_id: clip.id,
                                origin_x: pointer.x,
                                original_length: clip.length_steps,
                            }
                        } else {
                            ClipDrag::Move {
                                track_id: track.id,
                                clip_id: clip.id,
                                origin_x: pointer.x,
                                original_start: clip.start_step,
                            }
                        });
                    }
                    if let Some(pointer) = response.interact_pointer_pos() {
                        match &self.clip_drag {
                            Some(ClipDrag::Move {
                                track_id,
                                clip_id,
                                origin_x,
                                original_start,
                            }) if *track_id == track.id => {
                                if let Some(clip) =
                                    track.clips.iter_mut().find(|clip| clip.id == *clip_id)
                                {
                                    let delta =
                                        ((pointer.x - origin_x) / STEP_WIDTH).round() as i32;
                                    clip.start_step = (i32::from(*original_start) + delta)
                                        .clamp(0, i32::from(STEPS - clip.length_steps))
                                        as u16;
                                }
                            }
                            Some(ClipDrag::Resize {
                                track_id,
                                clip_id,
                                origin_x,
                                original_length,
                            }) if *track_id == track.id => {
                                if let Some(clip) =
                                    track.clips.iter_mut().find(|clip| clip.id == *clip_id)
                                {
                                    let delta =
                                        ((pointer.x - origin_x) / STEP_WIDTH).round() as i32;
                                    clip.length_steps = (i32::from(*original_length) + delta)
                                        .clamp(1, i32::from(STEPS - clip.start_step))
                                        as u16;
                                }
                            }
                            _ => {}
                        }
                    }
                    if response.drag_stopped() {
                        self.clip_drag = None;
                    }

                    for clip in &track.clips {
                        let area = clip_rect(clip.start_step, clip.length_steps);
                        let selected = self.selected_clip == Some((track.id, clip.id));
                        let clip_label = match &track.kind {
                            TrackKind::Sample { path } => path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or(&clip.name),
                            TrackKind::Instrument { .. } => &clip.name,
                        };
                        let color = match track.kind {
                            TrackKind::Instrument { .. } => Color32::from_rgb(68, 142, 112),
                            TrackKind::Sample { .. } => Color32::from_rgb(70, 101, 157),
                        };
                        ui.painter().rect_filled(
                            area,
                            4.0,
                            if selected {
                                Color32::from_rgb(226, 151, 61)
                            } else {
                                color
                            },
                        );
                        ui.painter().rect_filled(
                            egui::Rect::from_min_max(
                                egui::pos2(area.right() - HANDLE_WIDTH, area.top()),
                                area.max,
                            ),
                            2.0,
                            Color32::from_black_alpha(55),
                        );
                        ui.painter().text(
                            area.left_top() + egui::vec2(7.0, 6.0),
                            egui::Align2::LEFT_TOP,
                            clip_label,
                            egui::FontId::proportional(12.0),
                            Color32::WHITE,
                        );
                        if matches!(track.kind, TrackKind::Instrument { .. })
                            && !track.notes.is_empty()
                        {
                            let baseline = area.bottom() - 6.0;
                            for note in &track.notes {
                                let x =
                                    area.left() + area.width() * f32::from(note.start_step) / 32.0;
                                if x < area.right() - 3.0 {
                                    let y =
                                        baseline - f32::from(note.pitch.saturating_sub(48)) * 0.65;
                                    ui.painter().line_segment(
                                        [
                                            egui::pos2(x, y),
                                            egui::pos2((x + 5.0).min(area.right() - 2.0), y),
                                        ],
                                        egui::Stroke::new(1.0, Color32::from_white_alpha(170)),
                                    );
                                }
                            }
                        }
                    }
                });
                ui.add_space(4.0);
            }
        });
    }

    fn editor(&mut self, ui: &mut egui::Ui) {
        let Some(selected) = self.selected_track else {
            ui.centered_and_justified(|ui| ui.label("Select a track to edit it."));
            return;
        };
        let Some(index) = self
            .project
            .tracks
            .iter()
            .position(|track| track.id == selected)
        else {
            ui.centered_and_justified(|ui| ui.label("Select a track to edit it."));
            return;
        };
        let track = &mut self.project.tracks[index];

        ui.horizontal(|ui| {
            ui.heading(&track.name);
            ui.separator();
            ui.label("Instrument");
            if let TrackKind::Instrument { waveform } = &mut track.kind {
                egui::ComboBox::from_id_salt("waveform")
                    .selected_text(waveform.name())
                    .show_ui(ui, |ui| {
                        for choice in Waveform::ALL {
                            ui.selectable_value(waveform, choice, choice.name());
                        }
                    });
            }
        });
        ui.separator();
        if matches!(track.kind, TrackKind::Instrument { .. }) {
            self.piano_roll.show(ui, selected, track);
        } else {
            ui.label("Sample tracks do not have a piano roll.");
        }
    }
}

impl eframe::App for DawApp {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = root.ctx().clone();
        self.add_dropped_samples(&context);
        self.top_bar(root);
        self.track_list(root);
        egui::CentralPanel::default().show(root, |ui| match self.view {
            View::Arrangement => self.arrangement(ui),
            View::PianoRoll => self.editor(ui),
        });

        if context.input(|input| !input.raw.hovered_files.is_empty()) {
            let painter = context.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drop-overlay"),
            ));
            let screen = context.content_rect();
            painter.rect_filled(screen, 0.0, Color32::from_black_alpha(210));
            painter.text(
                screen.center(),
                egui::Align2::CENTER_CENTER,
                RichText::new("Drop to add sample track")
                    .size(28.0)
                    .color(Color32::WHITE)
                    .text(),
                egui::FontId::proportional(28.0),
                Color32::WHITE,
            );
        }
    }
}
