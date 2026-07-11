use std::collections::HashSet;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::model::{Note, Track};

const KEY_HEIGHT: f32 = 22.0;
const KEYBOARD_WIDTH: f32 = 78.0;
const STEP_WIDTH: f32 = 36.0;
const VELOCITY_HEIGHT: f32 = 100.0;
const LOWEST_PITCH: u8 = 12;
const PITCH_COUNT: u8 = 121;
const STEPS: u16 = 32;
const RESIZE_HANDLE_WIDTH: f32 = 7.0;

#[derive(Debug)]
enum Drag {
    Move {
        origin: Pos2,
        notes: Vec<(u64, u8, u16)>,
    },
    Resize {
        origin_x: f32,
        note_id: u64,
        original_length: u16,
    },
    Marquee {
        origin: Pos2,
        additive: bool,
    },
    Velocity {
        note_id: u64,
    },
}

#[derive(Default)]
pub struct PianoRoll {
    selected: HashSet<u64>,
    clipboard: Vec<Note>,
    drag: Option<Drag>,
    track_id: Option<u64>,
    scroll_to_middle_c: bool,
    mouse_pitch: Option<u8>,
}

#[derive(Default)]
pub struct PianoRollOutput {
    pub note_on: Option<u8>,
    pub note_off: Option<u8>,
}

impl PianoRoll {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        track_id: u64,
        track: &mut Track,
        auditioned_notes: &HashSet<u8>,
    ) -> PianoRollOutput {
        let mut output = PianoRollOutput::default();
        if self.track_id != Some(track_id) {
            self.selected.clear();
            self.drag = None;
            self.track_id = Some(track_id);
            self.scroll_to_middle_c = true;
        }
        if ui.input(|input| input.pointer.primary_released())
            && let Some(pitch) = self.mouse_pitch.take()
        {
            output.note_off = Some(pitch);
        }

        self.keyboard_shortcuts(ui, track);
        ui.horizontal(|ui| {
            ui.label(
                "Drag empty space to select. Drag notes to move; drag their right edge to resize.",
            );
            ui.separator();
            ui.weak("Ctrl/Cmd+C, X, V · Delete");
        });
        ui.weak("Play: Z…/ and Q…] are consecutive white keys · / = B3 · Q = C4");

        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            let grid_height = KEY_HEIGHT * f32::from(PITCH_COUNT);
            let size = Vec2::new(
                KEYBOARD_WIDTH + STEP_WIDTH * f32::from(STEPS),
                grid_height + VELOCITY_HEIGHT + 28.0,
            );
            let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
            let rect = response.rect;
            let grid = Rect::from_min_size(
                Pos2::new(rect.left() + KEYBOARD_WIDTH, rect.top()),
                Vec2::new(STEP_WIDTH * f32::from(STEPS), grid_height),
            );
            if self.scroll_to_middle_c {
                let c4_row = PITCH_COUNT - 1 - (60 - LOWEST_PITCH);
                let target = Rect::from_min_size(
                    Pos2::new(grid.left(), grid.top() + f32::from(c4_row) * KEY_HEIGHT),
                    Vec2::new(grid.width(), KEY_HEIGHT),
                );
                ui.scroll_to_rect(target, Some(egui::Align::Center));
                self.scroll_to_middle_c = false;
            }
            let velocity = Rect::from_min_max(
                Pos2::new(grid.left(), grid.bottom() + 28.0),
                Pos2::new(grid.right(), grid.bottom() + 28.0 + VELOCITY_HEIGHT),
            );
            let keyboard = Rect::from_min_size(rect.min, Vec2::new(KEYBOARD_WIDTH, grid_height));
            if ui.input(|input| input.pointer.primary_pressed())
                && let Some(pointer) = ui.input(|input| input.pointer.press_origin())
                && keyboard.contains(pointer)
            {
                let row = ((pointer.y - keyboard.top()) / KEY_HEIGHT).floor() as u8;
                let pitch = LOWEST_PITCH + PITCH_COUNT - 1 - row;
                self.mouse_pitch = Some(pitch);
                output.note_on = Some(pitch);
            }

            self.handle_click(ui, &response, grid, velocity, track);
            self.begin_drag(ui, &response, grid, velocity, track);
            self.update_drag(&response, grid, velocity, track);
            self.paint_keyboard(&painter, rect, track, auditioned_notes);
            self.paint_grid(&painter, grid);
            self.paint_notes(&painter, grid, track);
            self.paint_velocity(&painter, velocity, track);

            if let Some(Drag::Marquee { origin, .. }) = self.drag
                && let Some(pointer) = response.interact_pointer_pos()
            {
                let selection = Rect::from_two_pos(origin, pointer).intersect(grid);
                painter.rect_filled(selection, 0.0, Color32::from_white_alpha(20));
                painter.rect_stroke(
                    selection,
                    0.0,
                    Stroke::new(1.0, Color32::from_rgb(125, 205, 255)),
                    StrokeKind::Inside,
                );
            }

            if response.drag_stopped() {
                self.drag = None;
            }
        });
        output
    }

    fn handle_click(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        grid: Rect,
        velocity: Rect,
        track: &mut Track,
    ) {
        if response.clicked_by(egui::PointerButton::Secondary) {
            if let Some(pointer) = response.interact_pointer_pos()
                && let Some(note_id) = track
                    .notes
                    .iter()
                    .rev()
                    .find(|note| note_rect(grid, note).contains(pointer))
                    .map(|note| note.id)
            {
                track.notes.retain(|note| note.id != note_id);
                self.selected.remove(&note_id);
            }
            return;
        }
        if !response.clicked() {
            return;
        }
        let Some(pointer) = response.interact_pointer_pos() else {
            return;
        };
        let additive = ui.input(|input| input.modifiers.command || input.modifiers.shift);
        if velocity.contains(pointer) {
            if let Some(note) = track.notes.iter_mut().min_by(|left, right| {
                let left_x = grid.left() + (f32::from(left.start_step) + 0.5) * STEP_WIDTH;
                let right_x = grid.left() + (f32::from(right.start_step) + 0.5) * STEP_WIDTH;
                (left_x - pointer.x)
                    .abs()
                    .total_cmp(&(right_x - pointer.x).abs())
            }) {
                note.velocity = (((velocity.bottom() - pointer.y) / velocity.height()) * 127.0)
                    .round()
                    .clamp(1.0, 127.0) as u8;
                self.selected.clear();
                self.selected.insert(note.id);
            }
            return;
        }
        if let Some(note_id) = track
            .notes
            .iter()
            .rev()
            .find(|note| note_rect(grid, note).contains(pointer))
            .map(|note| note.id)
        {
            if additive && self.selected.contains(&note_id) {
                self.selected.remove(&note_id);
            } else {
                if !additive {
                    self.selected.clear();
                }
                self.selected.insert(note_id);
            }
        } else if grid.contains(pointer) {
            let step = ((pointer.x - grid.left()) / STEP_WIDTH).floor() as u16;
            let row = ((pointer.y - grid.top()) / KEY_HEIGHT).floor() as u8;
            let pitch = LOWEST_PITCH + PITCH_COUNT - 1 - row;
            let id = track.add_note(pitch, step, 1, 100);
            track.ensure_pattern_clip();
            self.selected.clear();
            self.selected.insert(id);
        }
    }

    fn keyboard_shortcuts(&mut self, ui: &mut egui::Ui, track: &mut Track) {
        let (copy, cut, paste, delete) = ui.input(|input| {
            (
                input.modifiers.command && input.key_pressed(egui::Key::C),
                input.modifiers.command && input.key_pressed(egui::Key::X),
                input.modifiers.command && input.key_pressed(egui::Key::V),
                input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace),
            )
        });
        if copy || cut {
            self.clipboard = track
                .notes
                .iter()
                .filter(|note| self.selected.contains(&note.id))
                .copied()
                .collect();
        }
        if cut || delete {
            track.notes.retain(|note| !self.selected.contains(&note.id));
            self.selected.clear();
        }
        if paste && !self.clipboard.is_empty() {
            let first_step = self
                .clipboard
                .iter()
                .map(|note| note.start_step)
                .min()
                .expect("a non-empty clipboard has a first note");
            self.selected.clear();
            for note in &self.clipboard {
                let start = note.start_step - first_step + 4;
                if start < STEPS {
                    let id = track.add_note(
                        note.pitch,
                        start,
                        note.length_steps.min(STEPS - start),
                        note.velocity,
                    );
                    self.selected.insert(id);
                }
            }
            track.ensure_pattern_clip();
        }
    }

    fn begin_drag(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        grid: Rect,
        velocity: Rect,
        track: &Track,
    ) {
        if !response.drag_started() {
            return;
        }
        let Some(pointer) = ui.input(|input| input.pointer.press_origin()) else {
            return;
        };
        let additive = ui.input(|input| input.modifiers.command || input.modifiers.shift);

        if velocity.contains(pointer)
            && let Some(note) = track.notes.iter().min_by(|left, right| {
                let left_x = grid.left() + (f32::from(left.start_step) + 0.5) * STEP_WIDTH;
                let right_x = grid.left() + (f32::from(right.start_step) + 0.5) * STEP_WIDTH;
                (left_x - pointer.x)
                    .abs()
                    .total_cmp(&(right_x - pointer.x).abs())
            })
        {
            if !self.selected.contains(&note.id) {
                self.selected.clear();
                self.selected.insert(note.id);
            }
            self.drag = Some(Drag::Velocity { note_id: note.id });
            return;
        }

        if let Some(note) = track
            .notes
            .iter()
            .rev()
            .find(|note| note_rect(grid, note).contains(pointer))
        {
            let rect = note_rect(grid, note);
            if !self.selected.contains(&note.id) {
                if !additive {
                    self.selected.clear();
                }
                self.selected.insert(note.id);
            } else if additive {
                self.selected.remove(&note.id);
                return;
            }
            if pointer.x >= rect.right() - RESIZE_HANDLE_WIDTH {
                self.drag = Some(Drag::Resize {
                    origin_x: pointer.x,
                    note_id: note.id,
                    original_length: note.length_steps,
                });
            } else {
                self.drag = Some(Drag::Move {
                    origin: pointer,
                    notes: track
                        .notes
                        .iter()
                        .filter(|note| self.selected.contains(&note.id))
                        .map(|note| (note.id, note.pitch, note.start_step))
                        .collect(),
                });
            }
        } else if grid.contains(pointer) {
            if !additive {
                self.selected.clear();
            }
            self.drag = Some(Drag::Marquee {
                origin: pointer,
                additive,
            });
        }
    }

    fn update_drag(
        &mut self,
        response: &egui::Response,
        grid: Rect,
        velocity: Rect,
        track: &mut Track,
    ) {
        let Some(pointer) = response.interact_pointer_pos() else {
            return;
        };
        match &self.drag {
            Some(Drag::Move { origin, notes }) => {
                let step_delta = ((pointer.x - origin.x) / STEP_WIDTH).round() as i32;
                let pitch_delta = -((pointer.y - origin.y) / KEY_HEIGHT).round() as i32;
                for note in &mut track.notes {
                    if let Some((_, pitch, start)) = notes.iter().find(|(id, _, _)| *id == note.id)
                    {
                        note.start_step = (i32::from(*start) + step_delta)
                            .clamp(0, i32::from(STEPS - note.length_steps))
                            as u16;
                        note.pitch = (i32::from(*pitch) + pitch_delta).clamp(
                            i32::from(LOWEST_PITCH),
                            i32::from(LOWEST_PITCH + PITCH_COUNT - 1),
                        ) as u8;
                    }
                }
            }
            Some(Drag::Resize {
                origin_x,
                note_id,
                original_length,
            }) => {
                if let Some(note) = track.notes.iter_mut().find(|note| note.id == *note_id) {
                    let delta = ((pointer.x - origin_x) / STEP_WIDTH).round() as i32;
                    note.length_steps = (i32::from(*original_length) + delta)
                        .clamp(1, i32::from(STEPS - note.start_step))
                        as u16;
                }
            }
            Some(Drag::Marquee { origin, additive }) => {
                if !additive {
                    self.selected.clear();
                }
                let selection = Rect::from_two_pos(*origin, pointer).intersect(grid);
                self.selected.extend(
                    track
                        .notes
                        .iter()
                        .filter(|note| selection.intersects(note_rect(grid, note)))
                        .map(|note| note.id),
                );
            }
            Some(Drag::Velocity { note_id }) => {
                let value = (((velocity.bottom() - pointer.y) / velocity.height()) * 127.0)
                    .round()
                    .clamp(1.0, 127.0) as u8;
                if let Some(note) = track.notes.iter_mut().find(|note| note.id == *note_id) {
                    note.velocity = value;
                }
            }
            None => {}
        }
    }

    fn paint_keyboard(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        track: &Track,
        auditioned_notes: &HashSet<u8>,
    ) {
        let selected_pitches = track
            .notes
            .iter()
            .filter(|note| self.selected.contains(&note.id))
            .map(|note| note.pitch)
            .collect::<HashSet<_>>();
        for row in 0..PITCH_COUNT {
            let pitch = LOWEST_PITCH + PITCH_COUNT - 1 - row;
            let key = Rect::from_min_size(
                Pos2::new(rect.left(), rect.top() + f32::from(row) * KEY_HEIGHT),
                Vec2::new(KEYBOARD_WIDTH, KEY_HEIGHT),
            );
            let black = matches!(pitch % 12, 1 | 3 | 6 | 8 | 10);
            let playing = auditioned_notes.contains(&pitch) || self.mouse_pitch == Some(pitch);
            let fill = if playing {
                Color32::from_rgb(68, 164, 119)
            } else if selected_pitches.contains(&pitch) {
                Color32::from_rgb(205, 133, 48)
            } else {
                Color32::from_gray(if black { 45 } else { 210 })
            };
            painter.rect_filled(key, 0.0, fill);
            painter.rect_stroke(
                key,
                0.0,
                Stroke::new(1.0, Color32::from_gray(80)),
                StrokeKind::Inside,
            );
            painter.text(
                key.center(),
                egui::Align2::CENTER_CENTER,
                note_name(pitch),
                egui::FontId::monospace(11.0),
                if black || playing || selected_pitches.contains(&pitch) {
                    Color32::WHITE
                } else {
                    Color32::BLACK
                },
            );
        }
    }

    fn paint_grid(&self, painter: &egui::Painter, grid: Rect) {
        painter.rect_filled(grid, 0.0, Color32::from_rgb(28, 31, 38));
        for step in 0..=STEPS {
            let x = grid.left() + f32::from(step) * STEP_WIDTH;
            let beat = step % 4 == 0;
            painter.line_segment(
                [Pos2::new(x, grid.top()), Pos2::new(x, grid.bottom())],
                Stroke::new(
                    if beat { 1.5 } else { 0.5 },
                    Color32::from_gray(if beat { 85 } else { 52 }),
                ),
            );
        }
        for row in 0..=PITCH_COUNT {
            let y = grid.top() + f32::from(row) * KEY_HEIGHT;
            painter.line_segment(
                [Pos2::new(grid.left(), y), Pos2::new(grid.right(), y)],
                Stroke::new(0.5, Color32::from_gray(55)),
            );
        }
    }

    fn paint_notes(&self, painter: &egui::Painter, grid: Rect, track: &Track) {
        for note in &track.notes {
            let rect = note_rect(grid, note);
            let selected = self.selected.contains(&note.id);
            painter.rect_filled(
                rect,
                3.0,
                if selected {
                    Color32::from_rgb(245, 173, 75)
                } else {
                    Color32::from_rgb(98, 200, 155)
                },
            );
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(rect.right() - RESIZE_HANDLE_WIDTH, rect.top()),
                    rect.max,
                ),
                2.0,
                Color32::from_black_alpha(45),
            );
            painter.rect_stroke(
                rect,
                3.0,
                Stroke::new(1.0, Color32::WHITE),
                StrokeKind::Inside,
            );
        }
    }

    fn paint_velocity(&self, painter: &egui::Painter, velocity: Rect, track: &Track) {
        painter.text(
            Pos2::new(velocity.left() - 8.0, velocity.top()),
            egui::Align2::RIGHT_TOP,
            "Velocity",
            egui::FontId::proportional(12.0),
            Color32::LIGHT_GRAY,
        );
        painter.rect_filled(velocity, 0.0, Color32::from_rgb(24, 27, 33));
        painter.line_segment(
            [velocity.left_top(), velocity.right_top()],
            Stroke::new(1.0, Color32::from_gray(75)),
        );
        for note in &track.notes {
            let x = velocity.left() + (f32::from(note.start_step) + 0.5) * STEP_WIDTH;
            let top = velocity.bottom() - velocity.height() * f32::from(note.velocity) / 127.0;
            let color = if self.selected.contains(&note.id) {
                Color32::from_rgb(245, 173, 75)
            } else {
                Color32::from_rgb(98, 200, 155)
            };
            painter.line_segment(
                [Pos2::new(x, velocity.bottom()), Pos2::new(x, top)],
                Stroke::new(3.0, color),
            );
            painter.circle_filled(Pos2::new(x, top), 4.0, color);
        }
    }
}

fn note_rect(grid: Rect, note: &Note) -> Rect {
    let row = PITCH_COUNT - 1 - (note.pitch - LOWEST_PITCH);
    Rect::from_min_size(
        Pos2::new(
            grid.left() + f32::from(note.start_step) * STEP_WIDTH + 1.0,
            grid.top() + f32::from(row) * KEY_HEIGHT + 2.0,
        ),
        Vec2::new(
            f32::from(note.length_steps) * STEP_WIDTH - 2.0,
            KEY_HEIGHT - 4.0,
        ),
    )
}

fn note_name(pitch: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", NAMES[usize::from(pitch % 12)], pitch / 12 - 1)
}

#[cfg(test)]
mod tests {
    use super::note_name;

    #[test]
    fn extended_keyboard_uses_requested_octave_bounds() {
        assert_eq!(note_name(12), "C0");
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(132), "C10");
    }
}
