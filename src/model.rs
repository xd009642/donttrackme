use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::synths::{SampleSynth, SimpleWaveformSynth};

pub const STEPS_PER_BEAT: u16 = 8;
pub const STEPS_PER_BAR: u16 = STEPS_PER_BEAT * 4;
pub const ARRANGEMENT_STEPS: u16 = STEPS_PER_BAR * 8;
pub const PATTERN_STEPS: u16 = STEPS_PER_BAR * 2;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum EffectKind {
    Distortion {
        drive: f32,
        mix: f32,
    },
    Delay {
        time_ms: f32,
        feedback: f32,
        mix: f32,
    },
    Chorus {
        rate_hz: f32,
        depth_ms: f32,
        mix: f32,
    },
    Tremolo {
        rate_hz: f32,
        depth: f32,
    },
    Reverb {
        room_size: f32,
        damping: f32,
        mix: f32,
    },
}

impl EffectKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Distortion { .. } => "Distortion",
            Self::Delay { .. } => "Delay",
            Self::Chorus { .. } => "Chorus",
            Self::Tremolo { .. } => "Tremolo",
            Self::Reverb { .. } => "Reverb",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct EffectSlot {
    pub enabled: bool,
    pub kind: EffectKind,
}

pub const DEFAULT_EFFECTS: [EffectSlot; 5] = [
    EffectSlot {
        enabled: false,
        kind: EffectKind::Distortion {
            drive: 2.5,
            mix: 0.5,
        },
    },
    EffectSlot {
        enabled: false,
        kind: EffectKind::Chorus {
            rate_hz: 0.8,
            depth_ms: 8.0,
            mix: 0.35,
        },
    },
    EffectSlot {
        enabled: false,
        kind: EffectKind::Tremolo {
            rate_hz: 4.0,
            depth: 0.5,
        },
    },
    EffectSlot {
        enabled: false,
        kind: EffectKind::Delay {
            time_ms: 280.0,
            feedback: 0.35,
            mix: 0.3,
        },
    },
    EffectSlot {
        enabled: false,
        kind: EffectKind::Reverb {
            room_size: 0.6,
            damping: 0.45,
            mix: 0.3,
        },
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterKind {
    Off,
    LowPass,
    HighPass,
    BandPass,
}

impl FilterKind {
    pub const ALL: [Self; 4] = [Self::Off, Self::LowPass, Self::HighPass, Self::BandPass];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::LowPass => "Low-pass",
            Self::HighPass => "High-pass",
            Self::BandPass => "Band-pass",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum TrackKind {
    Instrument { synth: SimpleWaveformSynth },
    Sampler { sampler: SampleSynth },
    Sample,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Note {
    pub id: u64,
    pub pitch: u8,
    pub start_step: u16,
    pub length_steps: u16,
    pub velocity: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutomationParameter {
    SamplerArticulation,
    SamplerFilterCutoff,
    SynthFilterCutoff,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AutomationValue {
    Choice(String),
    Continuous(f32),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AutomationPoint {
    pub step: u16,
    pub value: AutomationValue,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AutomationLane {
    pub parameter: AutomationParameter,
    pub points: Vec<AutomationPoint>,
}

impl AutomationLane {
    pub fn value_at(&self, step: u16) -> Option<&AutomationValue> {
        self.points
            .iter()
            .filter(|point| point.step <= step)
            .max_by_key(|point| point.step)
            .map(|point| &point.value)
    }

    pub fn continuous_value_at(&self, step: u16) -> Option<f32> {
        let previous = self
            .points
            .iter()
            .filter(|point| point.step <= step)
            .filter_map(|point| match point.value {
                AutomationValue::Continuous(value) => Some((point.step, value)),
                AutomationValue::Choice(_) => None,
            })
            .max_by_key(|(point_step, _)| *point_step);
        let next = self
            .points
            .iter()
            .filter(|point| point.step > step)
            .filter_map(|point| match point.value {
                AutomationValue::Continuous(value) => Some((point.step, value)),
                AutomationValue::Choice(_) => None,
            })
            .min_by_key(|(point_step, _)| *point_step);
        match (previous, next) {
            (Some((previous_step, previous_value)), Some((next_step, next_value))) => {
                let progress =
                    f32::from(step - previous_step) / f32::from(next_step - previous_step);
                Some(previous_value + (next_value - previous_value) * progress)
            }
            (Some((_, value)), None) | (None, Some((_, value))) => Some(value),
            (None, None) => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Clip {
    pub id: u64,
    pub source_id: u64,
    pub start_step: u16,
    pub length_steps: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClipSource {
    pub id: u64,
    pub channel_id: u64,
    pub name: String,
    pub length_steps: u16,
    pub kind: ClipSourceKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClipSourceKind {
    Pattern { pattern: Pattern },
    Sample { path: PathBuf },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pattern {
    pub notes: Vec<Note>,
    pub automation: Vec<AutomationLane>,
    next_note_id: u64,
}

impl Pattern {
    pub fn add_note(&mut self, pitch: u8, start_step: u16, length_steps: u16, velocity: u8) -> u64 {
        let id = self.next_note_id;
        self.next_note_id += 1;
        self.notes.push(Note {
            id,
            pitch,
            start_step,
            length_steps,
            velocity,
        });
        id
    }
}

impl Default for Pattern {
    fn default() -> Self {
        Self {
            notes: Vec::new(),
            automation: Vec::new(),
            next_note_id: 1,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Track {
    pub id: u64,
    pub name: String,
    pub kind: TrackKind,
    pub source_id: u64,
    pub clips: Vec<Clip>,
    pub muted: bool,
    pub solo: bool,
    pub rendered_from: Option<u64>,
    next_clip_id: u64,
}

impl Track {
    pub fn instrument(id: u64, source_id: u64, name: String) -> Self {
        Self {
            id,
            name,
            kind: TrackKind::Instrument {
                synth: SimpleWaveformSynth::default(),
            },
            source_id,
            clips: Vec::new(),
            muted: false,
            solo: false,
            rendered_from: None,
            next_clip_id: 1,
        }
    }

    pub fn sample(id: u64, source_id: u64, path: PathBuf, length_steps: u16) -> Self {
        let name = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("Sample")
            .to_owned();
        let mut track = Self {
            id,
            name,
            kind: TrackKind::Sample,
            source_id,
            clips: Vec::new(),
            muted: false,
            solo: false,
            rendered_from: None,
            next_clip_id: 1,
        };
        track.add_clip(source_id, 0, length_steps);
        track
    }

    pub fn sampler(id: u64, source_id: u64, name: String, sampler: SampleSynth) -> Self {
        Self {
            id,
            name,
            kind: TrackKind::Sampler { sampler },
            source_id,
            clips: Vec::new(),
            muted: false,
            solo: false,
            rendered_from: None,
            next_clip_id: 1,
        }
    }

    pub fn add_clip(&mut self, source_id: u64, start_step: u16, length_steps: u16) -> u64 {
        let id = self.next_clip_id;
        self.next_clip_id += 1;
        self.clips.push(Clip {
            id,
            source_id,
            start_step,
            length_steps,
        });
        id
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Project {
    pub bpm: f32,
    pub tracks: Vec<Track>,
    pub clip_library: Vec<ClipSource>,
    next_track_id: u64,
    next_source_id: u64,
}

impl Default for Project {
    fn default() -> Self {
        let mut project = Self {
            bpm: 120.0,
            tracks: Vec::new(),
            clip_library: Vec::new(),
            next_track_id: 1,
            next_source_id: 1,
        };
        project.add_instrument();
        project
    }
}

impl Project {
    pub fn add_instrument(&mut self) -> u64 {
        let id = self.next_track_id;
        self.next_track_id += 1;
        let source_id = self.next_source_id;
        self.next_source_id += 1;
        let number = self
            .tracks
            .iter()
            .filter(|track| matches!(track.kind, TrackKind::Instrument { .. }))
            .count()
            + 1;
        let name = format!("Simple waveform {number}");
        self.clip_library.push(ClipSource {
            id: source_id,
            channel_id: id,
            name: format!("Pattern {number}"),
            length_steps: PATTERN_STEPS,
            kind: ClipSourceKind::Pattern {
                pattern: Pattern::default(),
            },
        });
        self.tracks.push(Track::instrument(id, source_id, name));
        id
    }

    pub fn add_sample(&mut self, path: PathBuf) -> u64 {
        self.add_sample_with_length(path, 16)
    }

    pub fn add_sample_with_length(&mut self, path: PathBuf, length_steps: u16) -> u64 {
        let id = self.next_track_id;
        self.next_track_id += 1;
        let source_id = self.next_source_id;
        self.next_source_id += 1;
        let name = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("Sample")
            .to_owned();
        self.clip_library.push(ClipSource {
            id: source_id,
            channel_id: id,
            name,
            length_steps,
            kind: ClipSourceKind::Sample { path: path.clone() },
        });
        self.tracks
            .push(Track::sample(id, source_id, path, length_steps));
        id
    }

    pub fn add_sampler(&mut self) -> u64 {
        let number = self
            .tracks
            .iter()
            .filter(|track| matches!(track.kind, TrackKind::Sampler { .. }))
            .count()
            + 1;
        self.add_configured_sampler(format!("Sampler {number}"), SampleSynth::default())
    }

    pub fn add_configured_sampler(&mut self, name: String, sampler: SampleSynth) -> u64 {
        let id = self.next_track_id;
        self.next_track_id += 1;
        let source_id = self.next_source_id;
        self.next_source_id += 1;
        self.clip_library.push(ClipSource {
            id: source_id,
            channel_id: id,
            name: format!("{name} pattern"),
            length_steps: PATTERN_STEPS,
            kind: ClipSourceKind::Pattern {
                pattern: Pattern::default(),
            },
        });
        self.tracks
            .push(Track::sampler(id, source_id, name, sampler));
        id
    }

    pub fn source(&self, id: u64) -> Option<&ClipSource> {
        self.clip_library.iter().find(|source| source.id == id)
    }

    #[cfg(test)]
    pub fn source_mut(&mut self, id: u64) -> Option<&mut ClipSource> {
        self.clip_library.iter_mut().find(|source| source.id == id)
    }

    pub fn add_pattern(&mut self, channel_id: u64) -> u64 {
        let id = self.next_source_id;
        self.next_source_id += 1;
        let number = self
            .clip_library
            .iter()
            .filter(|source| matches!(source.kind, ClipSourceKind::Pattern { .. }))
            .count()
            + 1;
        self.clip_library.push(ClipSource {
            id,
            channel_id,
            name: format!("Pattern {number}"),
            length_steps: PATTERN_STEPS,
            kind: ClipSourceKind::Pattern {
                pattern: Pattern::default(),
            },
        });
        id
    }

    #[cfg(test)]
    pub fn add_note(
        &mut self,
        pattern_id: u64,
        pitch: u8,
        start_step: u16,
        length_steps: u16,
        velocity: u8,
    ) -> Option<u64> {
        let source = self.source_mut(pattern_id)?;
        let ClipSourceKind::Pattern { pattern } = &mut source.kind else {
            return None;
        };
        Some(pattern.add_note(pitch, start_step, length_steps, velocity))
    }

    pub fn pattern(&self, pattern_id: u64) -> Option<&Pattern> {
        let source = self.source(pattern_id)?;
        let ClipSourceKind::Pattern { pattern } = &source.kind else {
            return None;
        };
        Some(pattern)
    }

    #[cfg(test)]
    pub fn ensure_primary_pattern_clip(&mut self, channel_id: u64) {
        if let Some(track) = self.tracks.iter_mut().find(|track| track.id == channel_id)
            && !track
                .clips
                .iter()
                .any(|clip| clip.source_id == track.source_id)
        {
            track.add_clip(track.source_id, 0, PATTERN_STEPS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AutomationLane, AutomationParameter, AutomationPoint, AutomationValue, PATTERN_STEPS,
        Pattern, Project,
    };
    use crate::synths::{SimpleWaveformSynth, Waveform, noise_sample};

    #[test]
    fn continuous_automation_interpolates_between_points() {
        let lane = AutomationLane {
            parameter: AutomationParameter::SynthFilterCutoff,
            points: vec![
                AutomationPoint {
                    step: 4,
                    value: AutomationValue::Continuous(100.0),
                },
                AutomationPoint {
                    step: 12,
                    value: AutomationValue::Continuous(500.0),
                },
            ],
        };

        assert_eq!(lane.continuous_value_at(0), Some(100.0));
        assert_eq!(lane.continuous_value_at(8), Some(300.0));
        assert_eq!(lane.continuous_value_at(16), Some(500.0));
    }

    #[test]
    fn synth_defaults_to_one_layer_and_presets_respect_the_four_layer_cap() {
        assert_eq!(SimpleWaveformSynth::default().layer_count, 1);
        assert!(
            SimpleWaveformSynth::PRESETS
                .iter()
                .all(|preset| (1..=4).contains(&preset.synth.layer_count))
        );
        for expected in [
            "808-style kick",
            "Snare drum",
            "Closed hi-hat",
            "Violin",
            "Synth strings",
            "Ambient pad",
            "Voice-like pad",
            "Piccolo",
            "Contrabassoon",
            "French horn",
            "Tuba",
            "Viola",
            "Cello",
            "Double bass",
            "Harp",
            "Grand piano",
            "Timpani",
            "Marimba",
            "Tubular bells",
            "Concert bass drum",
            "Crash cymbal",
            "Triangle",
        ] {
            assert!(
                SimpleWaveformSynth::PRESETS
                    .iter()
                    .any(|preset| preset.name == expected)
            );
        }
        for category in [
            "Orchestra · Woodwinds",
            "Orchestra · Brass",
            "Orchestra · Strings",
            "Orchestra · Plucked and keys",
            "Orchestra · Tuned percussion",
            "Orchestra · Percussion",
        ] {
            assert!(
                SimpleWaveformSynth::PRESETS
                    .iter()
                    .any(|preset| preset.category == category)
            );
        }
        let snare = SimpleWaveformSynth::PRESETS
            .iter()
            .find(|preset| preset.name == "Snare drum")
            .expect("the snare preset should exist")
            .synth;
        assert_eq!(snare.layers[0].waveform, Waveform::Sine);
        assert_eq!(snare.layers[1].waveform, Waveform::Noise);
        assert_ne!(snare.pitch_shift, 0);
    }

    #[test]
    fn oscillator_waveforms_have_expected_phase_values() {
        assert!((Waveform::Sine.sample(0.25, 0.0) - 1.0).abs() < f32::EPSILON * 4.0);
        assert_eq!(Waveform::Square.sample(0.25, 0.0), 1.0);
        assert_eq!(Waveform::Square.sample(0.75, 0.0), -1.0);
        assert_eq!(Waveform::Sawtooth.sample(0.5, 0.0), 0.0);
        assert_eq!(Waveform::Noise.sample(0.5, -0.3), -0.3);
        assert_ne!(noise_sample(100), noise_sample(101));
        assert!((-1.0..=1.0).contains(&noise_sample(42)));
    }

    /// Note identities remain distinct so selection survives moving and resizing notes.
    #[test]
    fn notes_receive_stable_unique_ids() {
        let mut pattern = Pattern::default();

        let first = pattern.add_note(60, 0, 1, 100);
        let second = pattern.add_note(64, 4, 2, 90);
        pattern.notes[0].start_step = 8;

        assert_ne!(first, second);
        assert_eq!(pattern.notes[0].id, first);
        assert_eq!(pattern.notes[0].start_step, 8);
    }

    /// Adding more notes reuses the existing pattern clip instead of obscuring it with duplicates.
    #[test]
    fn pattern_clip_is_created_once() {
        let mut project = Project::default();
        let channel_id = project.tracks[0].id;

        project.ensure_primary_pattern_clip(channel_id);
        project.ensure_primary_pattern_clip(channel_id);

        assert_eq!(project.tracks[0].clips.len(), 1);
        assert_eq!(project.tracks[0].clips[0].start_step, 0);
        assert_eq!(project.tracks[0].clips[0].length_steps, PATTERN_STEPS);
    }

    /// Trimming an arrangement instance never changes its reusable library source.
    #[test]
    fn trimming_clip_preserves_source_length() {
        let mut project = Project::default();
        let channel_id = project.tracks[0].id;
        project.ensure_primary_pattern_clip(channel_id);
        let source_id = project.tracks[0].clips[0].source_id;
        project.tracks[0].clips[0].length_steps = 8;

        assert_eq!(
            project.source(source_id).map(|source| source.length_steps),
            Some(PATTERN_STEPS)
        );
        assert_eq!(project.tracks[0].clips[0].length_steps, 8);
    }

    #[test]
    fn patterns_on_one_instrument_have_independent_notes() {
        let mut project = Project::default();
        let channel_id = project.tracks[0].id;
        let first = project.tracks[0].source_id;
        let second = project.add_pattern(channel_id);

        project
            .add_note(first, 60, 0, 2, 100)
            .expect("first pattern should exist");
        project
            .add_note(second, 72, 4, 1, 90)
            .expect("second pattern should exist");

        assert_eq!(
            project
                .pattern(first)
                .expect("first pattern should exist")
                .notes[0]
                .pitch,
            60
        );
        assert_eq!(
            project
                .pattern(second)
                .expect("second pattern should exist")
                .notes[0]
                .pitch,
            72
        );
    }
}
