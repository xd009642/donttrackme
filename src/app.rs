use std::collections::HashSet;

use eframe::egui::{self, Color32, RichText};

use crate::{
    audio::AudioEngine,
    model::{Clip, ClipSourceKind, Project, TrackKind, Waveform},
    piano_roll,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Arrangement,
    PianoRoll,
    Instrument,
}

const PIANO_KEYS: [(egui::Key, u8); 37] = [
    (egui::Key::Z, 36),
    (egui::Key::S, 37),
    (egui::Key::X, 38),
    (egui::Key::D, 39),
    (egui::Key::C, 40),
    (egui::Key::V, 41),
    (egui::Key::G, 42),
    (egui::Key::B, 43),
    (egui::Key::H, 44),
    (egui::Key::N, 45),
    (egui::Key::J, 46),
    (egui::Key::M, 47),
    (egui::Key::Comma, 48),
    (egui::Key::L, 49),
    (egui::Key::Period, 50),
    (egui::Key::Semicolon, 51),
    (egui::Key::Slash, 52),
    (egui::Key::Q, 60),
    (egui::Key::Num2, 61),
    (egui::Key::W, 62),
    (egui::Key::Num3, 63),
    (egui::Key::E, 64),
    (egui::Key::R, 65),
    (egui::Key::Num5, 66),
    (egui::Key::T, 67),
    (egui::Key::Num6, 68),
    (egui::Key::Y, 69),
    (egui::Key::Num7, 70),
    (egui::Key::U, 71),
    (egui::Key::I, 72),
    (egui::Key::Num9, 73),
    (egui::Key::O, 74),
    (egui::Key::Num0, 75),
    (egui::Key::P, 76),
    (egui::Key::OpenBracket, 77),
    (egui::Key::Equals, 78),
    (egui::Key::CloseBracket, 79),
];

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
    clip_clipboard: Option<Clip>,
    audio: Option<AudioEngine>,
    audio_error: Option<String>,
    auditioned_notes: HashSet<u8>,
}

impl DawApp {
    pub fn new(context: &eframe::CreationContext<'_>) -> Self {
        context.egui_ctx.set_visuals(egui::Visuals::dark());
        let (audio, audio_error) = match AudioEngine::new() {
            Ok(audio) => (Some(audio), None),
            Err(error) => (None, Some(error)),
        };
        Self {
            project: Project::default(),
            selected_track: Some(1),
            view: View::Arrangement,
            playing: false,
            piano_roll: piano_roll::PianoRoll::default(),
            selected_clip: None,
            clip_drag: None,
            clip_clipboard: None,
            audio,
            audio_error,
            auditioned_notes: HashSet::new(),
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

    fn update_keyboard_audition(&mut self, context: &egui::Context) {
        let desired = context.input(|input| {
            if self.view != View::PianoRoll
                || input.modifiers.command
                || input.modifiers.ctrl
                || input.modifiers.alt
            {
                HashSet::new()
            } else {
                PIANO_KEYS
                    .iter()
                    .filter(|(key, _)| input.key_down(*key))
                    .map(|(_, pitch)| *pitch)
                    .collect()
            }
        });
        let synth = self.selected_track.and_then(|selected| {
            self.project
                .tracks
                .iter()
                .find(|track| track.id == selected)
                .and_then(|track| match track.kind {
                    TrackKind::Instrument { synth } => Some(synth),
                    TrackKind::Sample => None,
                })
        });

        if let Some(audio) = &self.audio {
            if let Some(synth) = synth {
                for pitch in desired.difference(&self.auditioned_notes) {
                    if let Err(error) = audio.audition_start(*pitch, synth) {
                        self.audio_error = Some(error);
                    }
                }
            }
            for pitch in self.auditioned_notes.difference(&desired) {
                if let Err(error) = audio.audition_stop(*pitch) {
                    self.audio_error = Some(error);
                }
            }
        }
        self.auditioned_notes = desired;
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
                    let result = if self.playing {
                        self.audio.as_ref().map(AudioEngine::stop)
                    } else {
                        self.audio.as_ref().map(|audio| audio.play(&self.project))
                    };
                    match result {
                        Some(Ok(())) => {
                            self.playing = !self.playing;
                            self.audio_error = None;
                        }
                        Some(Err(error)) => self.audio_error = Some(error),
                        None => {}
                    }
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
                    ui.selectable_value(&mut self.view, View::Instrument, "Instrument");
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(error) = &self.audio_error {
                        ui.colored_label(Color32::from_rgb(245, 115, 105), "Audio unavailable")
                            .on_hover_text(error);
                    } else {
                        ui.label("Drop audio files anywhere to create sample tracks");
                    }
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
                        TrackKind::Sample => "▰",
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
                                        if matches!(track.kind, TrackKind::Instrument { .. })
                                            && ui
                                                .small_button("⚙")
                                                .on_hover_text("Instrument settings")
                                                .clicked()
                                        {
                                            self.selected_track = Some(track.id);
                                            self.view = View::Instrument;
                                        }
                                    },
                                );
                            });
                        });
                    ui.add_space(3.0);
                }

                if self.project.tracks.is_empty() {
                    ui.weak("Add an instrument or drop an audio file here.");
                }

                ui.add_space(12.0);
                ui.separator();
                ui.heading("Clip library");
                ui.weak("Reusable originals");
                let mut add_source = None;
                for source in &self.project.clip_library {
                    ui.horizontal(|ui| {
                        ui.label(match &source.kind {
                            ClipSourceKind::Pattern => "▦",
                            ClipSourceKind::Sample { .. } => "▰",
                        });
                        let details = match &source.kind {
                            ClipSourceKind::Pattern => {
                                format!("Original length: {} steps", source.length_steps)
                            }
                            ClipSourceKind::Sample { path } => format!(
                                "{}\nOriginal length: {} steps",
                                path.display(),
                                source.length_steps
                            ),
                        };
                        ui.label(&source.name).on_hover_text(details);
                        if ui
                            .small_button("+")
                            .on_hover_text("Add to arrangement")
                            .clicked()
                        {
                            add_source = Some((source.track_id, source.id, source.length_steps));
                        }
                    });
                }
                if let Some((track_id, source_id, length)) = add_source
                    && let Some(track) = self
                        .project
                        .tracks
                        .iter_mut()
                        .find(|track| track.id == track_id)
                {
                    let start = track
                        .clips
                        .iter()
                        .map(|clip| clip.start_step + clip.length_steps)
                        .max()
                        .unwrap_or(0);
                    let id = track.add_clip(start.min(127), length.min(128 - start.min(127)));
                    debug_assert_eq!(
                        track.clips.last().map(|clip| clip.source_id),
                        Some(source_id)
                    );
                    self.selected_clip = Some((track_id, id));
                    self.view = View::Arrangement;
                }
            });
    }

    fn arrangement(&mut self, ui: &mut egui::Ui) {
        const STEPS: u16 = 128;
        const STEP_WIDTH: f32 = 24.0;
        const TRACK_HEIGHT: f32 = 58.0;
        const HANDLE_WIDTH: f32 = 7.0;

        let (copy, cut, paste, duplicate, delete) = ui.input(|input| {
            (
                input.modifiers.command && input.key_pressed(egui::Key::C),
                input.modifiers.command && input.key_pressed(egui::Key::X),
                input.modifiers.command && input.key_pressed(egui::Key::V),
                input.modifiers.command && input.key_pressed(egui::Key::D),
                input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace),
            )
        });
        if let Some((track_id, clip_id)) = self.selected_clip {
            if (copy || cut)
                && let Some(track) = self
                    .project
                    .tracks
                    .iter()
                    .find(|track| track.id == track_id)
                && let Some(clip) = track.clips.iter().find(|clip| clip.id == clip_id)
            {
                self.clip_clipboard = Some(clip.clone());
            }
            if (delete || cut)
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
        if paste
            && let Some(copied) = self.clip_clipboard.clone()
            && let Some(track_id) = self
                .project
                .source(copied.source_id)
                .map(|source| source.track_id)
            && let Some(track) = self
                .project
                .tracks
                .iter_mut()
                .find(|track| track.id == track_id)
        {
            let start = (copied.start_step + copied.length_steps).min(STEPS - 1);
            let id = track.add_clip(start, copied.length_steps.min(STEPS - start));
            let pasted = track
                .clips
                .last_mut()
                .expect("add_clip just inserted a clip");
            pasted.source_id = copied.source_id;
            self.selected_clip = Some((track_id, id));
        }

        ui.horizontal(|ui| {
            ui.heading("Arrangement");
            ui.separator();
            ui.label("8 bars · 4/4");
            ui.separator();
            ui.weak("Drag to move · right edge to trim · double-click pattern to edit · Ctrl/Cmd+C, X, V, D");
        });
        ui.add_space(8.0);

        let clip_library = self.project.clip_library.clone();
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
                                let id = track.add_clip(start, 32_u16.min(STEPS - start));
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
                        let source = clip_library
                            .iter()
                            .find(|source| source.id == clip.source_id)
                            .expect("every clip instance references a library source");
                        let color = match track.kind {
                            TrackKind::Instrument { .. } => Color32::from_rgb(68, 142, 112),
                            TrackKind::Sample => Color32::from_rgb(70, 101, 157),
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
                            &source.name,
                            egui::FontId::proportional(12.0),
                            Color32::WHITE,
                        );
                        if matches!(track.kind, TrackKind::Instrument { .. })
                            && !track.notes.is_empty()
                        {
                            let baseline = area.bottom() - 6.0;
                            for note in &track.notes {
                                let x = area.left() + f32::from(note.start_step) * STEP_WIDTH;
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
            if let TrackKind::Instrument { synth } = &mut track.kind {
                egui::ComboBox::from_id_salt("waveform")
                    .selected_text(synth.waveform.name())
                    .show_ui(ui, |ui| {
                        for choice in Waveform::ALL {
                            ui.selectable_value(&mut synth.waveform, choice, choice.name());
                        }
                    });
                if ui.button("⚙ Settings").clicked() {
                    self.view = View::Instrument;
                }
            }
        });
        ui.separator();
        if matches!(track.kind, TrackKind::Instrument { .. }) {
            self.piano_roll.show(ui, selected, track);
        } else {
            ui.label("Sample tracks do not have a piano roll.");
        }
    }

    fn instrument_settings(&mut self, ui: &mut egui::Ui) {
        let Some(selected) = self.selected_track else {
            ui.centered_and_justified(|ui| ui.label("Select an instrument track."));
            return;
        };
        let Some(track) = self
            .project
            .tracks
            .iter_mut()
            .find(|track| track.id == selected)
        else {
            return;
        };
        let TrackKind::Instrument { synth } = &mut track.kind else {
            ui.centered_and_justified(|ui| ui.label("The selected track is not an instrument."));
            return;
        };

        ui.horizontal(|ui| {
            ui.heading("Simple waveform");
            ui.separator();
            ui.label(&track.name);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Open piano roll").clicked() {
                    self.view = View::PianoRoll;
                }
            });
        });
        ui.add_space(12.0);

        egui::Frame::group(ui.style())
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_max_width(720.0);
                ui.heading("Oscillator");
                ui.horizontal(|ui| {
                    for waveform in Waveform::ALL {
                        ui.selectable_value(&mut synth.waveform, waveform, waveform.name());
                    }
                });
                ui.add_space(10.0);
                waveform_preview(
                    ui,
                    synth.waveform,
                    synth.level,
                    synth.attack_ms,
                    synth.release_ms,
                );
                ui.add_space(12.0);
                ui.label("Output level");
                ui.add(egui::Slider::new(&mut synth.level, 0.0..=1.0).show_value(true));

                ui.add_space(14.0);
                ui.heading("Envelope");
                ui.columns(2, |columns| {
                    columns[0].label("Attack");
                    columns[0].add(
                        egui::Slider::new(&mut synth.attack_ms, 0.0..=2_000.0)
                            .logarithmic(true)
                            .suffix(" ms"),
                    );
                    columns[1].label("Release");
                    columns[1].add(
                        egui::Slider::new(&mut synth.release_ms, 5.0..=5_000.0)
                            .logarithmic(true)
                            .suffix(" ms"),
                    );
                });
            });
    }
}

fn waveform_preview(
    ui: &mut egui::Ui,
    waveform: Waveform,
    level: f32,
    attack_ms: f32,
    release_ms: f32,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(680.0, 170.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 5.0, Color32::from_rgb(22, 26, 32));
    ui.painter().line_segment(
        [rect.left_center(), rect.right_center()],
        egui::Stroke::new(1.0, Color32::from_gray(55)),
    );
    let held_ms = 500.0;
    let preview_ms = attack_ms + held_ms + release_ms;
    let note_off_ms = attack_ms + held_ms;
    let mut envelope_points = Vec::with_capacity(257);
    let points = (0..=256)
        .map(|index| {
            let progress = index as f32 / 256.0;
            let time_ms = progress * preview_ms;
            let envelope = if time_ms < attack_ms && attack_ms > 0.0 {
                time_ms / attack_ms
            } else if time_ms <= note_off_ms {
                1.0
            } else if release_ms > 0.0 {
                1.0 - (time_ms - note_off_ms) / release_ms
            } else {
                0.0
            };
            let phase = progress * 12.0;
            let value = (index as u32)
                .wrapping_mul(747_796_405)
                .wrapping_add(2_891_336_453);
            let noise = (value as f32 / u32::MAX as f32) * 2.0 - 1.0;
            let sample = waveform.sample(phase, noise) * envelope;
            envelope_points.push(egui::pos2(
                rect.left() + rect.width() * progress,
                rect.center().y - envelope * level * rect.height() * 0.42,
            ));
            egui::pos2(
                rect.left() + rect.width() * progress,
                rect.center().y - sample * level * rect.height() * 0.42,
            )
        })
        .collect::<Vec<_>>();
    ui.painter().add(egui::Shape::line(
        points,
        egui::Stroke::new(2.0, Color32::from_rgb(98, 220, 168)),
    ));
    ui.painter().add(egui::Shape::line(
        envelope_points,
        egui::Stroke::new(1.0, Color32::from_white_alpha(90)),
    ));
}

impl eframe::App for DawApp {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = root.ctx().clone();
        self.add_dropped_samples(&context);
        self.update_keyboard_audition(&context);
        self.top_bar(root);
        self.track_list(root);
        egui::CentralPanel::default().show(root, |ui| match self.view {
            View::Arrangement => self.arrangement(ui),
            View::PianoRoll => self.editor(ui),
            View::Instrument => self.instrument_settings(ui),
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
