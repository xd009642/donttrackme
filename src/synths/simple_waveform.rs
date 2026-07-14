use serde::{Deserialize, Serialize};

use crate::model::FilterKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Waveform {
    Sine,
    Square,
    Sawtooth,
    Noise,
}

impl Waveform {
    pub const ALL: [Self; 4] = [Self::Sine, Self::Square, Self::Sawtooth, Self::Noise];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Sine => "Sine",
            Self::Square => "Square",
            Self::Sawtooth => "Sawtooth",
            Self::Noise => "Noise",
        }
    }

    pub fn sample(self, phase: f32, noise_sample: f32) -> f32 {
        match self {
            Self::Sine => (phase * std::f32::consts::TAU).sin(),
            Self::Square => {
                if phase.fract() < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Self::Sawtooth => phase.fract() * 2.0 - 1.0,
            Self::Noise => noise_sample,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct OscillatorLayer {
    pub waveform: Waveform,
    pub level: f32,
    pub detune_cents: f32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SimpleWaveformSynth {
    pub layers: [OscillatorLayer; 4],
    pub layer_count: u8,
    pub master_level: f32,
    pub attack_ms: f32,
    pub decay_ms: f32,
    pub sustain: f32,
    pub release_ms: f32,
    pub pitch_shift: i8,
    pub pan: f32,
    pub mono: bool,
    pub glide_ms: f32,
    pub filter: FilterKind,
    pub filter_cutoff_hz: f32,
    pub filter_resonance: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct SynthPreset {
    pub name: &'static str,
    pub category: &'static str,
    pub synth: SimpleWaveformSynth,
}

pub fn noise_sample(mut seed: u32) -> f32 {
    seed ^= seed >> 16;
    seed = seed.wrapping_mul(0x7feb_352d);
    seed ^= seed >> 15;
    seed = seed.wrapping_mul(0x846c_a68b);
    seed ^= seed >> 16;
    seed as f32 / u32::MAX as f32 * 2.0 - 1.0
}

include!("simple_waveform_presets.rs");
