use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::FilterKind;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SampleSynth {
    pub path: Option<PathBuf>,
    pub root_pitch: u8,
    pub regions: Vec<SampleRegion>,
    pub articulation: String,
    pub trim_start: f32,
    pub trim_end: f32,
    pub speed: f32,
    pub reverse: bool,
    pub looping: bool,
    pub loop_mode: SampleLoopMode,
    pub gain: f32,
    pub pan: f32,
    pub attack_ms: f32,
    pub decay_ms: f32,
    pub sustain: f32,
    pub release_ms: f32,
    pub filter: FilterKind,
    pub filter_cutoff_hz: f32,
    pub filter_resonance: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SampleRegion {
    pub path: PathBuf,
    pub root_pitch: u8,
    pub key_min: u8,
    pub key_max: u8,
    pub velocity_min: u8,
    pub velocity_max: u8,
    pub articulation: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SampleLoopMode {
    Forward,
    PingPong,
}

impl SampleLoopMode {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Forward => "Forward",
            Self::PingPong => "Bounce",
        }
    }
}

impl Default for SampleSynth {
    fn default() -> Self {
        Self {
            path: None,
            root_pitch: 60,
            regions: Vec::new(),
            articulation: "Standard".to_owned(),
            trim_start: 0.0,
            trim_end: 1.0,
            speed: 1.0,
            reverse: false,
            looping: false,
            loop_mode: SampleLoopMode::Forward,
            gain: 0.8,
            pan: 0.0,
            attack_ms: 0.0,
            decay_ms: 100.0,
            sustain: 1.0,
            release_ms: 80.0,
            filter: FilterKind::Off,
            filter_cutoff_hz: 8_000.0,
            filter_resonance: 0.1,
        }
    }
}
