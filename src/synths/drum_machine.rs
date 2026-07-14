use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct DrumVoice {
    pub tone_hz: f32,
    pub pitch_drop_hz: f32,
    pub tone_decay_ms: f32,
    pub tone_level: f32,
    pub noise_decay_ms: f32,
    pub noise_level: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrumVoiceKind {
    Kick,
    Snare,
    Clap,
    LowTom,
    MidTom,
    HighTom,
    ClosedHat,
    OpenHat,
    Crash,
    Ride,
}

impl DrumVoiceKind {
    pub const ALL: [Self; 10] = [
        Self::Kick,
        Self::Snare,
        Self::Clap,
        Self::LowTom,
        Self::MidTom,
        Self::HighTom,
        Self::ClosedHat,
        Self::OpenHat,
        Self::Crash,
        Self::Ride,
    ];

    pub const fn midi_pitch(self) -> u8 {
        match self {
            Self::Kick => 36,
            Self::Snare => 38,
            Self::Clap => 39,
            Self::LowTom => 41,
            Self::MidTom => 45,
            Self::HighTom => 48,
            Self::ClosedHat => 42,
            Self::OpenHat => 46,
            Self::Crash => 49,
            Self::Ride => 51,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Kick => "Kick",
            Self::Snare => "Snare",
            Self::Clap => "Clap",
            Self::LowTom => "Low tom",
            Self::MidTom => "Mid tom",
            Self::HighTom => "High tom",
            Self::ClosedHat => "Closed hat",
            Self::OpenHat => "Open hat",
            Self::Crash => "Crash",
            Self::Ride => "Ride",
        }
    }

    pub const fn from_midi_pitch(pitch: u8) -> Option<Self> {
        match pitch {
            36 => Some(Self::Kick),
            38 => Some(Self::Snare),
            39 => Some(Self::Clap),
            41 => Some(Self::LowTom),
            45 => Some(Self::MidTom),
            48 => Some(Self::HighTom),
            42 => Some(Self::ClosedHat),
            46 => Some(Self::OpenHat),
            49 => Some(Self::Crash),
            51 => Some(Self::Ride),
            _ => None,
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::Kick => 0,
            Self::Snare => 1,
            Self::Clap => 2,
            Self::LowTom => 3,
            Self::MidTom => 4,
            Self::HighTom => 5,
            Self::ClosedHat => 6,
            Self::OpenHat => 7,
            Self::Crash => 8,
            Self::Ride => 9,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct DrumMachineSynth {
    pub voices: [DrumVoice; 10],
    pub master_level: f32,
    pub pan: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct DrumMachinePreset {
    pub name: &'static str,
    pub category: &'static str,
    pub synth: DrumMachineSynth,
}

const fn voice(
    tone_hz: f32,
    pitch_drop_hz: f32,
    tone_decay_ms: f32,
    tone_level: f32,
    noise_decay_ms: f32,
    noise_level: f32,
) -> DrumVoice {
    DrumVoice {
        tone_hz,
        pitch_drop_hz,
        tone_decay_ms,
        tone_level,
        noise_decay_ms,
        noise_level,
    }
}

const fn kit(
    name: &'static str,
    category: &'static str,
    kick: DrumVoice,
    snare: DrumVoice,
    brightness: f32,
    decay: f32,
) -> DrumMachinePreset {
    DrumMachinePreset {
        name,
        category,
        synth: DrumMachineSynth {
            voices: [
                kick,
                snare,
                voice(900.0, 0.0, 35.0, 0.12, 130.0, 0.75),
                voice(90.0, 90.0, 260.0, 0.9, 100.0, 0.08),
                voice(125.0, 100.0, 220.0, 0.85, 90.0, 0.07),
                voice(175.0, 120.0, 180.0, 0.8, 80.0, 0.06),
                voice(5_800.0 * brightness, 0.0, 30.0, 0.08, 55.0, 0.62),
                voice(5_200.0 * brightness, 0.0, 80.0, 0.1, 300.0 * decay, 0.58),
                voice(3_200.0 * brightness, 0.0, 280.0, 0.12, 850.0 * decay, 0.5),
                voice(4_100.0 * brightness, 0.0, 700.0, 0.2, 520.0 * decay, 0.34),
            ],
            master_level: 0.8,
            pan: 0.0,
        },
    }
}

impl DrumMachineSynth {
    pub const PRESETS: &'static [DrumMachinePreset] = &[
        kit(
            "Classic rock",
            "Acoustic",
            voice(58.0, 105.0, 360.0, 1.0, 80.0, 0.05),
            voice(185.0, 30.0, 180.0, 0.42, 210.0, 0.68),
            0.82,
            0.8,
        ),
        kit(
            "Metal",
            "Acoustic",
            voice(64.0, 155.0, 230.0, 1.0, 45.0, 0.12),
            voice(210.0, 45.0, 130.0, 0.38, 240.0, 0.85),
            1.18,
            0.9,
        ),
        kit(
            "Jazz",
            "Acoustic",
            voice(52.0, 55.0, 300.0, 0.72, 60.0, 0.03),
            voice(175.0, 20.0, 150.0, 0.28, 170.0, 0.42),
            0.9,
            1.2,
        ),
        kit(
            "House",
            "Electronic",
            voice(52.0, 190.0, 420.0, 1.0, 35.0, 0.03),
            voice(205.0, 0.0, 90.0, 0.18, 170.0, 0.68),
            1.05,
            0.85,
        ),
        kit(
            "Trance",
            "Electronic",
            voice(48.0, 220.0, 520.0, 1.0, 30.0, 0.02),
            voice(220.0, 0.0, 80.0, 0.16, 210.0, 0.75),
            1.25,
            1.0,
        ),
        kit(
            "Techno",
            "Electronic",
            voice(55.0, 175.0, 330.0, 1.0, 40.0, 0.06),
            voice(195.0, 25.0, 110.0, 0.3, 150.0, 0.62),
            1.1,
            0.7,
        ),
        kit(
            "Hip-hop",
            "Electronic",
            voice(44.0, 125.0, 620.0, 1.0, 55.0, 0.04),
            voice(170.0, 15.0, 150.0, 0.25, 260.0, 0.72),
            0.78,
            0.9,
        ),
        kit(
            "Drum & bass",
            "Electronic",
            voice(60.0, 180.0, 210.0, 1.0, 35.0, 0.08),
            voice(225.0, 35.0, 95.0, 0.35, 170.0, 0.82),
            1.3,
            0.65,
        ),
    ];
}

impl Default for DrumMachineSynth {
    fn default() -> Self {
        Self::PRESETS[0].synth
    }
}

#[cfg(test)]
mod tests {
    use super::{DrumMachineSynth, DrumVoiceKind};

    #[test]
    fn kits_cover_requested_acoustic_and_electronic_styles() {
        for expected in [
            "Classic rock",
            "Metal",
            "Jazz",
            "House",
            "Trance",
            "Techno",
            "Hip-hop",
            "Drum & bass",
        ] {
            assert!(
                DrumMachineSynth::PRESETS
                    .iter()
                    .any(|preset| preset.name == expected)
            );
        }
        assert!(
            DrumVoiceKind::ALL.iter().all(|voice| {
                DrumVoiceKind::from_midi_pitch(voice.midi_pitch()) == Some(*voice)
            })
        );
    }
}
