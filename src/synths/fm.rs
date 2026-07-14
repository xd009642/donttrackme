use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FmAlgorithm {
    Stack,
    TwoPairs,
    ThreeModulators,
    Additive,
}

impl FmAlgorithm {
    pub const ALL: [Self; 4] = [
        Self::Stack,
        Self::TwoPairs,
        Self::ThreeModulators,
        Self::Additive,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Stack => "4 -> 3 -> 2 -> 1",
            Self::TwoPairs => "2 -> 1 + 4 -> 3",
            Self::ThreeModulators => "2 + 3 + 4 -> 1",
            Self::Additive => "1 + 2 + 3 + 4",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct FmOperator {
    pub ratio: f32,
    pub detune_cents: f32,
    pub level: f32,
    pub attack_ms: f32,
    pub decay_ms: f32,
    pub sustain: f32,
    pub release_ms: f32,
}

impl Default for FmOperator {
    fn default() -> Self {
        Self {
            ratio: 1.0,
            detune_cents: 0.0,
            level: 1.0,
            attack_ms: 5.0,
            decay_ms: 250.0,
            sustain: 0.7,
            release_ms: 500.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct FmSynth {
    pub operators: [FmOperator; 4],
    pub algorithm: FmAlgorithm,
    pub feedback: f32,
    pub master_level: f32,
    pub pan: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct FmPreset {
    pub name: &'static str,
    pub category: &'static str,
    pub synth: FmSynth,
}

#[expect(
    clippy::too_many_arguments,
    reason = "the preset factory keeps parallel four-operator parameter arrays visibly aligned"
)]
const fn preset(
    name: &'static str,
    category: &'static str,
    algorithm: FmAlgorithm,
    ratios: [f32; 4],
    levels: [f32; 4],
    decays: [f32; 4],
    sustains: [f32; 4],
    releases: [f32; 4],
    feedback: f32,
) -> FmPreset {
    FmPreset {
        name,
        category,
        synth: FmSynth {
            operators: [
                FmOperator {
                    ratio: ratios[0],
                    detune_cents: 0.0,
                    level: levels[0],
                    attack_ms: 3.0,
                    decay_ms: decays[0],
                    sustain: sustains[0],
                    release_ms: releases[0],
                },
                FmOperator {
                    ratio: ratios[1],
                    detune_cents: 0.0,
                    level: levels[1],
                    attack_ms: 3.0,
                    decay_ms: decays[1],
                    sustain: sustains[1],
                    release_ms: releases[1],
                },
                FmOperator {
                    ratio: ratios[2],
                    detune_cents: 0.0,
                    level: levels[2],
                    attack_ms: 3.0,
                    decay_ms: decays[2],
                    sustain: sustains[2],
                    release_ms: releases[2],
                },
                FmOperator {
                    ratio: ratios[3],
                    detune_cents: 0.0,
                    level: levels[3],
                    attack_ms: 3.0,
                    decay_ms: decays[3],
                    sustain: sustains[3],
                    release_ms: releases[3],
                },
            ],
            algorithm,
            feedback,
            master_level: 0.75,
            pan: 0.0,
        },
    }
}

impl FmSynth {
    pub const PRESETS: &'static [FmPreset] = &[
        preset(
            "Classic electric piano",
            "Keys",
            FmAlgorithm::TwoPairs,
            [1.0, 14.0, 1.0, 1.01],
            [1.0, 0.32, 0.7, 0.22],
            [900.0, 480.0, 1_200.0, 600.0],
            [0.45, 0.0, 0.35, 0.0],
            [1_100.0, 500.0, 1_300.0, 700.0],
            0.08,
        ),
        preset(
            "Warm suitcase piano",
            "Keys",
            FmAlgorithm::TwoPairs,
            [1.0, 2.0, 1.0, 3.01],
            [1.0, 0.36, 0.72, 0.18],
            [1_300.0, 420.0, 1_600.0, 650.0],
            [0.52, 0.0, 0.4, 0.0],
            [1_700.0, 550.0, 1_900.0, 800.0],
            0.03,
        ),
        preset(
            "Soft tine piano",
            "Keys",
            FmAlgorithm::TwoPairs,
            [1.0, 2.0, 1.0, 3.0],
            [1.0, 0.24, 0.6, 0.16],
            [1_400.0, 700.0, 1_600.0, 850.0],
            [0.55, 0.0, 0.4, 0.0],
            [1_600.0, 900.0, 1_800.0, 1_000.0],
            0.04,
        ),
        preset(
            "Drawbar organ",
            "Keys",
            FmAlgorithm::Additive,
            [1.0, 2.0, 3.0, 4.0],
            [1.0, 0.55, 0.32, 0.2],
            [80.0; 4],
            [0.95, 0.85, 0.8, 0.75],
            [180.0; 4],
            0.0,
        ),
        preset(
            "Glass bell",
            "Bells and mallets",
            FmAlgorithm::Stack,
            [1.0, 2.76, 5.4, 8.93],
            [1.0, 0.72, 0.5, 0.34],
            [2_200.0, 1_500.0, 900.0, 600.0],
            [0.0; 4],
            [2_800.0, 1_800.0, 1_100.0, 700.0],
            0.12,
        ),
        preset(
            "Tubular bell",
            "Bells and mallets",
            FmAlgorithm::ThreeModulators,
            [1.0, 1.41, 2.76, 5.43],
            [1.0, 0.58, 0.42, 0.3],
            [3_000.0, 2_200.0, 1_500.0, 900.0],
            [0.0; 4],
            [3_500.0, 2_500.0, 1_700.0, 1_100.0],
            0.16,
        ),
        preset(
            "Digital marimba",
            "Bells and mallets",
            FmAlgorithm::TwoPairs,
            [1.0, 4.0, 1.0, 7.0],
            [1.0, 0.5, 0.55, 0.28],
            [500.0, 260.0, 650.0, 300.0],
            [0.0; 4],
            [650.0, 300.0, 800.0, 350.0],
            0.03,
        ),
        preset(
            "Solid FM bass",
            "Bass",
            FmAlgorithm::Stack,
            [0.5, 1.0, 2.0, 3.0],
            [1.0, 0.65, 0.42, 0.25],
            [320.0, 220.0, 140.0, 90.0],
            [0.72, 0.35, 0.12, 0.0],
            [280.0, 220.0, 160.0, 100.0],
            0.2,
        ),
        preset(
            "Rubber bass",
            "Bass",
            FmAlgorithm::ThreeModulators,
            [0.5, 1.0, 1.5, 4.0],
            [1.0, 0.55, 0.38, 0.3],
            [180.0, 120.0, 90.0, 60.0],
            [0.8, 0.2, 0.0, 0.0],
            [220.0, 160.0, 120.0, 80.0],
            0.38,
        ),
        preset(
            "60s muted electric bass",
            "Bass guitar",
            FmAlgorithm::TwoPairs,
            [1.0, 2.0, 2.0, 3.0],
            [1.0, 0.16, 0.28, 0.07],
            [520.0, 150.0, 360.0, 110.0],
            [0.3, 0.0, 0.1, 0.0],
            [120.0, 70.0, 100.0, 60.0],
            0.0,
        ),
        preset(
            "70s round electric bass",
            "Bass guitar",
            FmAlgorithm::TwoPairs,
            [1.0, 2.0, 2.0, 3.01],
            [1.0, 0.24, 0.34, 0.11],
            [850.0, 260.0, 620.0, 180.0],
            [0.48, 0.0, 0.2, 0.0],
            [220.0, 110.0, 180.0, 90.0],
            0.01,
        ),
        preset(
            "80s bright electric bass",
            "Bass guitar",
            FmAlgorithm::TwoPairs,
            [1.0, 3.0, 2.0, 5.0],
            [1.0, 0.38, 0.4, 0.2],
            [680.0, 190.0, 480.0, 140.0],
            [0.42, 0.0, 0.16, 0.0],
            [170.0, 90.0, 140.0, 70.0],
            0.04,
        ),
        preset(
            "FM brass",
            "Leads and pads",
            FmAlgorithm::Stack,
            [1.0, 1.0, 2.0, 3.0],
            [1.0, 0.5, 0.28, 0.18],
            [450.0, 380.0, 260.0, 180.0],
            [0.8, 0.65, 0.3, 0.1],
            [650.0, 550.0, 380.0, 260.0],
            0.16,
        ),
        preset(
            "Digital choir",
            "Leads and pads",
            FmAlgorithm::Additive,
            [1.0, 2.0, 3.0, 5.0],
            [0.8, 0.38, 0.26, 0.14],
            [900.0; 4],
            [0.85, 0.72, 0.62, 0.5],
            [1_800.0; 4],
            0.0,
        ),
        preset(
            "Crystal pad",
            "Leads and pads",
            FmAlgorithm::TwoPairs,
            [1.0, 2.01, 0.5, 3.99],
            [0.75, 0.3, 0.55, 0.22],
            [1_200.0; 4],
            [0.78, 0.52, 0.7, 0.4],
            [2_400.0; 4],
            0.06,
        ),
        preset(
            "Plucked wire",
            "Plucks and percussion",
            FmAlgorithm::Stack,
            [1.0, 3.0, 7.0, 11.0],
            [1.0, 0.62, 0.4, 0.25],
            [160.0, 110.0, 70.0, 45.0],
            [0.0; 4],
            [130.0, 90.0, 60.0, 40.0],
            0.24,
        ),
        preset(
            "Metal hit",
            "Plucks and percussion",
            FmAlgorithm::ThreeModulators,
            [1.0, 1.41, 3.17, 7.23],
            [0.9, 0.65, 0.48, 0.36],
            [420.0, 300.0, 200.0, 130.0],
            [0.0; 4],
            [500.0, 360.0, 240.0, 160.0],
            0.42,
        ),
        preset(
            "Laser zap",
            "Plucks and percussion",
            FmAlgorithm::Stack,
            [1.0, 8.0, 12.0, 16.0],
            [0.8, 0.72, 0.5, 0.42],
            [120.0, 80.0, 55.0, 35.0],
            [0.0; 4],
            [90.0, 65.0, 45.0, 30.0],
            0.7,
        ),
    ];
}

impl Default for FmSynth {
    fn default() -> Self {
        let mut operators = [FmOperator::default(); 4];
        operators[1].ratio = 2.0;
        operators[1].level = 0.65;
        operators[2].ratio = 3.0;
        operators[2].level = 0.45;
        operators[3].ratio = 4.0;
        operators[3].level = 0.3;
        Self {
            operators,
            algorithm: FmAlgorithm::Stack,
            feedback: 0.15,
            master_level: 0.75,
            pan: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FmAlgorithm, FmSynth};

    #[test]
    fn warm_suitcase_piano_uses_two_decaying_tine_pairs() {
        let preset = FmSynth::PRESETS
            .iter()
            .find(|preset| preset.name == "Warm suitcase piano")
            .expect("the warm suitcase piano preset should exist");

        assert_eq!(preset.category, "Keys");
        assert_eq!(preset.synth.algorithm, FmAlgorithm::TwoPairs);
        assert_eq!(preset.synth.operators[1].sustain, 0.0);
        assert_eq!(preset.synth.operators[3].sustain, 0.0);
        assert!(preset.synth.operators[0].release_ms >= 1_500.0);
        assert!(preset.synth.operators[2].release_ms >= 1_500.0);
    }

    #[test]
    fn electric_bass_decades_progress_from_muted_to_bright() {
        let basses = [
            "60s muted electric bass",
            "70s round electric bass",
            "80s bright electric bass",
        ]
        .map(|name| {
            FmSynth::PRESETS
                .iter()
                .find(|preset| preset.name == name)
                .expect("each electric bass decade preset should exist")
        });

        assert!(basses.iter().all(|preset| preset.category == "Bass guitar"
            && preset.synth.algorithm == FmAlgorithm::TwoPairs));
        assert!(basses[0].synth.operators[0].decay_ms < basses[1].synth.operators[0].decay_ms);
        assert!(basses[0].synth.operators[1].level < basses[1].synth.operators[1].level);
        assert!(basses[1].synth.operators[1].level < basses[2].synth.operators[1].level);
    }

    #[test]
    fn preset_library_covers_core_fm_sound_families() {
        for category in [
            "Keys",
            "Bells and mallets",
            "Bass",
            "Leads and pads",
            "Plucks and percussion",
        ] {
            assert!(
                FmSynth::PRESETS
                    .iter()
                    .any(|preset| preset.category == category)
            );
        }
        assert!(FmSynth::PRESETS.len() >= 12);
    }
}
