use std::{
    collections::HashSet,
    path::PathBuf,
    time::{Duration, Instant},
};

use eframe::egui::{self, Color32, RichText};

use crate::{
    audio::{self, AudioEngine},
    model::{
        ARRANGEMENT_STEPS, AutomationLane, AutomationParameter, AutomationPoint, AutomationValue,
        Clip, ClipSourceKind, EffectKind, FilterKind, PATTERN_STEPS, Pattern, Project,
        STEPS_PER_BAR, STEPS_PER_BEAT, SampleLoopMode, SampleRegion, SampleSynth,
        SimpleWaveformSynth, TrackKind, Waveform, noise_sample,
    },
    piano_roll, project_io,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Arrangement,
    PianoRoll,
    Instrument,
}

#[derive(Default)]
struct TapTempo {
    open: bool,
    taps: Vec<Instant>,
    bpm: Option<f32>,
    key_was_down: bool,
}

impl TapTempo {
    fn record(&mut self, now: Instant) -> Option<f32> {
        if self
            .taps
            .last()
            .is_some_and(|previous| now.duration_since(*previous) > Duration::from_secs(3))
        {
            self.taps.clear();
        }
        self.taps.push(now);
        self.bpm = if self.taps.len() >= 2 {
            let elapsed = now.duration_since(self.taps[0]).as_secs_f32();
            Some(60.0 * (self.taps.len() - 1) as f32 / elapsed)
        } else {
            None
        };
        self.bpm
    }

    fn reset(&mut self) {
        self.taps.clear();
        self.bpm = None;
        self.key_was_down = false;
    }
}

const PIANO_KEYS: [(egui::Key, u8); 37] = [
    (egui::Key::Z, 43),
    (egui::Key::S, 44),
    (egui::Key::X, 45),
    (egui::Key::D, 46),
    (egui::Key::C, 47),
    (egui::Key::V, 48),
    (egui::Key::G, 49),
    (egui::Key::B, 50),
    (egui::Key::H, 51),
    (egui::Key::N, 52),
    (egui::Key::M, 53),
    (egui::Key::L, 54),
    (egui::Key::Comma, 55),
    (egui::Key::Semicolon, 56),
    (egui::Key::Period, 57),
    (egui::Key::Quote, 58),
    (egui::Key::Slash, 59),
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

#[derive(Clone, Copy)]
enum TrimHandle {
    Start,
    End,
}

pub struct DawApp {
    project: Project,
    selected_track: Option<u64>,
    view: View,
    playing: bool,
    transport_paused: bool,
    transport_pattern: Option<u64>,
    piano_roll: piano_roll::PianoRoll,
    selected_clip: Option<(u64, u64)>,
    clip_drag: Option<ClipDrag>,
    clip_clipboard: Option<Clip>,
    audio: Option<AudioEngine>,
    audio_error: Option<String>,
    auditioned_notes: HashSet<u8>,
    project_path: Option<PathBuf>,
    project_status: Option<String>,
    synth_mouse_pitch: Option<u8>,
    tap_tempo: TapTempo,
    space_was_down: bool,
    sampler_waveform_path: Option<PathBuf>,
    sampler_waveform: Vec<[f32; 2]>,
    sampler_trim_drag: Option<TrimHandle>,
    sampler_browser_directory: PathBuf,
    selected_iowa_instrument: Option<PathBuf>,
    selected_sample_region: Option<usize>,
    automation_articulation_brush: String,
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
            transport_paused: false,
            transport_pattern: None,
            piano_roll: piano_roll::PianoRoll::default(),
            selected_clip: None,
            clip_drag: None,
            clip_clipboard: None,
            audio,
            audio_error,
            auditioned_notes: HashSet::new(),
            project_path: None,
            project_status: None,
            synth_mouse_pitch: None,
            tap_tempo: TapTempo::default(),
            space_was_down: false,
            sampler_waveform_path: None,
            sampler_waveform: Vec::new(),
            sampler_trim_drag: None,
            sampler_browser_directory: std::env::current_dir()
                .expect("the DAW must start from an accessible working directory"),
            selected_iowa_instrument: None,
            selected_sample_region: None,
            automation_articulation_brush: String::new(),
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
            if !matches!(self.view, View::PianoRoll | View::Instrument)
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
        let instrument = self.selected_track.and_then(|selected| {
            self.project
                .tracks
                .iter()
                .find(|track| track.id == selected)
                .and_then(|track| match &track.kind {
                    TrackKind::Instrument { synth } => Some((Some(*synth), None)),
                    TrackKind::Sampler { sampler } => Some((None, Some(sampler.clone()))),
                    TrackKind::Sample => None,
                })
        });

        if let Some(audio) = &self.audio {
            if let Some((synth, sampler)) = instrument {
                for pitch in desired.difference(&self.auditioned_notes) {
                    let result = if let Some(synth) = synth {
                        audio.audition_start(*pitch, synth)
                    } else {
                        audio.audition_sample_start(
                            *pitch,
                            sampler.clone().expect("audition instrument is a sampler"),
                        )
                    };
                    if let Err(error) = result {
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
        if self.view != View::Instrument
            && let Some(pitch) = self.synth_mouse_pitch.take()
            && let Some(audio) = &self.audio
            && let Err(error) = audio.audition_stop(pitch)
        {
            self.audio_error = Some(error);
        }
    }

    fn save_project(&mut self, choose_path: bool) {
        let path = if choose_path || self.project_path.is_none() {
            rfd::FileDialog::new()
                .add_filter("Don't Track Me project", &["dtm"])
                .set_file_name("project.dtm")
                .save_file()
        } else {
            self.project_path.clone()
        };
        let Some(path) = path else {
            return;
        };
        match project_io::save(&self.project, &path) {
            Ok(()) => {
                self.project_path = Some(path);
                self.project_status = Some("Project saved".to_owned());
            }
            Err(error) => self.project_status = Some(error),
        }
    }

    fn load_project(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Don't Track Me project", &["dtm"])
            .pick_file()
        else {
            return;
        };
        match project_io::load(&path) {
            Ok(project) => {
                if (self.playing || self.transport_paused)
                    && let Some(audio) = &self.audio
                    && let Err(error) = audio.stop()
                {
                    self.audio_error = Some(error);
                }
                self.project = project;
                self.selected_track = self.project.tracks.first().map(|track| track.id);
                self.selected_clip = None;
                self.clip_drag = None;
                self.clip_clipboard = None;
                self.piano_roll = piano_roll::PianoRoll::default();
                self.playing = false;
                self.transport_paused = false;
                self.transport_pattern = None;
                self.view = View::Arrangement;
                self.project_path = Some(path);
                self.project_status = Some("Project loaded".to_owned());
            }
            Err(error) => self.project_status = Some(error),
        }
    }

    fn export_wav(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("WAV audio", &["wav"])
            .set_file_name("arrangement.wav")
            .save_file()
        else {
            return;
        };
        self.project_status = Some(match audio::export_wav(&self.project, &path) {
            Ok(()) => "WAV exported".to_owned(),
            Err(error) => error,
        });
    }

    fn current_pattern_id(&self) -> Option<u64> {
        self.selected_clip
            .and_then(|(lane_id, clip_id)| {
                self.project
                    .tracks
                    .iter()
                    .find(|track| track.id == lane_id)
                    .and_then(|track| track.clips.iter().find(|clip| clip.id == clip_id))
                    .map(|clip| clip.source_id)
            })
            .or_else(|| {
                self.selected_track.and_then(|track_id| {
                    self.project
                        .tracks
                        .iter()
                        .find(|track| track.id == track_id)
                        .map(|track| track.source_id)
                })
            })
    }

    fn toggle_transport(&mut self) {
        let Some(audio) = &self.audio else {
            return;
        };
        let desired_pattern = if self.view == View::PianoRoll {
            self.current_pattern_id()
        } else {
            None
        };
        let result = if self.playing {
            audio.pause()
        } else if self.transport_paused && desired_pattern == self.transport_pattern {
            audio.resume()
        } else if self.view == View::PianoRoll {
            desired_pattern
                .ok_or_else(|| "Select a pattern to play".to_owned())
                .and_then(|pattern_id| audio.play_pattern(&self.project, pattern_id))
        } else {
            audio.play(&self.project)
        };
        match result {
            Ok(()) => {
                if self.playing {
                    self.playing = false;
                    self.transport_paused = true;
                } else {
                    self.playing = true;
                    self.transport_paused = false;
                    self.transport_pattern = desired_pattern;
                }
                self.audio_error = None;
            }
            Err(error) => self.audio_error = Some(error),
        }
    }

    fn top_bar(&mut self, root: &mut egui::Ui) {
        egui::Panel::top("transport").show(root, |ui| {
            ui.horizontal(|ui| {
                ui.heading("DON'T TRACK ME");
                ui.separator();
                if ui.button("Save").clicked() {
                    self.save_project(false);
                }
                if ui.button("Save as").clicked() {
                    self.save_project(true);
                }
                if ui.button("Load").clicked() {
                    self.load_project();
                }
                if ui.button("Export WAV").clicked() {
                    self.export_wav();
                }
                ui.separator();
                let transport_label = if self.playing {
                    "Pause"
                } else if self.transport_paused {
                    "Resume"
                } else {
                    "Play"
                };
                if ui.button(transport_label).clicked() {
                    self.toggle_transport();
                }
                if ui
                    .add_enabled(
                        self.playing || self.transport_paused,
                        egui::Button::new("Stop"),
                    )
                    .clicked()
                    && let Some(audio) = &self.audio
                {
                    match audio.stop() {
                        Ok(()) => {
                            self.playing = false;
                            self.transport_paused = false;
                            self.transport_pattern = None;
                        }
                        Err(error) => self.audio_error = Some(error),
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
                if ui.button("Tap").clicked() {
                    self.tap_tempo.open = true;
                    self.tap_tempo.reset();
                }
                ui.separator();
                ui.selectable_value(&mut self.view, View::Arrangement, "Arrangement");
                let piano_enabled = self.selected_track_mut().is_some_and(|track| {
                    matches!(
                        track.kind,
                        TrackKind::Instrument { .. } | TrackKind::Sampler { .. }
                    )
                });
                ui.add_enabled_ui(piano_enabled, |ui| {
                    ui.selectable_value(&mut self.view, View::PianoRoll, "Piano roll");
                    ui.selectable_value(&mut self.view, View::Instrument, "Instrument");
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(error) = &self.audio_error {
                        ui.colored_label(
                            Color32::from_rgb(245, 115, 105),
                            if self.audio.is_some() {
                                "Audio/sample error"
                            } else {
                                "Audio unavailable"
                            },
                        )
                        .on_hover_text(error);
                    } else if let Some(status) = &self.project_status {
                        ui.label(status);
                    } else {
                        ui.label("Drop audio files anywhere to create sample tracks");
                    }
                });
            });
        });
    }

    fn tap_tempo_window(&mut self, context: &egui::Context) {
        if !self.tap_tempo.open {
            return;
        }
        let key_down = context
            .input(|input| input.key_down(egui::Key::Space) || input.key_down(egui::Key::Enter));
        let keyboard_tap = key_down && !self.tap_tempo.key_was_down;
        self.tap_tempo.key_was_down = key_down;
        let mut open = self.tap_tempo.open;
        let mut button_tap = false;
        let mut reset = false;
        egui::Window::new("Tap tempo")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(context, |ui| {
                ui.set_min_width(300.0);
                ui.label("Tap once per beat using Space, Enter, or the button.");
                ui.add_space(8.0);
                if ui
                    .add_sized([300.0, 90.0], egui::Button::new("TAP"))
                    .clicked()
                {
                    button_tap = true;
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(format!("Taps: {}", self.tap_tempo.taps.len()));
                    ui.separator();
                    if let Some(bpm) = self.tap_tempo.bpm {
                        ui.heading(format!("{bpm:.1} BPM"));
                    } else {
                        ui.weak("Tap again to calculate BPM");
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Reset").clicked() {
                            reset = true;
                        }
                    });
                });
            });
        self.tap_tempo.open = open;
        if reset {
            self.tap_tempo.reset();
        }
        if (keyboard_tap || button_tap)
            && let Some(bpm) = self.tap_tempo.record(Instant::now())
        {
            self.project.bpm = bpm.clamp(20.0, 300.0);
        }
    }

    fn track_list(&mut self, root: &mut egui::Ui) {
        let iowa_library = downloaded_iowa_instruments();
        egui::Panel::left("tracks")
            .default_size(245.0)
            .min_size(220.0)
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Tracks");
                    ui.menu_button("+ Instrument", |ui| {
                        if ui.button("Simple waveform").clicked() {
                            self.selected_track = Some(self.project.add_instrument());
                            ui.close();
                        }
                        if ui.button("Sampler").clicked() {
                            self.selected_track = Some(self.project.add_sampler());
                            ui.close();
                        }
                        if !iowa_library.is_empty() {
                            ui.menu_button("Iowa instrument", |ui| {
                                for folder in &iowa_library {
                                    let name = folder
                                        .file_name()
                                        .and_then(|name| name.to_str())
                                        .unwrap_or("Iowa instrument");
                                    if ui.button(name).clicked() {
                                        match sampler_from_iowa_folder(folder) {
                                            Ok(sampler) => {
                                                self.selected_track =
                                                    Some(self.project.add_configured_sampler(
                                                        name.to_owned(),
                                                        sampler,
                                                    ));
                                                ui.close();
                                            }
                                            Err(error) => self.audio_error = Some(error),
                                        }
                                    }
                                }
                            });
                        }
                    });
                });
                ui.separator();

                for track in &mut self.project.tracks {
                    let selected = self.selected_track == Some(track.id);
                    let icon = match track.kind {
                        TrackKind::Instrument { .. } => "⌁",
                        TrackKind::Sampler { .. } => "◫",
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
                                        if matches!(
                                            track.kind,
                                            TrackKind::Instrument { .. }
                                                | TrackKind::Sampler { .. }
                                        ) && ui
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
                ui.horizontal(|ui| {
                    ui.heading("Clip library");
                    if ui.button("+ Pattern").clicked()
                        && let Some(channel_id) = self.selected_track
                        && self.project.tracks.iter().any(|track| {
                            track.id == channel_id
                                && matches!(
                                    track.kind,
                                    TrackKind::Instrument { .. } | TrackKind::Sampler { .. }
                                )
                        })
                    {
                        let source_id = self.project.add_pattern(channel_id);
                        if let Some(track) = self
                            .project
                            .tracks
                            .iter_mut()
                            .find(|track| track.id == channel_id)
                        {
                            let start = track
                                .clips
                                .iter()
                                .map(|clip| clip.start_step + clip.length_steps)
                                .max()
                                .unwrap_or(0)
                                .min(ARRANGEMENT_STEPS - 1);
                            let id = track.add_clip(
                                source_id,
                                start,
                                PATTERN_STEPS.min(ARRANGEMENT_STEPS - start),
                            );
                            self.selected_clip = Some((channel_id, id));
                        }
                    }
                });
                ui.weak("Reusable originals");
                let mut add_source = None;
                for source in &self.project.clip_library {
                    ui.horizontal(|ui| {
                        ui.label(match &source.kind {
                            ClipSourceKind::Pattern { .. } => "▦",
                            ClipSourceKind::Sample { .. } => "▰",
                        });
                        let details = match &source.kind {
                            ClipSourceKind::Pattern { .. } => {
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
                            add_source = Some((source.channel_id, source.id, source.length_steps));
                        }
                    });
                }
                if let Some((channel_id, source_id, length)) = add_source {
                    let track_id = self.selected_track.unwrap_or(channel_id);
                    if let Some(track) = self
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
                        let start = start.min(ARRANGEMENT_STEPS - 1);
                        let id =
                            track.add_clip(source_id, start, length.min(ARRANGEMENT_STEPS - start));
                        self.selected_clip = Some((track_id, id));
                        self.view = View::Arrangement;
                    }
                }
            });
    }

    fn arrangement(&mut self, ui: &mut egui::Ui) {
        let mut prerender_track = None;
        let mut restore_live_track = None;
        const STEPS: u16 = ARRANGEMENT_STEPS;
        const STEP_WIDTH: f32 = 12.0;
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
                let id =
                    track.add_clip(clip.source_id, start, clip.length_steps.min(STEPS - start));
                self.selected_clip = Some((track_id, id));
            }
        }
        if paste
            && let Some(copied) = self.clip_clipboard.clone()
            && let Some(track_id) = self.selected_track.or_else(|| {
                self.project
                    .source(copied.source_id)
                    .map(|source| source.channel_id)
            })
            && let Some(track) = self
                .project
                .tracks
                .iter_mut()
                .find(|track| track.id == track_id)
        {
            let start = (copied.start_step + copied.length_steps).min(STEPS - 1);
            let id = track.add_clip(
                copied.source_id,
                start,
                copied.length_steps.min(STEPS - start),
            );
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
            ui.menu_button("+ Instrument track", |ui| {
                if ui.button("Simple waveform").clicked() {
                    self.selected_track = Some(self.project.add_instrument());
                    ui.close();
                }
                if ui.button("Sampler").clicked() {
                    self.selected_track = Some(self.project.add_sampler());
                    ui.close();
                }
            });
            if ui.button("+ Sample track").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Audio", &["wav", "mp3", "flac"])
                    .pick_file()
            {
                self.selected_track = Some(self.project.add_sample(path));
            }
            ui.separator();
            ui.label("8 bars · 4/4");
            ui.separator();
            ui.weak("Drag to move · right edge to trim · double-click pattern to edit · Ctrl/Cmd+C, X, V, D");
        });
        ui.add_space(8.0);

        let clip_library = self.project.clip_library.clone();
        let mut lane_rects = Vec::new();
        let mut clip_drop = None;
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            ui.set_min_width(180.0 + STEP_WIDTH * f32::from(STEPS));
            ui.horizontal(|ui| {
                ui.add_sized([170.0, 24.0], egui::Label::new(""));
                let (header, _) = ui.allocate_exact_size(
                    egui::vec2(STEP_WIDTH * f32::from(STEPS), 24.0),
                    egui::Sense::hover(),
                );
                for bar in 0..8 {
                    let x = header.left() + bar as f32 * STEP_WIDTH * f32::from(STEPS_PER_BAR);
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
                        if matches!(
                            track.kind,
                            TrackKind::Instrument { .. } | TrackKind::Sampler { .. }
                        ) && ui.small_button("+ Pattern clip").clicked()
                        {
                            let start = track
                                .clips
                                .iter()
                                .map(|clip| clip.start_step + clip.length_steps)
                                .max()
                                .unwrap_or(0);
                            if start < STEPS {
                                let id = track.add_clip(
                                    track.source_id,
                                    start,
                                    PATTERN_STEPS.min(STEPS - start),
                                );
                                self.selected_clip = Some((track.id, id));
                            }
                        }
                        if matches!(track.kind, TrackKind::Sampler { .. })
                            && ui.small_button("Pre-render").clicked()
                        {
                            prerender_track = Some(track.id);
                        }
                        if let Some(source_id) = track.rendered_from
                            && ui.small_button("Restore live").clicked()
                        {
                            restore_live_track = Some((track.id, source_id));
                        }
                    });
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(STEP_WIDTH * f32::from(STEPS), TRACK_HEIGHT),
                        egui::Sense::click_and_drag(),
                    );
                    lane_rects.push((track.id, rect));
                    ui.painter()
                        .rect_filled(rect, 3.0, Color32::from_rgb(31, 35, 42));
                    for step in 0..=STEPS {
                        let x = rect.left() + f32::from(step) * STEP_WIDTH;
                        ui.painter().line_segment(
                            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                            egui::Stroke::new(
                                if step % STEPS_PER_BAR == 0 {
                                    1.5
                                } else if step % STEPS_PER_BEAT == 0 {
                                    1.0
                                } else {
                                    0.5
                                },
                                Color32::from_gray(if step % STEPS_PER_BAR == 0 { 70 } else { 48 }),
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
                        self.selected_track = Some(track.id);
                    }
                    if response.double_clicked()
                        && let Some(pointer) = response.interact_pointer_pos()
                        && track.clips.iter().any(|clip| {
                            clip_rect(clip.start_step, clip.length_steps).contains(pointer)
                        })
                    {
                        let source_id = track
                            .clips
                            .iter()
                            .find(|clip| {
                                clip_rect(clip.start_step, clip.length_steps).contains(pointer)
                            })
                            .map(|clip| clip.source_id)
                            .expect("double-clicked clip was just found");
                        self.selected_track = clip_library
                            .iter()
                            .find(|source| source.id == source_id)
                            .map(|source| source.channel_id);
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
                        if let Some(pointer) = response.interact_pointer_pos() {
                            clip_drop = match &self.clip_drag {
                                Some(ClipDrag::Move {
                                    track_id, clip_id, ..
                                }) => Some((*track_id, *clip_id, pointer)),
                                Some(ClipDrag::Resize { .. }) | None => None,
                            };
                        }
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
                            TrackKind::Sampler { .. } => Color32::from_rgb(137, 91, 166),
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
                        if let ClipSourceKind::Pattern { pattern } = &source.kind {
                            let baseline = area.bottom() - 6.0;
                            for note in &pattern.notes {
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
        if let Some(track_id) = prerender_track {
            let render_directory = PathBuf::from("data/renders");
            let render_path = render_directory.join(format!("sampler-track-{track_id}.wav"));
            match std::fs::create_dir_all(&render_directory)
                .map_err(|error| format!("Could not create render directory: {error}"))
                .and_then(|_| audio::export_track_wav(&self.project, track_id, &render_path))
            {
                Ok(()) => {
                    let source_name = self
                        .project
                        .tracks
                        .iter()
                        .find(|track| track.id == track_id)
                        .map(|track| track.name.clone())
                        .expect("pre-render source track was just selected");
                    self.project
                        .tracks
                        .iter_mut()
                        .find(|track| track.id == track_id)
                        .expect("pre-render source track was just selected")
                        .muted = true;
                    let rendered_id = self
                        .project
                        .tracks
                        .iter()
                        .find(|track| track.rendered_from == Some(track_id))
                        .map(|track| track.id)
                        .unwrap_or_else(|| {
                            let rendered_id = self
                                .project
                                .add_sample_with_length(render_path, ARRANGEMENT_STEPS);
                            self.project
                                .tracks
                                .iter_mut()
                                .find(|track| track.id == rendered_id)
                                .expect("rendered sample track was just inserted")
                                .rendered_from = Some(track_id);
                            rendered_id
                        });
                    let rendered = self
                        .project
                        .tracks
                        .iter_mut()
                        .find(|track| track.id == rendered_id)
                        .expect("rendered sample track was just inserted");
                    rendered.name = format!("{source_name} (rendered)");
                    rendered.muted = false;
                    self.selected_track = Some(rendered_id);
                    self.project_status = Some(format!("Pre-rendered {source_name}"));
                }
                Err(error) => self.audio_error = Some(error),
            }
        }
        if let Some((rendered_id, source_id)) = restore_live_track {
            self.project
                .tracks
                .iter_mut()
                .find(|track| track.id == rendered_id)
                .expect("rendered track requesting restore still exists")
                .muted = true;
            if let Some(source) = self
                .project
                .tracks
                .iter_mut()
                .find(|track| track.id == source_id)
            {
                source.muted = false;
                self.selected_track = Some(source_id);
                self.project_status = Some(format!("Restored live {}", source.name));
            } else {
                self.audio_error =
                    Some("The rendered track's source sampler is missing".to_owned());
            }
        }
        if let Some((from_track_id, clip_id, pointer)) = clip_drop
            && let Some(to_track_id) = lane_rects
                .iter()
                .find(|(_, rect)| rect.contains(pointer))
                .map(|(track_id, _)| *track_id)
            && to_track_id != from_track_id
            && let Some(from_index) = self
                .project
                .tracks
                .iter()
                .position(|track| track.id == from_track_id)
            && let Some(clip_index) = self.project.tracks[from_index]
                .clips
                .iter()
                .position(|clip| clip.id == clip_id)
        {
            let clip = self.project.tracks[from_index].clips.remove(clip_index);
            let to_track = self
                .project
                .tracks
                .iter_mut()
                .find(|track| track.id == to_track_id)
                .expect("drop target lane was just found");
            let new_id = to_track.add_clip(clip.source_id, clip.start_step, clip.length_steps);
            self.selected_clip = Some((to_track_id, new_id));
        }
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
        let pattern_id = self
            .selected_clip
            .and_then(|(lane_id, clip_id)| {
                self.project
                    .tracks
                    .iter()
                    .find(|track| track.id == lane_id)
                    .and_then(|track| track.clips.iter().find(|clip| clip.id == clip_id))
                    .map(|clip| clip.source_id)
            })
            .unwrap_or(self.project.tracks[index].source_id);
        let Some(source_index) = self
            .project
            .clip_library
            .iter()
            .position(|source| source.id == pattern_id)
        else {
            ui.label("This pattern is no longer in the clip library.");
            return;
        };
        let (tracks, sources) = (&mut self.project.tracks, &mut self.project.clip_library);
        let track = &mut tracks[index];

        ui.horizontal(|ui| {
            ui.heading(&track.name);
            ui.separator();
            ui.label("Instrument");
            match &mut track.kind {
                TrackKind::Instrument { synth } => {
                    egui::ComboBox::from_id_salt("waveform")
                        .selected_text(synth.layers[0].waveform.name())
                        .show_ui(ui, |ui| {
                            for choice in Waveform::ALL {
                                ui.selectable_value(
                                    &mut synth.layers[0].waveform,
                                    choice,
                                    choice.name(),
                                );
                            }
                        });
                }
                TrackKind::Sampler { .. } => {
                    ui.label("Sampler");
                }
                TrackKind::Sample => {}
            }
            if !matches!(track.kind, TrackKind::Sample) && ui.button("Settings").clicked() {
                self.view = View::Instrument;
            }
        });
        ui.separator();
        let pattern_name = sources[source_index].name.clone();
        if let ClipSourceKind::Pattern { pattern } = &mut sources[source_index].kind
            && !matches!(track.kind, TrackKind::Sample)
        {
            ui.label(format!("Editing {pattern_name}"));
            pattern_automation_editor(
                ui,
                pattern,
                &track.kind,
                &mut self.automation_articulation_brush,
            );
            let output = self
                .piano_roll
                .show(ui, pattern_id, pattern, &self.auditioned_notes);
            if let Some(audio) = &self.audio {
                if let Some(pitch) = output.note_off
                    && let Err(error) = audio.audition_stop(pitch)
                {
                    self.audio_error = Some(error);
                }
                if let Some(pitch) = output.note_on {
                    let result = match &track.kind {
                        TrackKind::Instrument { synth } => audio.audition_start(pitch, *synth),
                        TrackKind::Sampler { sampler } => {
                            audio.audition_sample_start(pitch, sampler.clone())
                        }
                        TrackKind::Sample => unreachable!("sample tracks have no piano roll"),
                    };
                    if let Err(error) = result {
                        self.audio_error = Some(error);
                    }
                }
            }
        } else {
            ui.label("Select an instrument pattern to open its piano roll.");
        }
    }

    fn instrument_settings(&mut self, ui: &mut egui::Ui) {
        let Some(selected) = self.selected_track else {
            ui.centered_and_justified(|ui| ui.label("Select an instrument track."));
            return;
        };
        if self
            .project
            .tracks
            .iter()
            .find(|track| track.id == selected)
            .is_some_and(|track| matches!(track.kind, TrackKind::Sampler { .. }))
        {
            self.sampler_settings(ui, selected);
            return;
        }
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

        let mut keyboard_output = SynthKeyboardOutput::default();
        let mut mouse_pitch = self.synth_mouse_pitch;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.set_max_width(920.0);
            egui::Frame::group(ui.style())
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.heading("Presets");
                    egui::ComboBox::from_id_salt("waveform-synth-preset")
                        .selected_text("Choose preset…")
                        .width(280.0)
                        .show_ui(ui, |ui| {
                            let mut categories = Vec::new();
                            for preset in SimpleWaveformSynth::PRESETS {
                                if !categories.contains(&preset.category) {
                                    categories.push(preset.category);
                                }
                            }
                            for (index, category) in categories.into_iter().enumerate() {
                                if index > 0 {
                                    ui.separator();
                                }
                                ui.label(egui::RichText::new(category).strong());
                                for preset in SimpleWaveformSynth::PRESETS
                                    .iter()
                                    .filter(|preset| preset.category == category)
                                {
                                    if ui.selectable_label(false, preset.name).clicked() {
                                        *synth = preset.synth;
                                        ui.close();
                                    }
                                }
                            }
                        });
                });
            ui.add_space(10.0);
            egui::Frame::group(ui.style())
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading("Oscillator layers");
                        ui.add(
                            egui::Slider::new(&mut synth.layer_count, 1..=4)
                                .text("Voices")
                                .integer(),
                        );
                    });
                    for (index, layer) in synth
                        .layers
                        .iter_mut()
                        .take(usize::from(synth.layer_count))
                        .enumerate()
                    {
                        ui.horizontal(|ui| {
                            ui.label(format!("Voice {}", index + 1));
                            egui::ComboBox::from_id_salt(("layer-waveform", index))
                                .selected_text(layer.waveform.name())
                                .show_ui(ui, |ui| {
                                    for waveform in Waveform::ALL {
                                        ui.selectable_value(
                                            &mut layer.waveform,
                                            waveform,
                                            waveform.name(),
                                        );
                                    }
                                });
                            ui.add(
                                egui::Slider::new(&mut layer.detune_cents, -100.0..=100.0)
                                    .text("Detune")
                                    .suffix(" cents"),
                            );
                            ui.add(egui::Slider::new(&mut layer.level, 0.0..=1.0).text("Volume"));
                        });
                    }
                    ui.add_space(8.0);
                    waveform_preview(ui, synth);
                });
            ui.add_space(10.0);
            egui::Frame::group(ui.style())
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.heading("Amplifier and pitch");
                    ui.columns(3, |columns| {
                        columns[0].add(
                            egui::Slider::new(&mut synth.master_level, 0.0..=1.0)
                                .text("Master volume"),
                        );
                        columns[1].add(
                            egui::Slider::new(&mut synth.pan, -1.0..=1.0)
                                .text("Pan")
                                .custom_formatter(|value, _| {
                                    if value.abs() < 0.01 {
                                        "Centre".to_owned()
                                    } else if value < 0.0 {
                                        format!("L {:.0}%", -value * 100.0)
                                    } else {
                                        format!("R {:.0}%", value * 100.0)
                                    }
                                }),
                        );
                        columns[2].add(
                            egui::Slider::new(&mut synth.pitch_shift, -24..=24)
                                .text("Pitch")
                                .suffix(" semitones"),
                        );
                    });
                });
            ui.add_space(10.0);
            egui::Frame::group(ui.style())
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.heading("ADSR envelope");
                    ui.columns(4, |columns| {
                        columns[0].add(
                            egui::Slider::new(&mut synth.attack_ms, 0.0..=2_000.0)
                                .text("Attack")
                                .suffix(" ms"),
                        );
                        columns[1].add(
                            egui::Slider::new(&mut synth.decay_ms, 0.0..=3_000.0)
                                .text("Decay")
                                .suffix(" ms"),
                        );
                        columns[2]
                            .add(egui::Slider::new(&mut synth.sustain, 0.0..=1.0).text("Sustain"));
                        columns[3].add(
                            egui::Slider::new(&mut synth.release_ms, 0.0..=5_000.0)
                                .text("Release")
                                .suffix(" ms"),
                        );
                    });
                });
            ui.add_space(10.0);
            egui::Frame::group(ui.style())
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.heading("Playing mode");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut synth.mono, "Mono");
                        ui.add_enabled(
                            synth.mono,
                            egui::Slider::new(&mut synth.glide_ms, 0.0..=1_000.0)
                                .text("Pitch glide")
                                .suffix(" ms"),
                        );
                    });
                });
            ui.add_space(10.0);
            egui::Frame::group(ui.style())
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.heading("Filter");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("filter-kind")
                            .selected_text(synth.filter.name())
                            .show_ui(ui, |ui| {
                                for filter in FilterKind::ALL {
                                    ui.selectable_value(&mut synth.filter, filter, filter.name());
                                }
                            });
                        ui.add_enabled(
                            synth.filter != FilterKind::Off,
                            egui::Slider::new(&mut synth.filter_cutoff_hz, 20.0..=20_000.0)
                                .logarithmic(true)
                                .text("Cutoff")
                                .suffix(" Hz"),
                        );
                        ui.add_enabled(
                            synth.filter != FilterKind::Off,
                            egui::Slider::new(&mut synth.filter_resonance, 0.0..=0.95)
                                .text("Resonance"),
                        );
                    });
                });
            ui.add_space(10.0);
            egui::Frame::group(ui.style())
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.heading("Effects stack");
                    ui.weak("Signal flows from top to bottom after all voices are mixed.");
                    let mut reorder = None;
                    for (index, effect) in synth.effects.iter_mut().enumerate() {
                        egui::Frame::group(ui.style())
                            .inner_margin(8.0)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut effect.enabled, "");
                                    ui.strong(format!("{}. {}", index + 1, effect.kind.name()));
                                if ui
                                    .add_enabled(index > 0, egui::Button::new("Move up"))
                                    .clicked()
                                {
                                    reorder = Some((index, index - 1));
                                }
                                if ui
                                    .add_enabled(index + 1 < 5, egui::Button::new("Move down"))
                                    .clicked()
                                {
                                        reorder = Some((index, index + 1));
                                    }
                                    if !effect.enabled {
                                        ui.weak("Bypassed");
                                    }
                                });
                                ui.add_enabled_ui(effect.enabled, |ui| {
                                    ui.horizontal(|ui| match &mut effect.kind {
                                        EffectKind::Distortion { drive, mix } => {
                                            ui.add(
                                                egui::Slider::new(drive, 1.0..=20.0).text("Drive"),
                                            );
                                            ui.add(egui::Slider::new(mix, 0.0..=1.0).text("Mix"));
                                        }
                                        EffectKind::Delay {
                                            time_ms,
                                            feedback,
                                            mix,
                                        } => {
                                            ui.add(
                                                egui::Slider::new(time_ms, 10.0..=1_000.0)
                                                    .text("Time")
                                                    .suffix(" ms"),
                                            );
                                            ui.add(
                                                egui::Slider::new(feedback, 0.0..=0.9)
                                                    .text("Feedback"),
                                            );
                                            ui.add(egui::Slider::new(mix, 0.0..=1.0).text("Mix"));
                                        }
                                        EffectKind::Chorus {
                                            rate_hz,
                                            depth_ms,
                                            mix,
                                        } => {
                                            ui.add(
                                                egui::Slider::new(rate_hz, 0.05..=5.0)
                                                    .text("Rate")
                                                    .suffix(" Hz"),
                                            );
                                            ui.add(
                                                egui::Slider::new(depth_ms, 1.0..=30.0)
                                                    .text("Depth")
                                                    .suffix(" ms"),
                                            );
                                            ui.add(egui::Slider::new(mix, 0.0..=1.0).text("Mix"));
                                        }
                                        EffectKind::Tremolo { rate_hz, depth } => {
                                            ui.add(
                                                egui::Slider::new(rate_hz, 0.1..=20.0)
                                                    .text("Rate")
                                                    .suffix(" Hz"),
                                            );
                                            ui.add(
                                                egui::Slider::new(depth, 0.0..=1.0).text("Depth"),
                                            );
                                        }
                                        EffectKind::Reverb {
                                            room_size,
                                            damping,
                                            mix,
                                        } => {
                                            ui.add(
                                                egui::Slider::new(room_size, 0.0..=1.0)
                                                    .text("Room"),
                                            );
                                            ui.add(
                                                egui::Slider::new(damping, 0.0..=0.95)
                                                    .text("Damping"),
                                            );
                                            ui.add(egui::Slider::new(mix, 0.0..=1.0).text("Mix"));
                                        }
                                    });
                                });
                            });
                        ui.add_space(4.0);
                    }
                    if let Some((from, to)) = reorder {
                        synth.effects.swap(from, to);
                    }
                });
            ui.add_space(10.0);
            egui::Frame::group(ui.style())
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.heading("Test keyboard");
                    ui.weak("Play with the mouse or the same computer-keyboard mapping as the piano roll.");
                    keyboard_output = synth_test_keyboard(
                        ui,
                        &self.auditioned_notes,
                        &mut mouse_pitch,
                    );
                });
        });
        self.synth_mouse_pitch = mouse_pitch;
        let audition_synth = *synth;
        if let Some(audio) = &self.audio {
            if let Some(pitch) = keyboard_output.note_off
                && let Err(error) = audio.audition_stop(pitch)
            {
                self.audio_error = Some(error);
            }
            if let Some(pitch) = keyboard_output.note_on
                && let Err(error) = audio.audition_start(pitch, audition_synth)
            {
                self.audio_error = Some(error);
            }
        }
    }

    fn sampler_settings(&mut self, ui: &mut egui::Ui, selected: u64) {
        let iowa_library = downloaded_iowa_instruments();
        if self.selected_iowa_instrument.is_none() {
            self.selected_iowa_instrument = iowa_library.first().cloned();
        }
        let Some(track) = self
            .project
            .tracks
            .iter_mut()
            .find(|track| track.id == selected)
        else {
            return;
        };
        let TrackKind::Sampler { sampler } = &mut track.kind else {
            return;
        };
        if self.sampler_waveform_path.as_ref() != sampler.path.as_ref() {
            self.sampler_waveform_path = sampler.path.clone();
            self.sampler_waveform.clear();
            if let Some(path) = &sampler.path {
                match audio::load_waveform_preview(path, 900) {
                    Ok(preview) => self.sampler_waveform = preview,
                    Err(error) => self.audio_error = Some(error),
                }
            }
        }
        ui.horizontal(|ui| {
            ui.heading("Sampler");
            ui.separator();
            ui.label(&track.name);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Open piano roll").clicked() {
                    self.view = View::PianoRoll;
                }
            });
        });
        ui.add_space(12.0);
        let mut keyboard_output = SynthKeyboardOutput::default();
        let mut mouse_pitch = self.synth_mouse_pitch;
        let mut trim_drag = self.sampler_trim_drag;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.set_max_width(920.0);
            egui::Frame::group(ui.style())
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.heading("Sample");
                    if !iowa_library.is_empty() {
                        ui.horizontal(|ui| {
                            ui.label("Downloaded Iowa library");
                            egui::ComboBox::from_id_salt("downloaded-iowa-instrument")
                                .selected_text(
                                    self.selected_iowa_instrument
                                        .as_ref()
                                        .and_then(|path| path.file_name())
                                        .and_then(|name| name.to_str())
                                        .unwrap_or("Select instrument"),
                                )
                                .show_ui(ui, |ui| {
                                    for path in &iowa_library {
                                        let name = path
                                            .file_name()
                                            .and_then(|name| name.to_str())
                                            .unwrap_or("Unnamed instrument");
                                        ui.selectable_value(
                                            &mut self.selected_iowa_instrument,
                                            Some(path.clone()),
                                            name,
                                        );
                                    }
                                });
                            if ui.button("Load instrument").clicked()
                                && let Some(folder) = &self.selected_iowa_instrument
                            {
                                match discover_iowa_regions(folder) {
                                    Ok(regions) if !regions.is_empty() => {
                                        sampler.path = Some(regions[0].path.clone());
                                        sampler.root_pitch = regions[0].root_pitch;
                                        sampler.articulation = regions[0].articulation.clone();
                                        sampler.regions = regions;
                                        self.selected_sample_region = Some(0);
                                        sampler.trim_start = 0.0;
                                        sampler.trim_end = 1.0;
                                    }
                                    Ok(_) => {
                                        self.audio_error = Some(format!(
                                            "No pitch-named WAV files were found in {}",
                                            folder.display()
                                        ));
                                    }
                                    Err(error) => self.audio_error = Some(error),
                                }
                            }
                        });
                        ui.add_space(8.0);
                    } else {
                        ui.weak("No downloaded Iowa instruments found in data/samples/iowa.");
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Load WAV").clicked()
                            && let Some(path) = rfd::FileDialog::new()
                                .set_directory(&self.sampler_browser_directory)
                                .add_filter("WAV audio", &["wav"])
                                .pick_file()
                        {
                            self.sampler_browser_directory = path
                                .parent()
                                .expect("a selected sample file has a parent directory")
                                .to_owned();
                            sampler.path = Some(path);
                            sampler.trim_start = 0.0;
                            sampler.trim_end = 1.0;
                            sampler.regions.clear();
                            sampler.articulation = "Standard".to_owned();
                            self.selected_sample_region = None;
                        }
                        if ui.button("Import Iowa instrument").clicked()
                            && let Some(folder) = rfd::FileDialog::new()
                                .set_directory(&self.sampler_browser_directory)
                                .pick_folder()
                        {
                            self.sampler_browser_directory = folder.clone();
                            match discover_iowa_regions(&folder) {
                                Ok(regions) if !regions.is_empty() => {
                                    sampler.path = Some(regions[0].path.clone());
                                    sampler.root_pitch = regions[0].root_pitch;
                                    sampler.articulation = regions[0].articulation.clone();
                                    sampler.regions = regions;
                                    self.selected_sample_region = Some(0);
                                    sampler.trim_start = 0.0;
                                    sampler.trim_end = 1.0;
                                }
                                Ok(_) => {
                                    self.audio_error = Some(
                                        "No pitch-named WAV files were found in that folder"
                                            .to_owned(),
                                    );
                                }
                                Err(error) => self.audio_error = Some(error),
                            }
                        }
                        ui.label(
                            sampler
                                .path
                                .as_ref()
                                .and_then(|path| path.file_name())
                                .and_then(|name| name.to_str())
                                .unwrap_or("No sample loaded"),
                        );
                    });
                    if !sampler.regions.is_empty() {
                        ui.label(format!("{} mapped sample regions", sampler.regions.len()));
                        let mut articulations = sampler
                            .regions
                            .iter()
                            .map(|region| region.articulation.clone())
                            .collect::<Vec<_>>();
                        articulations.sort();
                        articulations.dedup();
                        ui.horizontal(|ui| {
                            ui.label("Articulation");
                            let previous = sampler.articulation.clone();
                            egui::ComboBox::from_id_salt("sampler-articulation")
                                .selected_text(&sampler.articulation)
                                .show_ui(ui, |ui| {
                                    for articulation in articulations {
                                        ui.selectable_value(
                                            &mut sampler.articulation,
                                            articulation.clone(),
                                            articulation,
                                        );
                                    }
                                });
                            if sampler.articulation != previous {
                                let region = sampler
                                    .regions
                                    .iter()
                                    .find(|region| region.articulation == sampler.articulation)
                                    .expect("the selected articulation came from a sample region");
                                sampler.path = Some(region.path.clone());
                                sampler.root_pitch = region.root_pitch;
                                self.selected_sample_region = sampler
                                    .regions
                                    .iter()
                                    .position(|region| region.articulation == sampler.articulation);
                            }
                        });
                        sample_region_editor(ui, sampler, &mut self.selected_sample_region);
                    }
                    ui.add_space(8.0);
                    sample_waveform_editor(ui, &self.sampler_waveform, sampler, &mut trim_drag);
                    ui.weak("Click or drag near a marker to set the sample start or end point.");
                    ui.add_space(8.0);
                    ui.add(
                        egui::Slider::new(&mut sampler.trim_start, 0.0..=sampler.trim_end - 0.001)
                            .text("Start"),
                    );
                    ui.add(
                        egui::Slider::new(&mut sampler.trim_end, sampler.trim_start + 0.001..=1.0)
                            .text("End"),
                    );
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut sampler.reverse, "Reverse");
                        ui.checkbox(&mut sampler.looping, "Loop until note ends");
                        ui.add_enabled_ui(sampler.looping, |ui| {
                            egui::ComboBox::from_id_salt("sampler-loop-mode")
                                .selected_text(sampler.loop_mode.name())
                                .show_ui(ui, |ui| {
                                    for mode in [SampleLoopMode::Forward, SampleLoopMode::PingPong]
                                    {
                                        ui.selectable_value(
                                            &mut sampler.loop_mode,
                                            mode,
                                            mode.name(),
                                        );
                                    }
                                });
                        });
                    });
                });
            ui.add_space(10.0);
            egui::Frame::group(ui.style())
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.heading("Pitch and timing");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::Slider::new(&mut sampler.root_pitch, 12..=132)
                                .text("Root key")
                                .integer(),
                        );
                        ui.add(
                            egui::Slider::new(&mut sampler.speed, 0.25..=4.0)
                                .logarithmic(true)
                                .text("Speed / stretch"),
                        );
                    });
                    ui.weak("Classic sampler stretching changes both duration and pitch.");
                });
            ui.add_space(10.0);
            egui::Frame::group(ui.style())
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.heading("Output and envelope");
                    ui.columns(2, |columns| {
                        columns[0]
                            .add(egui::Slider::new(&mut sampler.gain, 0.0..=2.0).text("Gain"));
                        columns[1].add(egui::Slider::new(&mut sampler.pan, -1.0..=1.0).text("Pan"));
                        columns[0].add(
                            egui::Slider::new(&mut sampler.attack_ms, 0.0..=2_000.0)
                                .text("Attack")
                                .suffix(" ms"),
                        );
                        columns[1].add(
                            egui::Slider::new(&mut sampler.decay_ms, 0.0..=2_000.0)
                                .text("Decay")
                                .suffix(" ms"),
                        );
                        columns[0].add(
                            egui::Slider::new(&mut sampler.sustain, 0.0..=1.0).text("Sustain"),
                        );
                        columns[1].add(
                            egui::Slider::new(&mut sampler.release_ms, 0.0..=5_000.0)
                                .text("Release")
                                .suffix(" ms"),
                        );
                    });
                });
            ui.add_space(10.0);
            egui::Frame::group(ui.style())
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.heading("Filter");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("sampler-filter")
                            .selected_text(sampler.filter.name())
                            .show_ui(ui, |ui| {
                                for filter in FilterKind::ALL {
                                    ui.selectable_value(&mut sampler.filter, filter, filter.name());
                                }
                            });
                        ui.add_enabled(
                            sampler.filter != FilterKind::Off,
                            egui::Slider::new(&mut sampler.filter_cutoff_hz, 20.0..=20_000.0)
                                .logarithmic(true)
                                .text("Cutoff")
                                .suffix(" Hz"),
                        );
                        ui.add_enabled(
                            sampler.filter != FilterKind::Off,
                            egui::Slider::new(&mut sampler.filter_resonance, 0.0..=0.95)
                                .text("Resonance"),
                        );
                    });
                });
            ui.add_space(10.0);
            egui::Frame::group(ui.style())
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.heading("Test keyboard");
                    keyboard_output =
                        synth_test_keyboard(ui, &self.auditioned_notes, &mut mouse_pitch);
                });
        });
        self.synth_mouse_pitch = mouse_pitch;
        self.sampler_trim_drag = trim_drag;
        let audition_sampler = sampler.clone();
        if let Some(audio) = &self.audio {
            if let Some(pitch) = keyboard_output.note_off
                && let Err(error) = audio.audition_stop(pitch)
            {
                self.audio_error = Some(error);
            }
            if let Some(pitch) = keyboard_output.note_on
                && let Err(error) = audio.audition_sample_start(pitch, audition_sampler)
            {
                self.audio_error = Some(error);
            }
        }
    }
}

fn pattern_automation_editor(
    ui: &mut egui::Ui,
    pattern: &mut Pattern,
    kind: &TrackKind,
    articulation_brush: &mut String,
) {
    let (articulations, articulation_default, cutoff_parameter, cutoff_default) = match kind {
        TrackKind::Sampler { sampler } => {
            let mut choices = sampler
                .regions
                .iter()
                .map(|region| region.articulation.clone())
                .collect::<Vec<_>>();
            choices.sort();
            choices.dedup();
            if choices.is_empty() {
                choices.push(sampler.articulation.clone());
            }
            (
                Some(choices),
                Some(sampler.articulation.as_str()),
                AutomationParameter::SamplerFilterCutoff,
                sampler.filter_cutoff_hz,
            )
        }
        TrackKind::Instrument { synth } => (
            None,
            None,
            AutomationParameter::SynthFilterCutoff,
            synth.filter_cutoff_hz,
        ),
        TrackKind::Sample => return,
    };

    egui::Frame::group(ui.style())
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.heading("Automation lanes");
            if let (Some(articulations), Some(default)) = (articulations, articulation_default) {
                if articulation_brush.is_empty() || !articulations.contains(articulation_brush) {
                    *articulation_brush = default.to_owned();
                }
                ui.horizontal(|ui| {
                    ui.label("Articulation brush");
                    egui::ComboBox::from_id_salt("automation-articulation-brush")
                        .selected_text(articulation_brush.as_str())
                        .show_ui(ui, |ui| {
                            for articulation in &articulations {
                                ui.selectable_value(
                                    articulation_brush,
                                    articulation.clone(),
                                    articulation,
                                );
                            }
                        });
                    if ui.button("Clear lane").clicked() {
                        pattern.automation.retain(|lane| {
                            lane.parameter != AutomationParameter::SamplerArticulation
                        });
                    }
                });
                ui.weak(
                    "Click the timeline to paint from that step onward. Right-click a marker to remove it.",
                );
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width().min(880.0), 34.0),
                    egui::Sense::click(),
                );
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 2.0, Color32::from_rgb(25, 29, 36));
                for step in (0..=PATTERN_STEPS).step_by(usize::from(STEPS_PER_BEAT)) {
                    let x = rect.left() + f32::from(step) / f32::from(PATTERN_STEPS) * rect.width();
                    painter.line_segment(
                        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                        egui::Stroke::new(1.0, Color32::from_white_alpha(35)),
                    );
                }
                let lane = pattern
                    .automation
                    .iter()
                    .find(|lane| lane.parameter == AutomationParameter::SamplerArticulation);
                for step in 0..PATTERN_STEPS {
                    let value = lane
                        .and_then(|lane| lane.value_at(step))
                        .and_then(|value| match value {
                            AutomationValue::Choice(value) => Some(value.as_str()),
                            AutomationValue::Continuous(_) => None,
                        })
                        .unwrap_or(default);
                    let index = articulations
                        .iter()
                        .position(|choice| choice == value)
                        .unwrap_or(0);
                    let x0 =
                        rect.left() + f32::from(step) / f32::from(PATTERN_STEPS) * rect.width();
                    let x1 =
                        rect.left() + f32::from(step + 1) / f32::from(PATTERN_STEPS) * rect.width();
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(x0, rect.top()),
                            egui::pos2(x1, rect.bottom()),
                        ),
                        0.0,
                        Color32::from_rgb(
                            65 + (index as u8 * 31) % 80,
                            105 + (index as u8 * 47) % 80,
                            135 + (index as u8 * 19) % 70,
                        ),
                    );
                }
                if let Some(lane) = lane {
                    for point in &lane.points {
                        let x = rect.left()
                            + f32::from(point.step) / f32::from(PATTERN_STEPS) * rect.width();
                        painter.line_segment(
                            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                            egui::Stroke::new(2.0, Color32::WHITE),
                        );
                        if let AutomationValue::Choice(value) = &point.value {
                            painter.text(
                                egui::pos2(x + 4.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                value,
                                egui::FontId::proportional(11.0),
                                Color32::WHITE,
                            );
                        }
                    }
                }
                if let Some(pointer) = response.hover_pos() {
                    let step = (((pointer.x - rect.left()) / rect.width())
                        * f32::from(PATTERN_STEPS))
                    .floor()
                    .clamp(0.0, f32::from(PATTERN_STEPS - 1)) as u16;
                    painter.text(
                        pointer + egui::vec2(8.0, -8.0),
                        egui::Align2::LEFT_BOTTOM,
                        format!("Step {step} · paint {articulation_brush}"),
                        egui::FontId::monospace(10.0),
                        Color32::WHITE,
                    );
                }
                if response.clicked_by(egui::PointerButton::Primary)
                    && let Some(pointer) = response.interact_pointer_pos()
                {
                    let step = (((pointer.x - rect.left()) / rect.width())
                        * f32::from(PATTERN_STEPS))
                    .floor()
                    .clamp(0.0, f32::from(PATTERN_STEPS - 1)) as u16;
                    upsert_automation_point(
                        pattern,
                        AutomationParameter::SamplerArticulation,
                        AutomationPoint {
                            step,
                            value: AutomationValue::Choice(articulation_brush.clone()),
                        },
                    );
                }
                if response.clicked_by(egui::PointerButton::Secondary)
                    && let Some(pointer) = response.interact_pointer_pos()
                {
                    let step = (((pointer.x - rect.left()) / rect.width())
                        * f32::from(PATTERN_STEPS))
                    .round() as u16;
                    if let Some(lane) = pattern
                        .automation
                        .iter_mut()
                        .find(|lane| lane.parameter == AutomationParameter::SamplerArticulation)
                    {
                        lane.points.retain(|point| point.step.abs_diff(step) > 1);
                    }
                }
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Filter cutoff · 20 Hz to 20 kHz");
                if ui.button("Clear lane").clicked() {
                    pattern
                        .automation
                        .retain(|lane| lane.parameter != cutoff_parameter);
                }
            });
            ui.weak(
                "Click or drag to draw cutoff. Time runs left to right; frequency runs bottom to top. Right-click a point to remove it.",
            );
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width().min(880.0), 70.0),
                egui::Sense::click_and_drag(),
            );
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 2.0, Color32::from_rgb(25, 29, 36));
            for step in (0..=PATTERN_STEPS).step_by(usize::from(STEPS_PER_BEAT)) {
                let x = rect.left() + f32::from(step) / f32::from(PATTERN_STEPS) * rect.width();
                painter.line_segment(
                    [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                    egui::Stroke::new(1.0, Color32::from_white_alpha(35)),
                );
            }
            painter.text(
                rect.right_top() + egui::vec2(-4.0, 3.0),
                egui::Align2::RIGHT_TOP,
                "20 kHz",
                egui::FontId::monospace(9.0),
                Color32::LIGHT_GRAY,
            );
            painter.text(
                rect.right_bottom() + egui::vec2(-4.0, -3.0),
                egui::Align2::RIGHT_BOTTOM,
                "20 Hz",
                egui::FontId::monospace(9.0),
                Color32::LIGHT_GRAY,
            );
            let lane = pattern
                .automation
                .iter()
                .find(|lane| lane.parameter == cutoff_parameter);
            let mut previous = None;
            if let Some(lane) = lane {
                for point in &lane.points {
                    let AutomationValue::Continuous(value) = point.value else {
                        continue;
                    };
                    let x = rect.left()
                        + f32::from(point.step) / f32::from(PATTERN_STEPS) * rect.width();
                    let normalized = (value.clamp(20.0, 20_000.0) / 20.0).log10() / 3.0;
                    let position = egui::pos2(x, rect.bottom() - normalized * rect.height());
                    if let Some(previous) = previous {
                        painter.line_segment(
                            [previous, position],
                            egui::Stroke::new(2.0, Color32::from_rgb(98, 200, 155)),
                        );
                    }
                    painter.circle_filled(position, 4.0, Color32::from_rgb(98, 200, 155));
                    previous = Some(position);
                }
            } else {
                let normalized = (cutoff_default.clamp(20.0, 20_000.0) / 20.0).log10() / 3.0;
                let y = rect.bottom() - normalized * rect.height();
                painter.line_segment(
                    [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                    egui::Stroke::new(1.0, Color32::from_gray(80)),
                );
            }
            if let Some(pointer) = response.hover_pos() {
                let step = (((pointer.x - rect.left()) / rect.width()) * f32::from(PATTERN_STEPS))
                    .round()
                    .clamp(0.0, f32::from(PATTERN_STEPS - 1)) as u16;
                let normalized = ((rect.bottom() - pointer.y) / rect.height()).clamp(0.0, 1.0);
                let cutoff = 20.0 * 1_000.0_f32.powf(normalized);
                painter.text(
                    pointer + egui::vec2(8.0, -8.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("Step {step} · {cutoff:.0} Hz"),
                    egui::FontId::monospace(10.0),
                    Color32::WHITE,
                );
            }
            if (response.clicked() || response.dragged())
                && let Some(pointer) = response.interact_pointer_pos()
            {
                let step = (((pointer.x - rect.left()) / rect.width()) * f32::from(PATTERN_STEPS))
                    .round()
                    .clamp(0.0, f32::from(PATTERN_STEPS - 1)) as u16;
                let normalized = ((rect.bottom() - pointer.y) / rect.height()).clamp(0.0, 1.0);
                upsert_automation_point(
                    pattern,
                    cutoff_parameter,
                    AutomationPoint {
                        step,
                        value: AutomationValue::Continuous(20.0 * 1_000.0_f32.powf(normalized)),
                    },
                );
            }
            if response.clicked_by(egui::PointerButton::Secondary)
                && let Some(pointer) = response.interact_pointer_pos()
            {
                let step = (((pointer.x - rect.left()) / rect.width()) * f32::from(PATTERN_STEPS))
                    .round() as u16;
                if let Some(lane) = pattern
                    .automation
                    .iter_mut()
                    .find(|lane| lane.parameter == cutoff_parameter)
                {
                    lane.points.retain(|point| point.step.abs_diff(step) > 1);
                }
            }
        });
}

fn upsert_automation_point(
    pattern: &mut Pattern,
    parameter: AutomationParameter,
    point: AutomationPoint,
) {
    let lane = if let Some(index) = pattern
        .automation
        .iter()
        .position(|lane| lane.parameter == parameter)
    {
        &mut pattern.automation[index]
    } else {
        pattern.automation.push(AutomationLane {
            parameter,
            points: Vec::new(),
        });
        pattern
            .automation
            .last_mut()
            .expect("an automation lane was just inserted")
    };
    lane.points.retain(|existing| existing.step != point.step);
    lane.points.push(point);
    lane.points.sort_by_key(|point| point.step);
}

fn sample_region_editor(
    ui: &mut egui::Ui,
    sampler: &mut SampleSynth,
    selected_region: &mut Option<usize>,
) {
    ui.add_space(8.0);
    ui.label("Key and velocity map");
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().min(880.0), 180.0),
        egui::Sense::click(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, Color32::from_rgb(22, 25, 31));
    for velocity in [42_u8, 84] {
        let y = rect.bottom() - f32::from(velocity) / 127.0 * rect.height();
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            egui::Stroke::new(1.0, Color32::from_gray(55)),
        );
    }
    for (index, region) in sampler
        .regions
        .iter()
        .enumerate()
        .filter(|(_, region)| region.articulation == sampler.articulation)
    {
        let left = rect.left() + f32::from(region.key_min - 12) / 121.0 * rect.width();
        let right = rect.left() + f32::from(region.key_max - 11) / 121.0 * rect.width();
        let top = rect.bottom() - f32::from(region.velocity_max) / 127.0 * rect.height();
        let bottom = rect.bottom()
            - f32::from(region.velocity_min.saturating_sub(1)) / 127.0 * rect.height();
        let area = egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom));
        let hue = region.root_pitch.wrapping_mul(29);
        painter.rect_filled(
            area.shrink(1.0),
            2.0,
            Color32::from_rgb(65 + hue % 55, 105 + hue % 70, 145 + hue % 65),
        );
        painter.rect_stroke(
            area,
            2.0,
            egui::Stroke::new(
                if *selected_region == Some(index) {
                    2.0
                } else {
                    1.0
                },
                if *selected_region == Some(index) {
                    Color32::WHITE
                } else {
                    Color32::from_black_alpha(130)
                },
            ),
            egui::StrokeKind::Inside,
        );
        if response.clicked()
            && response
                .interact_pointer_pos()
                .is_some_and(|pointer| area.contains(pointer))
        {
            *selected_region = Some(index);
        }
    }

    painter.text(
        rect.left_top() + egui::vec2(5.0, 4.0),
        egui::Align2::LEFT_TOP,
        "127",
        egui::FontId::monospace(10.0),
        Color32::LIGHT_GRAY,
    );
    painter.text(
        rect.left_bottom() + egui::vec2(5.0, -4.0),
        egui::Align2::LEFT_BOTTOM,
        "1",
        egui::FontId::monospace(10.0),
        Color32::LIGHT_GRAY,
    );

    if let Some(index) = *selected_region
        && let Some(region) = sampler.regions.get_mut(index)
        && region.articulation == sampler.articulation
    {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                region
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Selected region"),
            );
            ui.add(
                egui::Slider::new(&mut region.root_pitch, 12..=132)
                    .text("Root")
                    .integer(),
            );
            ui.add(
                egui::Slider::new(&mut region.key_min, 12..=region.key_max)
                    .text("Key low")
                    .integer(),
            );
            ui.add(
                egui::Slider::new(&mut region.key_max, region.key_min..=132)
                    .text("Key high")
                    .integer(),
            );
            ui.add(
                egui::Slider::new(&mut region.velocity_min, 1..=region.velocity_max)
                    .text("Velocity low")
                    .integer(),
            );
            ui.add(
                egui::Slider::new(&mut region.velocity_max, region.velocity_min..=127)
                    .text("Velocity high")
                    .integer(),
            );
        });
        sampler.path = Some(region.path.clone());
        sampler.root_pitch = region.root_pitch;
    }
}

fn sample_waveform_editor(
    ui: &mut egui::Ui,
    waveform: &[[f32; 2]],
    sampler: &mut crate::model::SampleSynth,
    trim_drag: &mut Option<TrimHandle>,
) {
    let width = ui.available_width().min(880.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::Vec2::new(width, 180.0), egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_rgb(22, 25, 31));
    painter.line_segment(
        [
            egui::pos2(rect.left(), rect.center().y),
            egui::pos2(rect.right(), rect.center().y),
        ],
        egui::Stroke::new(1.0, Color32::from_gray(55)),
    );

    if waveform.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Load a WAV to edit its waveform",
            egui::FontId::proportional(14.0),
            Color32::GRAY,
        );
    } else {
        for (column, [minimum, maximum]) in waveform.iter().enumerate() {
            let x = rect.left() + (column as f32 + 0.5) * rect.width() / waveform.len() as f32;
            let top = rect.center().y - maximum * rect.height() * 0.46;
            let bottom = rect.center().y - minimum * rect.height() * 0.46;
            painter.line_segment(
                [egui::pos2(x, top), egui::pos2(x, bottom)],
                egui::Stroke::new(1.0, Color32::from_rgb(104, 190, 220)),
            );
        }
    }

    let start_x = rect.left() + sampler.trim_start * rect.width();
    let end_x = rect.left() + sampler.trim_end * rect.width();
    painter.rect_filled(
        egui::Rect::from_min_max(rect.min, egui::pos2(start_x, rect.bottom())),
        0.0,
        Color32::from_black_alpha(145),
    );
    painter.rect_filled(
        egui::Rect::from_min_max(egui::pos2(end_x, rect.top()), rect.max),
        0.0,
        Color32::from_black_alpha(145),
    );
    for (x, label) in [(start_x, "START"), (end_x, "END")] {
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(2.0, Color32::from_rgb(255, 184, 77)),
        );
        painter.text(
            egui::pos2(x, rect.top() + 5.0),
            egui::Align2::CENTER_TOP,
            label,
            egui::FontId::monospace(11.0),
            Color32::WHITE,
        );
    }

    if (response.drag_started() || response.clicked())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        *trim_drag = Some(
            if (pointer.x - start_x).abs() <= (pointer.x - end_x).abs() {
                TrimHandle::Start
            } else {
                TrimHandle::End
            },
        );
    }
    if (response.dragged() || response.clicked())
        && let (Some(handle), Some(pointer)) = (*trim_drag, response.interact_pointer_pos())
    {
        let position = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        match handle {
            TrimHandle::Start => sampler.trim_start = position.min(sampler.trim_end - 0.001),
            TrimHandle::End => sampler.trim_end = position.max(sampler.trim_start + 0.001),
        }
    }
    if response.drag_stopped() || response.clicked() {
        *trim_drag = None;
    }
}

fn discover_iowa_regions(folder: &std::path::Path) -> Result<Vec<SampleRegion>, String> {
    let mut pending = vec![folder.to_owned()];
    let mut regions = Vec::new();
    let mut contains_chromatic_scales = false;
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory)
            .map_err(|error| format!("Could not read {}: {error}", directory.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "Could not read an entry in {}: {error}",
                    directory.display()
                )
            })?;
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().and_then(|value| value.to_str()) != Some("wav") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            let parts = stem.split('.').collect::<Vec<_>>();
            contains_chromatic_scales |= parts.iter().any(|part| {
                part.char_indices()
                    .skip(1)
                    .find(|(_, character)| matches!(character, 'A'..='G'))
                    .is_some_and(|(index, _)| {
                        parse_note_pitch(&part[..index]).is_some()
                            && parse_note_pitch(&part[index..]).is_some()
                    })
            });
            let Some(root_pitch) = parts.iter().rev().find_map(|part| parse_note_pitch(part))
            else {
                continue;
            };
            let (velocity_min, velocity_max) = if parts.contains(&"pp") {
                (1, 42)
            } else if parts.contains(&"mf") {
                (43, 84)
            } else if parts.contains(&"ff") {
                (85, 127)
            } else {
                (1, 127)
            };
            let articulation = parts
                .iter()
                .position(|part| matches!(*part, "pp" | "mf" | "ff"))
                .filter(|index| *index > 1)
                .map(|index| parts[1..index].join(" "))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "Standard".to_owned());
            regions.push(SampleRegion {
                path,
                root_pitch,
                key_min: root_pitch,
                key_max: root_pitch,
                velocity_min,
                velocity_max,
                articulation,
            });
        }
    }
    regions.sort_by(|left, right| {
        (&left.articulation, left.root_pitch, left.velocity_min).cmp(&(
            &right.articulation,
            right.root_pitch,
            right.velocity_min,
        ))
    });
    if regions.is_empty() && contains_chromatic_scales {
        return Err(format!(
            "{} contains chromatic-scale WAV files that must be split into individual notes before import",
            folder.display()
        ));
    }
    for index in 0..regions.len() {
        let roots = regions
            .iter()
            .filter(|region| {
                region.articulation == regions[index].articulation
                    && region.velocity_min == regions[index].velocity_min
                    && region.velocity_max == regions[index].velocity_max
            })
            .map(|region| region.root_pitch)
            .collect::<Vec<_>>();
        let root = regions[index].root_pitch;
        let previous = roots.iter().copied().filter(|pitch| *pitch < root).max();
        let next = roots.iter().copied().filter(|pitch| *pitch > root).min();
        regions[index].key_min = previous.map_or(12, |pitch| {
            ((u16::from(pitch) + u16::from(root)) / 2 + 1) as u8
        });
        regions[index].key_max = next.map_or(132, |pitch| {
            ((u16::from(root) + u16::from(pitch)) / 2) as u8
        });
    }
    Ok(regions)
}

fn downloaded_iowa_instruments() -> Vec<PathBuf> {
    let root = PathBuf::from("data/samples/iowa");
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut instruments = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter(|path| discover_iowa_regions(path).is_ok_and(|regions| !regions.is_empty()))
        .collect::<Vec<_>>();
    instruments.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
    instruments
}

fn sampler_from_iowa_folder(folder: &std::path::Path) -> Result<SampleSynth, String> {
    let regions = discover_iowa_regions(folder)?;
    let first = regions.first().ok_or_else(|| {
        format!(
            "No pitch-named WAV files were found in {}",
            folder.display()
        )
    })?;
    Ok(SampleSynth {
        path: Some(first.path.clone()),
        root_pitch: first.root_pitch,
        articulation: first.articulation.clone(),
        regions,
        ..SampleSynth::default()
    })
}

fn parse_note_pitch(value: &str) -> Option<u8> {
    let mut characters = value.chars();
    let semitone = match characters.next()? {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let remainder = characters.as_str();
    let (accidental, octave) = if let Some(octave) = remainder.strip_prefix('b') {
        (-1, octave)
    } else if let Some(octave) = remainder.strip_prefix('#') {
        (1, octave)
    } else {
        (0, remainder)
    };
    let octave = octave.parse::<i16>().ok()?;
    u8::try_from((octave + 1) * 12 + semitone + accidental).ok()
}

#[derive(Default)]
struct SynthKeyboardOutput {
    note_on: Option<u8>,
    note_off: Option<u8>,
}

fn synth_test_keyboard(
    ui: &mut egui::Ui,
    keyboard_notes: &HashSet<u8>,
    mouse_pitch: &mut Option<u8>,
) -> SynthKeyboardOutput {
    const FIRST_PITCH: u8 = 48;
    const LAST_PITCH: u8 = 72;
    const WHITE_WIDTH: f32 = 48.0;
    const HEIGHT: f32 = 130.0;
    let white_count = (FIRST_PITCH..=LAST_PITCH)
        .filter(|pitch| !matches!(pitch % 12, 1 | 3 | 6 | 8 | 10))
        .count();
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(white_count as f32 * WHITE_WIDTH, HEIGHT),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter();
    let mut keys = Vec::with_capacity(25);
    let mut white_index = 0;
    for pitch in FIRST_PITCH..=LAST_PITCH {
        if !matches!(pitch % 12, 1 | 3 | 6 | 8 | 10) {
            let key = egui::Rect::from_min_size(
                egui::pos2(rect.left() + white_index as f32 * WHITE_WIDTH, rect.top()),
                egui::vec2(WHITE_WIDTH, HEIGHT),
            );
            let playing = keyboard_notes.contains(&pitch) || *mouse_pitch == Some(pitch);
            painter.rect_filled(
                key,
                0.0,
                if playing {
                    Color32::from_rgb(68, 164, 119)
                } else {
                    Color32::from_gray(220)
                },
            );
            painter.rect_stroke(
                key,
                0.0,
                egui::Stroke::new(1.0, Color32::from_gray(65)),
                egui::StrokeKind::Inside,
            );
            keys.push((pitch, key, false));
            white_index += 1;
        }
    }
    white_index = 0;
    for pitch in FIRST_PITCH..LAST_PITCH {
        if matches!(pitch % 12, 1 | 3 | 6 | 8 | 10) {
            let key = egui::Rect::from_min_size(
                egui::pos2(
                    rect.left() + white_index as f32 * WHITE_WIDTH - WHITE_WIDTH * 0.31,
                    rect.top(),
                ),
                egui::vec2(WHITE_WIDTH * 0.62, HEIGHT * 0.62),
            );
            let playing = keyboard_notes.contains(&pitch) || *mouse_pitch == Some(pitch);
            painter.rect_filled(
                key,
                2.0,
                if playing {
                    Color32::from_rgb(54, 137, 98)
                } else {
                    Color32::from_gray(35)
                },
            );
            keys.push((pitch, key, true));
        } else {
            white_index += 1;
        }
    }

    let mut output = SynthKeyboardOutput::default();
    if ui.input(|input| input.pointer.primary_released()) {
        output.note_off = mouse_pitch.take();
    }
    if response.hovered()
        && ui.input(|input| input.pointer.primary_pressed())
        && let Some(pointer) = ui.input(|input| input.pointer.press_origin())
        && let Some((pitch, _, _)) = keys.iter().rev().find(|(_, key, _)| key.contains(pointer))
    {
        *mouse_pitch = Some(*pitch);
        output.note_on = Some(*pitch);
    }
    output
}

fn waveform_preview(ui: &mut egui::Ui, synth: &SimpleWaveformSynth) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(680.0, 170.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 5.0, Color32::from_rgb(22, 26, 32));
    ui.painter().line_segment(
        [rect.left_center(), rect.right_center()],
        egui::Stroke::new(1.0, Color32::from_gray(55)),
    );
    let held_ms = 500.0;
    let preview_ms = synth.attack_ms + synth.decay_ms + held_ms + synth.release_ms;
    let note_off_ms = synth.attack_ms + synth.decay_ms + held_ms;
    let mut envelope_points = Vec::with_capacity(257);
    let points = (0..=256)
        .map(|index| {
            let progress = index as f32 / 256.0;
            let time_ms = progress * preview_ms;
            let envelope = if time_ms < synth.attack_ms && synth.attack_ms > 0.0 {
                time_ms / synth.attack_ms
            } else if time_ms < synth.attack_ms + synth.decay_ms && synth.decay_ms > 0.0 {
                let decay = (time_ms - synth.attack_ms) / synth.decay_ms;
                1.0 + (synth.sustain - 1.0) * decay
            } else if time_ms <= note_off_ms {
                synth.sustain
            } else if synth.release_ms > 0.0 {
                synth.sustain * (1.0 - (time_ms - note_off_ms) / synth.release_ms)
            } else {
                0.0
            };
            let phase = progress * 12.0 * 2.0_f32.powf(f32::from(synth.pitch_shift) / 12.0);
            let noise = noise_sample(index as u32);
            let sample = synth
                .layers
                .iter()
                .take(usize::from(synth.layer_count))
                .map(|layer| {
                    let detuned_phase = phase * 2.0_f32.powf(layer.detune_cents / 1_200.0);
                    layer.waveform.sample(detuned_phase, noise) * layer.level
                })
                .sum::<f32>()
                / f32::from(synth.layer_count);
            envelope_points.push(egui::pos2(
                rect.left() + rect.width() * progress,
                rect.center().y - envelope * synth.master_level * rect.height() * 0.42,
            ));
            egui::pos2(
                rect.left() + rect.width() * progress,
                rect.center().y - sample * envelope * synth.master_level * rect.height() * 0.42,
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
        let space_down = context.input(|input| input.key_down(egui::Key::Space));
        if !self.tap_tempo.open && space_down && !self.space_was_down {
            self.toggle_transport();
        }
        self.space_was_down = space_down;
        self.top_bar(root);
        self.track_list(root);
        egui::CentralPanel::default().show(root, |ui| match self.view {
            View::Arrangement => self.arrangement(ui),
            View::PianoRoll => self.editor(ui),
            View::Instrument => self.instrument_settings(ui),
        });
        self.tap_tempo_window(&context);

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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{PIANO_KEYS, TapTempo, discover_iowa_regions, parse_note_pitch};
    use eframe::egui::Key;

    #[test]
    fn typing_keyboard_is_continuous_at_the_row_boundary() {
        let pitch = |key| {
            PIANO_KEYS
                .iter()
                .find(|(candidate, _)| *candidate == key)
                .map(|(_, pitch)| *pitch)
                .expect("tested key is part of the piano mapping")
        };

        assert_eq!(pitch(Key::Slash), 59);
        assert_eq!(pitch(Key::Q), 60);
        assert_eq!(pitch(Key::Num2), 61);
        assert_eq!(pitch(Key::W), 62);
    }

    #[test]
    fn tap_tempo_uses_the_mean_interval_and_resets_after_a_pause() {
        let start = Instant::now();
        let mut tap = TapTempo::default();

        assert_eq!(tap.record(start), None);
        tap.record(start + Duration::from_millis(500));
        tap.record(start + Duration::from_millis(1_000));
        let bpm = tap
            .record(start + Duration::from_millis(1_500))
            .expect("two or more taps should produce a tempo");

        assert!((bpm - 120.0).abs() < f32::EPSILON);
        assert_eq!(tap.record(start + Duration::from_secs(5)), None);
        assert_eq!(tap.taps.len(), 1);
    }

    #[test]
    fn iowa_note_names_map_to_midi_pitches() {
        assert_eq!(parse_note_pitch("C4"), Some(60));
        assert_eq!(parse_note_pitch("Bb3"), Some(58));
        assert_eq!(parse_note_pitch("F#5"), Some(78));
        assert_eq!(parse_note_pitch("stereo"), None);
    }

    #[test]
    fn iowa_import_separates_articulations_and_dynamics() {
        let folder =
            std::env::temp_dir().join(format!("donttrackme-iowa-regions-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("temporary Iowa folder should be created");
        for name in [
            "Violin.arco.pp.C4.wav",
            "Violin.arco.pp.G4.wav",
            "Violin.pizz.ff.D4.wav",
        ] {
            std::fs::File::create(folder.join(name)).expect("empty fixture WAV should be created");
        }

        let regions = discover_iowa_regions(&folder).expect("Iowa filenames should be discovered");
        std::fs::remove_dir_all(folder).expect("temporary Iowa folder should be removed");

        assert_eq!(regions[0].articulation, "arco");
        assert_eq!((regions[0].velocity_min, regions[0].velocity_max), (1, 42));
        assert_eq!((regions[0].key_min, regions[0].key_max), (12, 63));
        assert_eq!((regions[1].key_min, regions[1].key_max), (64, 132));
        assert_eq!(regions[2].articulation, "pizz");
        assert_eq!(
            (regions[2].velocity_min, regions[2].velocity_max),
            (85, 127)
        );
    }

    #[test]
    fn iowa_import_rejects_unsplit_chromatic_scale_recordings() {
        let folder =
            std::env::temp_dir().join(format!("donttrackme-iowa-scale-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("temporary Iowa folder should be created");
        std::fs::File::create(folder.join("Cello.arco.ff.C2B2.wav"))
            .expect("empty chromatic scale fixture should be created");

        let error = discover_iowa_regions(&folder)
            .expect_err("an unsplit chromatic scale must not become one sample region");
        std::fs::remove_dir_all(folder).expect("temporary Iowa folder should be removed");

        assert!(error.contains("must be split into individual notes"));
    }
}
