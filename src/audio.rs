use std::{
    collections::HashMap,
    path::Path,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
};

use cpal::{
    BufferSize, FromSample, Sample, SampleFormat, SizedSample, Stream, SupportedBufferSize,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

use crate::model::{
    ARRANGEMENT_STEPS, ArpeggiatorOrder, ArpeggiatorSettings, AutomationParameter, AutomationValue,
    DEFAULT_EFFECTS, EffectKind, EffectSlot, FilterKind, Pattern, Project, STEPS_PER_BEAT,
    TrackKind,
};
use crate::synths::{
    DrumMachineSynth, DrumVoiceKind, FmAlgorithm, FmOperator, FmSynth, SampleLoopMode, SampleSynth,
    SimpleWaveformSynth, noise_sample,
};

enum Command {
    Play(PlaybackPlan),
    Stop,
    Pause,
    Resume,
    Seek(u64),
    SetLoopRange(Option<(u64, u64)>),
    AuditionStart {
        pitch: u8,
        synth: SimpleWaveformSynth,
        effects: [EffectSlot; 5],
        arpeggiator: ArpeggiatorSettings,
        bpm: f32,
    },
    AuditionFmStart {
        pitch: u8,
        synth: FmSynth,
        effects: [EffectSlot; 5],
        arpeggiator: ArpeggiatorSettings,
        bpm: f32,
    },
    AuditionDrumStart {
        pitch: u8,
        synth: DrumMachineSynth,
        effects: [EffectSlot; 5],
    },
    AuditionStop {
        pitch: u8,
    },
    AuditionSampleStart {
        pitch: u8,
        root_pitch: u8,
        sampler: SampleSynth,
        sample: Arc<SampleBuffer>,
        effects: [EffectSlot; 5],
        arpeggiator: ArpeggiatorSettings,
        bpm: f32,
    },
}

struct PlaybackPlan {
    channels: Vec<ChannelPlan>,
    loop_samples: u64,
}

struct ChannelPlan {
    instrument: RenderInstrument,
    voices: Vec<Voice>,
    effects: EffectChain,
    next_drum_voice: usize,
    active_drum_voices: Vec<ActiveDrumVoice>,
}

struct ActiveDrumVoice {
    pitch: u8,
    start_sample: u64,
    gain: f32,
}

enum RenderInstrument {
    Synth(SimpleWaveformSynth),
    Fm(FmSynth),
    DrumMachine(DrumMachineSynth),
    Sampler { sampler: SampleSynth },
}

struct SampleBuffer {
    frames: Vec<[f32; 2]>,
    sample_rate: f32,
}

struct Voice {
    pitch: u8,
    start_sample: u64,
    note_off_sample: u64,
    frequency: f32,
    glide_from_frequency: f32,
    gain: f32,
    filter: FilterState,
    sample: Option<Arc<SampleBuffer>>,
    // TODO: Evaluate continuous automation throughout sustained notes. The first automation
    // slice samples filter cutoff at note-on, which proves shared synth/sampler targeting but
    // does not yet sweep a note that is already sounding.
    automated_filter_cutoff: Option<f32>,
    fm_operator_frequencies: [f32; 4],
    fm_feedback: f32,
}

#[derive(Default)]
struct FilterState {
    low: f32,
    band: f32,
}

struct EffectChain {
    slots: [EffectSlot; 5],
    states: Vec<EffectState>,
}

enum EffectState {
    Distortion,
    Delay {
        buffer: Vec<[f32; 2]>,
        position: usize,
    },
    Chorus {
        buffer: Vec<[f32; 2]>,
        position: usize,
        phase: f32,
    },
    Tremolo {
        phase: f32,
    },
    Reverb {
        left: Vec<f32>,
        right: Vec<f32>,
        left_position: usize,
        right_position: usize,
        damped: [f32; 2],
    },
}

pub struct AudioEngine {
    _stream: Stream,
    commands: Sender<Command>,
    sample_rate: f32,
    sample_cache: Mutex<HashMap<PathBuf, Arc<SampleBuffer>>>,
}

impl AudioEngine {
    pub fn new() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "No default audio output device is available".to_owned())?;
        let supported = device
            .default_output_config()
            .map_err(|error| format!("Could not read the default audio configuration: {error}"))?;
        let sample_rate = supported.sample_rate() as f32;
        let buffer_size = match supported.buffer_size() {
            SupportedBufferSize::Range { min, max } => {
                BufferSize::Fixed(1_024_u32.clamp(*min, *max))
            }
            SupportedBufferSize::Unknown => BufferSize::Default,
        };
        let mut config = supported.config();
        config.buffer_size = buffer_size;
        let (commands, receiver) = mpsc::channel();

        let stream = match supported.sample_format() {
            SampleFormat::I8 => build_stream::<i8>(&device, &config, receiver),
            SampleFormat::I16 => build_stream::<i16>(&device, &config, receiver),
            SampleFormat::I24 => build_stream::<cpal::I24>(&device, &config, receiver),
            SampleFormat::I32 => build_stream::<i32>(&device, &config, receiver),
            SampleFormat::I64 => build_stream::<i64>(&device, &config, receiver),
            SampleFormat::U8 => build_stream::<u8>(&device, &config, receiver),
            SampleFormat::U16 => build_stream::<u16>(&device, &config, receiver),
            SampleFormat::U24 => build_stream::<cpal::U24>(&device, &config, receiver),
            SampleFormat::U32 => build_stream::<u32>(&device, &config, receiver),
            SampleFormat::U64 => build_stream::<u64>(&device, &config, receiver),
            SampleFormat::F32 => build_stream::<f32>(&device, &config, receiver),
            SampleFormat::F64 => build_stream::<f64>(&device, &config, receiver),
            format => return Err(format!("The output sample format {format} is unsupported")),
        }?;
        stream
            .play()
            .map_err(|error| format!("Could not start the audio output stream: {error}"))?;

        Ok(Self {
            _stream: stream,
            commands,
            sample_rate,
            sample_cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn play(&self, project: &Project) -> Result<(), String> {
        self.commands
            .send(Command::Play(PlaybackPlan::from_project(
                project,
                self.sample_rate,
            )?))
            .map_err(|_| "The audio output stream has stopped".to_owned())
    }

    pub fn stop(&self) -> Result<(), String> {
        self.commands
            .send(Command::Stop)
            .map_err(|_| "The audio output stream has stopped".to_owned())
    }

    pub fn pause(&self) -> Result<(), String> {
        self.commands
            .send(Command::Pause)
            .map_err(|_| "The audio output stream has stopped".to_owned())
    }

    pub fn resume(&self) -> Result<(), String> {
        self.commands
            .send(Command::Resume)
            .map_err(|_| "The audio output stream has stopped".to_owned())
    }

    pub fn seek_step(&self, bpm: f32, step: f32) -> Result<(), String> {
        let sample =
            (step * self.sample_rate * 60.0 / bpm / f32::from(STEPS_PER_BEAT)).round() as u64;
        self.commands
            .send(Command::Seek(sample))
            .map_err(|_| "The audio output stream has stopped".to_owned())
    }

    pub fn set_loop_steps(&self, bpm: f32, range: Option<(f32, f32)>) -> Result<(), String> {
        let samples_per_step = self.sample_rate * 60.0 / bpm / f32::from(STEPS_PER_BEAT);
        let range = range.map(|(start, end)| {
            (
                (start * samples_per_step).round() as u64,
                (end * samples_per_step).round() as u64,
            )
        });
        self.commands
            .send(Command::SetLoopRange(range))
            .map_err(|_| "The audio output stream has stopped".to_owned())
    }

    pub fn play_pattern(&self, project: &Project, pattern_id: u64) -> Result<(), String> {
        self.commands
            .send(Command::Play(PlaybackPlan::from_pattern(
                project,
                pattern_id,
                self.sample_rate,
            )?))
            .map_err(|_| "The audio output stream has stopped".to_owned())
    }

    pub fn audition_start(
        &self,
        pitch: u8,
        synth: SimpleWaveformSynth,
        effects: [EffectSlot; 5],
        arpeggiator: ArpeggiatorSettings,
        bpm: f32,
    ) -> Result<(), String> {
        self.commands
            .send(Command::AuditionStart {
                pitch,
                synth,
                effects,
                arpeggiator,
                bpm,
            })
            .map_err(|_| "The audio output stream has stopped".to_owned())
    }

    pub fn audition_fm_start(
        &self,
        pitch: u8,
        synth: FmSynth,
        effects: [EffectSlot; 5],
        arpeggiator: ArpeggiatorSettings,
        bpm: f32,
    ) -> Result<(), String> {
        self.commands
            .send(Command::AuditionFmStart {
                pitch,
                synth,
                effects,
                arpeggiator,
                bpm,
            })
            .map_err(|_| "The audio output stream has stopped".to_owned())
    }

    pub fn audition_drum_start(
        &self,
        pitch: u8,
        synth: DrumMachineSynth,
        effects: [EffectSlot; 5],
    ) -> Result<(), String> {
        self.commands
            .send(Command::AuditionDrumStart {
                pitch,
                synth,
                effects,
            })
            .map_err(|_| "The audio output stream has stopped".to_owned())
    }

    pub fn audition_stop(&self, pitch: u8) -> Result<(), String> {
        self.commands
            .send(Command::AuditionStop { pitch })
            .map_err(|_| "The audio output stream has stopped".to_owned())
    }

    pub fn audition_sample_start(
        &self,
        pitch: u8,
        sampler: SampleSynth,
        effects: [EffectSlot; 5],
        arpeggiator: ArpeggiatorSettings,
        bpm: f32,
    ) -> Result<(), String> {
        let Some((path, root_pitch)) =
            select_sample_region(&sampler, pitch, 127, &sampler.articulation)
        else {
            return Err("Load a WAV into the sampler first".to_owned());
        };
        let mut cache = self
            .sample_cache
            .lock()
            .map_err(|_| "The sample cache is unavailable".to_owned())?;
        let sample = if let Some(sample) = cache.get(path) {
            Arc::clone(sample)
        } else {
            let sample = Arc::new(load_wav(path)?);
            cache.insert(path.to_owned(), Arc::clone(&sample));
            sample
        };
        self.commands
            .send(Command::AuditionSampleStart {
                pitch,
                root_pitch,
                sampler,
                sample,
                effects,
                arpeggiator,
                bpm,
            })
            .map_err(|_| "The audio output stream has stopped".to_owned())
    }
}

pub fn export_wav(project: &Project, path: &Path) -> Result<(), String> {
    const SAMPLE_RATE: u32 = 44_100;
    let plan = PlaybackPlan::from_project(project, SAMPLE_RATE as f32)?;
    let frame_count = plan.loop_samples;
    let (_sender, receiver) = mpsc::channel();
    let mut renderer = Renderer::new(receiver, SAMPLE_RATE as f32);
    renderer.plan = Some(plan);
    let specification = hound::WavSpec {
        channels: 2,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, specification)
        .map_err(|error| format!("Could not create the WAV file: {error}"))?;
    for _ in 0..frame_count {
        let [left, right] = renderer.next_frame();
        writer
            .write_sample((left.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16)
            .and_then(|_| {
                writer.write_sample((right.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16)
            })
            .map_err(|error| format!("Could not write WAV audio: {error}"))?;
    }
    writer
        .finalize()
        .map_err(|error| format!("Could not finish the WAV file: {error}"))
}

pub fn export_track_wav(project: &Project, track_id: u64, path: &Path) -> Result<(), String> {
    const SAMPLE_RATE: u32 = 44_100;
    let plan = PlaybackPlan::from_project_channel(project, SAMPLE_RATE as f32, Some(track_id))?;
    let frame_count = plan.loop_samples;
    let (_sender, receiver) = mpsc::channel();
    let mut renderer = Renderer::new(receiver, SAMPLE_RATE as f32);
    renderer.plan = Some(plan);
    let specification = hound::WavSpec {
        channels: 2,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, specification)
        .map_err(|error| format!("Could not create the rendered track: {error}"))?;
    for _ in 0..frame_count {
        let [left, right] = renderer.next_frame();
        writer
            .write_sample((left.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16)
            .and_then(|_| {
                writer.write_sample((right.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16)
            })
            .map_err(|error| format!("Could not write the rendered track: {error}"))?;
    }
    writer
        .finalize()
        .map_err(|error| format!("Could not finish the rendered track: {error}"))
}

impl PlaybackPlan {
    fn from_project(project: &Project, sample_rate: f32) -> Result<Self, String> {
        Self::from_project_channel(project, sample_rate, None)
    }

    fn from_project_channel(
        project: &Project,
        sample_rate: f32,
        only_track: Option<u64>,
    ) -> Result<Self, String> {
        let samples_per_step = sample_rate * 60.0 / project.bpm / f32::from(STEPS_PER_BEAT);
        let any_solo = project.tracks.iter().any(|track| track.solo);
        let mut channels = Vec::new();
        let mut decoded_samples = HashMap::<PathBuf, Arc<SampleBuffer>>::new();

        for channel in &project.tracks {
            if only_track.is_some_and(|track_id| channel.id != track_id) {
                continue;
            }
            if only_track.is_none() && (channel.muted || (any_solo && !channel.solo)) {
                continue;
            }
            let instrument = match &channel.kind {
                TrackKind::Instrument { synth } => RenderInstrument::Synth(*synth),
                TrackKind::Fm { synth } => RenderInstrument::Fm(*synth),
                TrackKind::DrumMachine { synth } => RenderInstrument::DrumMachine(*synth),
                TrackKind::Sampler { sampler } => {
                    if sampler.path.is_none() && sampler.regions.is_empty() {
                        continue;
                    }
                    RenderInstrument::Sampler {
                        sampler: sampler.clone(),
                    }
                }
                TrackKind::Sample => RenderInstrument::Sampler {
                    sampler: SampleSynth::default(),
                },
            };
            let mut channel_voices = Vec::new();
            if matches!(channel.kind, TrackKind::Sample) {
                for clip in &channel.clips {
                    let Some(crate::model::ClipSource {
                        kind: crate::model::ClipSourceKind::Sample { path },
                        ..
                    }) = project.source(clip.source_id)
                    else {
                        continue;
                    };
                    let sample = if let Some(sample) = decoded_samples.get(path) {
                        Arc::clone(sample)
                    } else {
                        let sample = Arc::new(load_wav(path)?);
                        decoded_samples.insert(path.clone(), Arc::clone(&sample));
                        sample
                    };
                    channel_voices.push(Voice {
                        pitch: 60,
                        start_sample: (f32::from(clip.start_step) * samples_per_step).round()
                            as u64,
                        note_off_sample: (f32::from(clip.start_step + clip.length_steps)
                            * samples_per_step)
                            .round() as u64,
                        frequency: 1.0,
                        glide_from_frequency: 1.0,
                        gain: 1.0,
                        filter: FilterState::default(),
                        sample: Some(sample),
                        automated_filter_cutoff: None,
                        fm_operator_frequencies: [0.0; 4],
                        fm_feedback: 0.0,
                    });
                }
            } else {
                for lane in project.tracks.iter().filter(|lane| !lane.muted) {
                    for clip in &lane.clips {
                        let Some(source) = project.source(clip.source_id) else {
                            continue;
                        };
                        if source.channel_id != channel.id {
                            continue;
                        }
                        let Some(pattern) = project.pattern(source.id) else {
                            continue;
                        };
                        for note in &pattern.notes {
                            if note.start_step >= clip.length_steps {
                                continue;
                            }
                            let start_step = clip.start_step + note.start_step;
                            if start_step >= ARRANGEMENT_STEPS {
                                continue;
                            }
                            let end_step = clip.start_step
                                + (note.start_step + note.length_steps).min(clip.length_steps);
                            let frequency = match &instrument {
                                RenderInstrument::Synth(synth) => {
                                    pitch_frequency(note.pitch, synth.pitch_shift)
                                }
                                RenderInstrument::Fm(_) => pitch_frequency(note.pitch, 0),
                                RenderInstrument::DrumMachine(_) => 0.0,
                                RenderInstrument::Sampler { sampler } => {
                                    let (_, root_pitch) = select_sample_region(
                                        sampler,
                                        note.pitch,
                                        note.velocity,
                                        pattern_articulation(
                                            pattern,
                                            note.start_step,
                                            &sampler.articulation,
                                        ),
                                    )
                                    .expect("loaded sampler has at least one sample region");
                                    2.0_f32.powf(
                                        (f32::from(note.pitch) - f32::from(root_pitch)) / 12.0,
                                    ) * sampler.speed
                                }
                            };
                            let sample = match &instrument {
                                RenderInstrument::Sampler { sampler } => {
                                    let (path, _) = select_sample_region(
                                        sampler,
                                        note.pitch,
                                        note.velocity,
                                        pattern_articulation(
                                            pattern,
                                            note.start_step,
                                            &sampler.articulation,
                                        ),
                                    )
                                    .expect("loaded sampler has at least one sample region");
                                    if let Some(sample) = decoded_samples.get(path) {
                                        Some(Arc::clone(sample))
                                    } else {
                                        let sample = Arc::new(load_wav(path)?);
                                        decoded_samples
                                            .insert(path.to_owned(), Arc::clone(&sample));
                                        Some(sample)
                                    }
                                }
                                RenderInstrument::Synth(_) => None,
                                RenderInstrument::Fm(_) => None,
                                RenderInstrument::DrumMachine(_) => None,
                            };
                            channel_voices.push(Voice {
                                pitch: note.pitch,
                                start_sample: (f32::from(start_step) * samples_per_step).round()
                                    as u64,
                                note_off_sample: (f32::from(end_step) * samples_per_step).round()
                                    as u64,
                                frequency,
                                glide_from_frequency: frequency,
                                gain: f32::from(note.velocity) / 127.0,
                                filter: FilterState::default(),
                                sample,
                                automated_filter_cutoff: pattern_filter_cutoff(
                                    pattern,
                                    note.start_step,
                                    &instrument,
                                ),
                                fm_operator_frequencies: match &instrument {
                                    RenderInstrument::Fm(synth) => {
                                        fm_operator_frequencies(synth, frequency)
                                    }
                                    _ => [0.0; 4],
                                },
                                fm_feedback: 0.0,
                            });
                        }
                    }
                }
            }
            channel_voices.sort_by_key(|voice| voice.start_sample);
            if matches!(&instrument, RenderInstrument::Synth(synth) if synth.mono) {
                for index in 0..channel_voices.len() {
                    if index > 0 {
                        channel_voices[index].glide_from_frequency =
                            channel_voices[index - 1].frequency;
                    }
                    if index + 1 < channel_voices.len() {
                        channel_voices[index].note_off_sample = channel_voices[index]
                            .note_off_sample
                            .min(channel_voices[index + 1].start_sample);
                    }
                }
            }
            if !channel_voices.is_empty() {
                channels.push(ChannelPlan {
                    effects: EffectChain::new(channel.effects, sample_rate),
                    instrument,
                    voices: channel_voices,
                    next_drum_voice: 0,
                    active_drum_voices: Vec::new(),
                });
            }
        }

        Ok(Self {
            channels,
            loop_samples: (f32::from(ARRANGEMENT_STEPS) * samples_per_step).round() as u64,
        })
    }

    fn from_pattern(project: &Project, pattern_id: u64, sample_rate: f32) -> Result<Self, String> {
        let source = project
            .source(pattern_id)
            .ok_or_else(|| "The selected pattern is no longer in the library".to_owned())?;
        let pattern = project
            .pattern(pattern_id)
            .ok_or_else(|| "The selected clip is not a pattern".to_owned())?;
        let channel = project
            .tracks
            .iter()
            .find(|track| track.id == source.channel_id)
            .ok_or_else(|| "The pattern's instrument channel is missing".to_owned())?;
        let instrument = match &channel.kind {
            TrackKind::Instrument { synth } => RenderInstrument::Synth(*synth),
            TrackKind::Fm { synth } => RenderInstrument::Fm(*synth),
            TrackKind::DrumMachine { synth } => RenderInstrument::DrumMachine(*synth),
            TrackKind::Sampler { sampler } => {
                if sampler.path.is_none() && sampler.regions.is_empty() {
                    return Err("Load a WAV into the sampler first".to_owned());
                }
                RenderInstrument::Sampler {
                    sampler: sampler.clone(),
                }
            }
            TrackKind::Sample => return Err("Sample tracks do not have patterns".to_owned()),
        };
        let samples_per_step = sample_rate * 60.0 / project.bpm / f32::from(STEPS_PER_BEAT);
        let mut decoded_samples = HashMap::<PathBuf, Arc<SampleBuffer>>::new();
        let mut voices = pattern
            .notes
            .iter()
            .map(|note| -> Result<Voice, String> {
                let frequency = match &instrument {
                    RenderInstrument::Synth(synth) => {
                        pitch_frequency(note.pitch, synth.pitch_shift)
                    }
                    RenderInstrument::Fm(_) => pitch_frequency(note.pitch, 0),
                    RenderInstrument::DrumMachine(_) => 0.0,
                    RenderInstrument::Sampler { sampler } => {
                        let (_, root_pitch) = select_sample_region(
                            sampler,
                            note.pitch,
                            note.velocity,
                            pattern_articulation(pattern, note.start_step, &sampler.articulation),
                        )
                        .expect("loaded sampler has at least one sample region");
                        2.0_f32.powf((f32::from(note.pitch) - f32::from(root_pitch)) / 12.0)
                            * sampler.speed
                    }
                };
                let sample = match &instrument {
                    RenderInstrument::Sampler { sampler } => {
                        let (path, _) = select_sample_region(
                            sampler,
                            note.pitch,
                            note.velocity,
                            pattern_articulation(pattern, note.start_step, &sampler.articulation),
                        )
                        .expect("loaded sampler has at least one sample region");
                        if let Some(sample) = decoded_samples.get(path) {
                            Some(Arc::clone(sample))
                        } else {
                            let sample = Arc::new(load_wav(path)?);
                            decoded_samples.insert(path.to_owned(), Arc::clone(&sample));
                            Some(sample)
                        }
                    }
                    RenderInstrument::Synth(_) => None,
                    RenderInstrument::Fm(_) => None,
                    RenderInstrument::DrumMachine(_) => None,
                };
                Ok(Voice {
                    pitch: note.pitch,
                    start_sample: (f32::from(note.start_step) * samples_per_step).round() as u64,
                    note_off_sample: (f32::from(note.start_step + note.length_steps)
                        * samples_per_step)
                        .round() as u64,
                    frequency,
                    glide_from_frequency: frequency,
                    gain: f32::from(note.velocity) / 127.0,
                    filter: FilterState::default(),
                    sample,
                    automated_filter_cutoff: pattern_filter_cutoff(
                        pattern,
                        note.start_step,
                        &instrument,
                    ),
                    fm_operator_frequencies: match &instrument {
                        RenderInstrument::Fm(synth) => fm_operator_frequencies(synth, frequency),
                        _ => [0.0; 4],
                    },
                    fm_feedback: 0.0,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        voices.sort_by_key(|voice| voice.start_sample);
        if matches!(&instrument, RenderInstrument::Synth(synth) if synth.mono) {
            for index in 1..voices.len() {
                voices[index].glide_from_frequency = voices[index - 1].frequency;
                voices[index - 1].note_off_sample = voices[index - 1]
                    .note_off_sample
                    .min(voices[index].start_sample);
            }
        }
        let effects = EffectChain::new(channel.effects, sample_rate);
        Ok(Self {
            channels: vec![ChannelPlan {
                instrument,
                voices,
                effects,
                next_drum_voice: 0,
                active_drum_voices: Vec::new(),
            }],
            loop_samples: (f32::from(source.length_steps) * samples_per_step).round() as u64,
        })
    }
}

impl ChannelPlan {
    fn reset_drum_schedule(&mut self, position: u64, sample_rate: f32) {
        self.active_drum_voices.clear();
        let RenderInstrument::DrumMachine(synth) = &self.instrument else {
            return;
        };
        self.next_drum_voice = self
            .voices
            .partition_point(|voice| voice.start_sample <= position);
        self.active_drum_voices.extend(
            self.voices[..self.next_drum_voice]
                .iter()
                .filter(|voice| {
                    position - voice.start_sample
                        < drum_voice_duration_samples(synth, voice.pitch, sample_rate)
                })
                .map(|voice| ActiveDrumVoice {
                    pitch: voice.pitch,
                    start_sample: voice.start_sample,
                    gain: voice.gain,
                }),
        );
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    receiver: Receiver<Command>,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let mut renderer = Renderer::new(receiver, config.sample_rate as f32);
    device
        .build_output_stream(
            *config,
            move |output: &mut [T], _| renderer.render(output, channels),
            |error| eprintln!("Audio output error: {error}"),
            None,
        )
        .map_err(|error| format!("Could not create the audio output stream: {error}"))
}

struct Renderer {
    receiver: Receiver<Command>,
    plan: Option<PlaybackPlan>,
    position: u64,
    paused: bool,
    loop_range: Option<(u64, u64)>,
    sample_rate: f32,
    audition_voices: Vec<AuditionVoice>,
    audition_effects: Option<EffectChain>,
    audition_samples: Vec<AuditionSampleVoice>,
    audition_fm: Vec<AuditionFmVoice>,
    audition_drums: Vec<AuditionDrumVoice>,
    held_arpeggiator_notes: Vec<HeldArpeggiatorNote>,
    arpeggiator: ArpeggiatorSettings,
    arpeggiator_bpm: f32,
    arpeggiator_step_remaining: u64,
    arpeggiator_gate_remaining: u64,
    arpeggiator_index: usize,
}

struct AuditionVoice {
    pitch: u8,
    synth: SimpleWaveformSynth,
    frequency: f32,
    glide_from_frequency: f32,
    glide_elapsed: u64,
    elapsed: u64,
    released_at: Option<(u64, f32)>,
    filter: FilterState,
}

struct AuditionSampleVoice {
    pitch: u8,
    sampler: SampleSynth,
    sample: Arc<SampleBuffer>,
    playback_rate: f32,
    elapsed: u64,
    released_at: Option<(u64, f32)>,
    filter: FilterState,
    finished: bool,
}

struct AuditionFmVoice {
    pitch: u8,
    synth: FmSynth,
    elapsed: u64,
    released_at: Option<u64>,
    feedback: f32,
    operator_frequencies: [f32; 4],
}

struct AuditionDrumVoice {
    pitch: u8,
    synth: DrumMachineSynth,
    elapsed: u64,
}

#[derive(Clone)]
enum HeldArpeggiatorInstrument {
    Synth(SimpleWaveformSynth),
    Fm(FmSynth),
    Sampler {
        sampler: SampleSynth,
        sample: Arc<SampleBuffer>,
        root_pitch: u8,
    },
}

#[derive(Clone)]
struct HeldArpeggiatorNote {
    pitch: u8,
    instrument: HeldArpeggiatorInstrument,
}

impl Renderer {
    fn new(receiver: Receiver<Command>, sample_rate: f32) -> Self {
        Self {
            receiver,
            plan: None,
            position: 0,
            paused: false,
            loop_range: None,
            sample_rate,
            audition_voices: Vec::with_capacity(40),
            audition_effects: None,
            audition_samples: Vec::with_capacity(40),
            audition_fm: Vec::with_capacity(40),
            audition_drums: Vec::with_capacity(40),
            held_arpeggiator_notes: Vec::new(),
            arpeggiator: ArpeggiatorSettings::default(),
            arpeggiator_bpm: 120.0,
            arpeggiator_step_remaining: 0,
            arpeggiator_gate_remaining: 0,
            arpeggiator_index: 0,
        }
    }

    fn render<T>(&mut self, output: &mut [T], channels: usize)
    where
        T: Sample + FromSample<f32>,
    {
        self.receive_commands();
        for frame in output.chunks_mut(channels) {
            let [left, right] = self.next_frame();
            if channels == 1 {
                frame[0] = T::from_sample((left + right) * 0.5);
            } else {
                for (index, sample) in frame.iter_mut().enumerate() {
                    *sample = T::from_sample(if index % 2 == 0 { left } else { right });
                }
            }
        }
    }

    fn receive_commands(&mut self) {
        while let Ok(command) = self.receiver.try_recv() {
            match command {
                Command::Play(plan) => {
                    self.plan = Some(plan);
                    self.position = 0;
                    self.paused = false;
                }
                Command::Stop => {
                    self.plan = None;
                    self.position = 0;
                    self.paused = false;
                }
                Command::Pause => self.paused = true,
                Command::Resume => {
                    if self.plan.is_some() {
                        self.paused = false;
                    }
                }
                Command::Seek(position) => {
                    if let Some(plan) = &mut self.plan {
                        self.position = position.min(plan.loop_samples.saturating_sub(1));
                        for channel in &mut plan.channels {
                            channel.reset_drum_schedule(self.position, self.sample_rate);
                        }
                    }
                }
                Command::SetLoopRange(range) => self.loop_range = range,
                Command::AuditionStart {
                    pitch,
                    synth,
                    effects,
                    arpeggiator,
                    bpm,
                } => {
                    if arpeggiator.enabled {
                        self.hold_arpeggiator_note(
                            pitch,
                            HeldArpeggiatorInstrument::Synth(synth),
                            effects,
                            arpeggiator,
                            bpm,
                        );
                    } else {
                        self.start_audition(pitch, synth, effects);
                    }
                }
                Command::AuditionFmStart {
                    pitch,
                    synth,
                    effects,
                    arpeggiator,
                    bpm,
                } => {
                    if arpeggiator.enabled {
                        self.hold_arpeggiator_note(
                            pitch,
                            HeldArpeggiatorInstrument::Fm(synth),
                            effects,
                            arpeggiator,
                            bpm,
                        );
                        continue;
                    }
                    self.start_audition_effects(effects);
                    self.audition_fm.retain(|voice| voice.pitch != pitch);
                    self.audition_fm.push(AuditionFmVoice {
                        pitch,
                        synth,
                        elapsed: 0,
                        released_at: None,
                        feedback: 0.0,
                        operator_frequencies: fm_operator_frequencies(
                            &synth,
                            pitch_frequency(pitch, 0),
                        ),
                    });
                }
                Command::AuditionDrumStart {
                    pitch,
                    synth,
                    effects,
                } => {
                    self.start_audition_effects(effects);
                    self.audition_drums.push(AuditionDrumVoice {
                        pitch,
                        synth,
                        elapsed: 0,
                    });
                }
                Command::AuditionStop { pitch } => {
                    self.held_arpeggiator_notes
                        .retain(|note| note.pitch != pitch);
                    for voice in self
                        .audition_voices
                        .iter_mut()
                        .filter(|voice| voice.pitch == pitch && voice.released_at.is_none())
                    {
                        let level = held_envelope(&voice.synth, voice.elapsed, self.sample_rate);
                        voice.released_at = Some((voice.elapsed, level));
                    }
                    for voice in self
                        .audition_fm
                        .iter_mut()
                        .filter(|voice| voice.pitch == pitch && voice.released_at.is_none())
                    {
                        voice.released_at = Some(voice.elapsed);
                    }
                    for voice in self
                        .audition_samples
                        .iter_mut()
                        .filter(|voice| voice.pitch == pitch && voice.released_at.is_none())
                    {
                        let level =
                            held_sample_envelope(&voice.sampler, voice.elapsed, self.sample_rate);
                        voice.released_at = Some((voice.elapsed, level));
                    }
                }
                Command::AuditionSampleStart {
                    pitch,
                    root_pitch,
                    sampler,
                    sample,
                    effects,
                    arpeggiator,
                    bpm,
                } => {
                    if arpeggiator.enabled {
                        self.hold_arpeggiator_note(
                            pitch,
                            HeldArpeggiatorInstrument::Sampler {
                                sampler,
                                sample,
                                root_pitch,
                            },
                            effects,
                            arpeggiator,
                            bpm,
                        );
                        continue;
                    }
                    self.start_audition_effects(effects);
                    let playback_rate = 2.0_f32
                        .powf((f32::from(pitch) - f32::from(root_pitch)) / 12.0)
                        * sampler.speed;
                    self.audition_samples.retain(|voice| voice.pitch != pitch);
                    self.audition_samples.push(AuditionSampleVoice {
                        pitch,
                        sampler,
                        sample,
                        playback_rate,
                        elapsed: 0,
                        released_at: None,
                        filter: FilterState::default(),
                        finished: false,
                    });
                }
            }
        }
    }

    fn start_audition(&mut self, pitch: u8, synth: SimpleWaveformSynth, effects: [EffectSlot; 5]) {
        self.start_audition_effects(effects);
        let frequency = pitch_frequency(pitch, synth.pitch_shift);
        if synth.mono
            && let Some(voice) = self.audition_voices.first_mut()
        {
            let current_frequency = glide_frequency(
                voice.glide_from_frequency,
                voice.frequency,
                voice.glide_elapsed,
                ms_samples(voice.synth.glide_ms, self.sample_rate),
            );
            voice.pitch = pitch;
            voice.synth = synth;
            voice.glide_from_frequency = current_frequency;
            voice.frequency = frequency;
            voice.glide_elapsed = 0;
            voice.released_at = None;
            return;
        }
        self.audition_voices.retain(|voice| voice.pitch != pitch);
        self.audition_voices.push(AuditionVoice {
            pitch,
            synth,
            frequency,
            glide_from_frequency: frequency,
            glide_elapsed: 0,
            elapsed: 0,
            released_at: None,
            filter: FilterState::default(),
        });
    }

    fn start_audition_effects(&mut self, effects: [EffectSlot; 5]) {
        if self.audition_voices.is_empty()
            && self.audition_fm.is_empty()
            && self.audition_samples.is_empty()
            && self.audition_drums.is_empty()
            && self.held_arpeggiator_notes.is_empty()
        {
            self.audition_effects = Some(EffectChain::new(effects, self.sample_rate));
        }
    }

    fn hold_arpeggiator_note(
        &mut self,
        pitch: u8,
        instrument: HeldArpeggiatorInstrument,
        effects: [EffectSlot; 5],
        arpeggiator: ArpeggiatorSettings,
        bpm: f32,
    ) {
        self.start_audition_effects(effects);
        self.arpeggiator = arpeggiator;
        self.arpeggiator_bpm = bpm;
        self.held_arpeggiator_notes
            .retain(|note| note.pitch != pitch);
        self.held_arpeggiator_notes
            .push(HeldArpeggiatorNote { pitch, instrument });
        if self.held_arpeggiator_notes.len() == 1 {
            self.arpeggiator_step_remaining = 0;
            self.arpeggiator_index = 0;
        }
    }

    fn advance_arpeggiator(&mut self) {
        if self.arpeggiator_gate_remaining > 0 {
            self.arpeggiator_gate_remaining -= 1;
            if self.arpeggiator_gate_remaining == 0 {
                self.release_audition_voices();
            }
        }
        if self.held_arpeggiator_notes.is_empty() {
            return;
        }
        if self.arpeggiator_step_remaining > 0 {
            self.arpeggiator_step_remaining -= 1;
            return;
        }

        let sequence = arpeggiator_sequence(&self.held_arpeggiator_notes, self.arpeggiator);
        if sequence.is_empty() {
            return;
        }
        let note = sequence[self.arpeggiator_index % sequence.len()].clone();
        self.arpeggiator_index = (self.arpeggiator_index
            + usize::from(self.arpeggiator.note_skip.max(1)))
            % sequence.len();
        match note.instrument {
            HeldArpeggiatorInstrument::Synth(synth) => {
                self.start_audition(note.pitch, synth, DEFAULT_EFFECTS);
            }
            HeldArpeggiatorInstrument::Fm(synth) => {
                self.audition_fm.push(AuditionFmVoice {
                    pitch: note.pitch,
                    synth,
                    elapsed: 0,
                    released_at: None,
                    feedback: 0.0,
                    operator_frequencies: fm_operator_frequencies(
                        &synth,
                        pitch_frequency(note.pitch, 0),
                    ),
                });
            }
            HeldArpeggiatorInstrument::Sampler {
                sampler,
                sample,
                root_pitch,
            } => {
                let playback_rate = 2.0_f32
                    .powf((f32::from(note.pitch) - f32::from(root_pitch)) / 12.0)
                    * sampler.speed;
                self.audition_samples.push(AuditionSampleVoice {
                    pitch: note.pitch,
                    sampler,
                    sample,
                    playback_rate,
                    elapsed: 0,
                    released_at: None,
                    filter: FilterState::default(),
                    finished: false,
                });
            }
        }
        let step_samples = (self.sample_rate * 60.0
            / self.arpeggiator_bpm.max(1.0)
            / f32::from(self.arpeggiator.steps_per_beat.max(1)))
        .round() as u64;
        self.arpeggiator_step_remaining = step_samples.max(1);
        self.arpeggiator_gate_remaining =
            (step_samples as f32 * self.arpeggiator.gate.clamp(0.05, 1.0)).round() as u64;
    }

    fn release_audition_voices(&mut self) {
        for voice in self
            .audition_voices
            .iter_mut()
            .filter(|voice| voice.released_at.is_none())
        {
            let level = held_envelope(&voice.synth, voice.elapsed, self.sample_rate);
            voice.released_at = Some((voice.elapsed, level));
        }
        for voice in self
            .audition_fm
            .iter_mut()
            .filter(|voice| voice.released_at.is_none())
        {
            voice.released_at = Some(voice.elapsed);
        }
        for voice in self
            .audition_samples
            .iter_mut()
            .filter(|voice| voice.released_at.is_none())
        {
            let level = held_sample_envelope(&voice.sampler, voice.elapsed, self.sample_rate);
            voice.released_at = Some((voice.elapsed, level));
        }
    }

    fn next_frame(&mut self) -> [f32; 2] {
        let mut output = [0.0, 0.0];
        self.advance_arpeggiator();
        if !self.paused
            && let Some(plan) = &mut self.plan
        {
            for channel in &mut plan.channels {
                let mut channel_output = [0.0, 0.0];
                match &mut channel.instrument {
                    RenderInstrument::Synth(synth) => {
                        for voice in &mut channel.voices {
                            if self.position < voice.start_sample {
                                continue;
                            }
                            let elapsed = self.position - voice.start_sample;
                            let release_samples = ms_samples(synth.release_ms, self.sample_rate);
                            if self.position >= voice.note_off_sample + release_samples {
                                continue;
                            }
                            let note_length = voice.note_off_sample - voice.start_sample;
                            let envelope =
                                note_envelope(synth, elapsed, note_length, self.sample_rate);
                            let frequency = glide_frequency(
                                voice.glide_from_frequency,
                                voice.frequency,
                                elapsed,
                                ms_samples(synth.glide_ms, self.sample_rate),
                            );
                            let raw =
                                oscillator_sample(synth, elapsed, frequency, self.sample_rate)
                                    * envelope
                                    * voice.gain
                                    * synth.master_level;
                            let filtered = voice.filter.process_values(
                                raw,
                                synth.filter,
                                voice
                                    .automated_filter_cutoff
                                    .unwrap_or(synth.filter_cutoff_hz),
                                synth.filter_resonance,
                                self.sample_rate,
                            );
                            add_panned(&mut channel_output, filtered, synth.pan);
                        }
                    }
                    RenderInstrument::Fm(synth) => {
                        let release = synth
                            .operators
                            .iter()
                            .map(|operator| ms_samples(operator.release_ms, self.sample_rate))
                            .max()
                            .expect("FM synth always has four operators");
                        for voice in &mut channel.voices {
                            if self.position < voice.start_sample
                                || self.position >= voice.note_off_sample + release
                            {
                                continue;
                            }
                            let elapsed = self.position - voice.start_sample;
                            let note_length = voice.note_off_sample - voice.start_sample;
                            let raw = fm_sample(
                                synth,
                                elapsed,
                                note_length,
                                voice.fm_operator_frequencies,
                                self.sample_rate,
                                &mut voice.fm_feedback,
                            ) * voice.gain
                                * synth.master_level;
                            add_panned(&mut channel_output, raw, synth.pan);
                        }
                    }
                    RenderInstrument::DrumMachine(synth) => {
                        while channel.next_drum_voice < channel.voices.len()
                            && channel.voices[channel.next_drum_voice].start_sample <= self.position
                        {
                            let voice = &channel.voices[channel.next_drum_voice];
                            channel.active_drum_voices.push(ActiveDrumVoice {
                                pitch: voice.pitch,
                                start_sample: voice.start_sample,
                                gain: voice.gain,
                            });
                            channel.next_drum_voice += 1;
                        }
                        channel.active_drum_voices.retain(|voice| {
                            self.position - voice.start_sample
                                < drum_voice_duration_samples(synth, voice.pitch, self.sample_rate)
                        });
                        for voice in &channel.active_drum_voices {
                            let raw = drum_sample(
                                synth,
                                voice.pitch,
                                self.position - voice.start_sample,
                                self.sample_rate,
                            ) * voice.gain
                                * synth.master_level;
                            add_panned(&mut channel_output, raw, synth.pan);
                        }
                    }
                    RenderInstrument::Sampler { sampler } => {
                        for voice in &mut channel.voices {
                            if self.position < voice.start_sample {
                                continue;
                            }
                            let elapsed = self.position - voice.start_sample;
                            let note_length = voice.note_off_sample - voice.start_sample;
                            let release = ms_samples(sampler.release_ms, self.sample_rate);
                            if elapsed >= note_length + release {
                                continue;
                            }
                            let envelope = if elapsed < note_length {
                                held_sample_envelope(sampler, elapsed, self.sample_rate)
                            } else if release > 0 {
                                held_sample_envelope(sampler, note_length, self.sample_rate)
                                    * (1.0 - (elapsed - note_length) as f32 / release as f32)
                                        .clamp(0.0, 1.0)
                            } else {
                                0.0
                            };
                            let sample = voice
                                .sample
                                .as_ref()
                                .expect("sampler voices carry their selected sample");
                            let Some(frame) = sampler_frame(
                                sample,
                                sampler,
                                elapsed,
                                voice.frequency,
                                self.sample_rate,
                            ) else {
                                continue;
                            };
                            let mono =
                                (frame[0] + frame[1]) * 0.5 * envelope * sampler.gain * voice.gain;
                            let filtered = voice.filter.process_values(
                                mono,
                                sampler.filter,
                                voice
                                    .automated_filter_cutoff
                                    .unwrap_or(sampler.filter_cutoff_hz),
                                sampler.filter_resonance,
                                self.sample_rate,
                            );
                            add_panned(&mut channel_output, filtered, sampler.pan);
                        }
                    }
                }
                let effected = channel.effects.process(channel_output, self.sample_rate);
                output[0] += effected[0];
                output[1] += effected[1];
            }
            self.position += 1;
            let loop_target = self
                .loop_range
                .filter(|(start, end)| start < end && *end <= plan.loop_samples);
            if loop_target.is_some_and(|(_, end)| self.position >= end)
                || self.position >= plan.loop_samples
            {
                self.position = loop_target.map_or(0, |(start, _)| start);
                for channel in &mut plan.channels {
                    for voice in &mut channel.voices {
                        voice.filter = FilterState::default();
                    }
                    channel.reset_drum_schedule(self.position, self.sample_rate);
                }
            }
        }

        let mut audition_output = [0.0, 0.0];
        for voice in &mut self.audition_voices {
            let envelope = match voice.released_at {
                Some((released_at, release_level)) => {
                    let release = ms_samples(voice.synth.release_ms, self.sample_rate);
                    if release == 0 {
                        0.0
                    } else {
                        release_level
                            * (1.0 - (voice.elapsed - released_at) as f32 / release as f32)
                                .clamp(0.0, 1.0)
                    }
                }
                None => held_envelope(&voice.synth, voice.elapsed, self.sample_rate),
            };
            let frequency = glide_frequency(
                voice.glide_from_frequency,
                voice.frequency,
                voice.glide_elapsed,
                ms_samples(voice.synth.glide_ms, self.sample_rate),
            );
            let raw = oscillator_sample(&voice.synth, voice.elapsed, frequency, self.sample_rate)
                * envelope
                * voice.synth.master_level;
            let filtered = voice.filter.process(raw, &voice.synth, self.sample_rate);
            add_panned(&mut audition_output, filtered, voice.synth.pan);
            voice.elapsed += 1;
            voice.glide_elapsed += 1;
        }
        self.audition_voices.retain(|voice| {
            voice.released_at.is_none_or(|(released_at, _)| {
                voice.elapsed < released_at + ms_samples(voice.synth.release_ms, self.sample_rate)
            })
        });
        for voice in &mut self.audition_samples {
            let envelope = match voice.released_at {
                Some((released_at, release_level)) => {
                    let release = ms_samples(voice.sampler.release_ms, self.sample_rate);
                    if release == 0 {
                        0.0
                    } else {
                        release_level
                            * (1.0 - (voice.elapsed - released_at) as f32 / release as f32)
                                .clamp(0.0, 1.0)
                    }
                }
                None => held_sample_envelope(&voice.sampler, voice.elapsed, self.sample_rate),
            };
            let Some(frame) = sampler_frame(
                &voice.sample,
                &voice.sampler,
                voice.elapsed,
                voice.playback_rate,
                self.sample_rate,
            ) else {
                voice.finished = true;
                continue;
            };
            let mono = (frame[0] + frame[1]) * 0.5 * envelope * voice.sampler.gain;
            let filtered = voice.filter.process_values(
                mono,
                voice.sampler.filter,
                voice.sampler.filter_cutoff_hz,
                voice.sampler.filter_resonance,
                self.sample_rate,
            );
            add_panned(&mut audition_output, filtered, voice.sampler.pan);
            voice.elapsed += 1;
        }
        self.audition_samples.retain(|voice| {
            !voice.finished
                && voice.released_at.is_none_or(|(released_at, _)| {
                    voice.elapsed
                        < released_at + ms_samples(voice.sampler.release_ms, self.sample_rate)
                })
        });
        for voice in &mut self.audition_fm {
            let note_length = voice.released_at.unwrap_or(u64::MAX);
            let raw = fm_sample(
                &voice.synth,
                voice.elapsed,
                note_length,
                voice.operator_frequencies,
                self.sample_rate,
                &mut voice.feedback,
            ) * voice.synth.master_level;
            add_panned(&mut audition_output, raw, voice.synth.pan);
            voice.elapsed += 1;
        }
        self.audition_fm.retain(|voice| {
            voice.released_at.is_none_or(|released_at| {
                let release = voice
                    .synth
                    .operators
                    .iter()
                    .map(|operator| ms_samples(operator.release_ms, self.sample_rate))
                    .max()
                    .expect("FM synth always has four operators");
                voice.elapsed < released_at + release
            })
        });
        for voice in &mut self.audition_drums {
            let raw = drum_sample(&voice.synth, voice.pitch, voice.elapsed, self.sample_rate)
                * voice.synth.master_level;
            add_panned(&mut audition_output, raw, voice.synth.pan);
            voice.elapsed += 1;
        }
        self.audition_drums
            .retain(|voice| voice.elapsed < ms_samples(2_500.0, self.sample_rate));
        if let Some(effects) = &mut self.audition_effects {
            let effected = effects.process(audition_output, self.sample_rate);
            output[0] += effected[0];
            output[1] += effected[1];
        }

        [output[0].tanh(), output[1].tanh()]
    }
}

impl FilterState {
    fn process(&mut self, input: f32, synth: &SimpleWaveformSynth, sample_rate: f32) -> f32 {
        self.process_values(
            input,
            synth.filter,
            synth.filter_cutoff_hz,
            synth.filter_resonance,
            sample_rate,
        )
    }

    fn process_values(
        &mut self,
        input: f32,
        kind: FilterKind,
        cutoff_hz: f32,
        resonance: f32,
        sample_rate: f32,
    ) -> f32 {
        if kind == FilterKind::Off {
            return input;
        }
        let cutoff = cutoff_hz.clamp(20.0, sample_rate * 0.45);
        let frequency = ((std::f32::consts::PI * cutoff / sample_rate).sin() * 2.0).min(0.99);
        let damping = (2.0 * (1.0 - resonance.powf(0.25))).clamp(0.05, 2.0);
        self.low += frequency * self.band;
        let high = input - self.low - damping * self.band;
        self.band += frequency * high;
        match kind {
            FilterKind::Off => input,
            FilterKind::LowPass => self.low,
            FilterKind::HighPass => high,
            FilterKind::BandPass => self.band,
        }
    }
}

impl EffectChain {
    fn new(slots: [EffectSlot; 5], sample_rate: f32) -> Self {
        let states = slots
            .iter()
            .map(|slot| match slot.kind {
                EffectKind::Distortion { .. } => EffectState::Distortion,
                EffectKind::Delay { time_ms, .. } => EffectState::Delay {
                    buffer: vec![[0.0, 0.0]; ms_samples(time_ms, sample_rate).max(1) as usize],
                    position: 0,
                },
                EffectKind::Chorus { .. } => EffectState::Chorus {
                    buffer: vec![[0.0, 0.0]; (sample_rate * 0.06).round().max(1.0) as usize],
                    position: 0,
                    phase: 0.0,
                },
                EffectKind::Tremolo { .. } => EffectState::Tremolo { phase: 0.0 },
                EffectKind::Reverb { room_size, .. } => EffectState::Reverb {
                    left: vec![0.0; (sample_rate * (0.035 + room_size * 0.065)).round() as usize],
                    right: vec![0.0; (sample_rate * (0.043 + room_size * 0.079)).round() as usize],
                    left_position: 0,
                    right_position: 0,
                    damped: [0.0, 0.0],
                },
            })
            .collect();
        Self { slots, states }
    }

    fn process(&mut self, mut frame: [f32; 2], sample_rate: f32) -> [f32; 2] {
        for (slot, state) in self.slots.iter().zip(&mut self.states) {
            if !slot.enabled {
                continue;
            }
            match (slot.kind, state) {
                (EffectKind::Distortion { drive, mix }, EffectState::Distortion) => {
                    let normalization = drive.tanh().max(0.001);
                    for sample in &mut frame {
                        let wet = (*sample * drive).tanh() / normalization;
                        *sample += (wet - *sample) * mix;
                    }
                }
                (
                    EffectKind::Delay { feedback, mix, .. },
                    EffectState::Delay { buffer, position },
                ) => {
                    let delayed = buffer[*position];
                    buffer[*position] = [
                        frame[0] + delayed[0] * feedback,
                        frame[1] + delayed[1] * feedback,
                    ];
                    *position = (*position + 1) % buffer.len();
                    frame[0] += (delayed[0] - frame[0]) * mix;
                    frame[1] += (delayed[1] - frame[1]) * mix;
                }
                (
                    EffectKind::Chorus {
                        rate_hz,
                        depth_ms,
                        mix,
                    },
                    EffectState::Chorus {
                        buffer,
                        position,
                        phase,
                    },
                ) => {
                    let base_delay = sample_rate * 0.018;
                    let modulation = sample_rate * depth_ms / 1_000.0;
                    let left_delay = (base_delay + modulation * phase.sin()).max(1.0) as usize;
                    let right_delay =
                        (base_delay + modulation * (*phase + 1.7).sin()).max(1.0) as usize;
                    let left_index = (*position + buffer.len() - left_delay.min(buffer.len() - 1))
                        % buffer.len();
                    let right_index = (*position + buffer.len()
                        - right_delay.min(buffer.len() - 1))
                        % buffer.len();
                    let wet = [buffer[left_index][0], buffer[right_index][1]];
                    buffer[*position] = frame;
                    *position = (*position + 1) % buffer.len();
                    *phase = (*phase + std::f32::consts::TAU * rate_hz / sample_rate)
                        % std::f32::consts::TAU;
                    frame[0] += (wet[0] - frame[0]) * mix;
                    frame[1] += (wet[1] - frame[1]) * mix;
                }
                (EffectKind::Tremolo { rate_hz, depth }, EffectState::Tremolo { phase }) => {
                    let gain = 1.0 - depth * 0.5 + phase.sin() * depth * 0.5;
                    frame[0] *= gain;
                    frame[1] *= gain;
                    *phase = (*phase + std::f32::consts::TAU * rate_hz / sample_rate)
                        % std::f32::consts::TAU;
                }
                (
                    EffectKind::Reverb {
                        room_size,
                        damping,
                        mix,
                    },
                    EffectState::Reverb {
                        left,
                        right,
                        left_position,
                        right_position,
                        damped,
                    },
                ) => {
                    let wet = [left[*left_position], right[*right_position]];
                    damped[0] += (wet[0] - damped[0]) * (1.0 - damping);
                    damped[1] += (wet[1] - damped[1]) * (1.0 - damping);
                    let feedback = 0.55 + room_size * 0.4;
                    left[*left_position] = frame[0] + damped[1] * feedback;
                    right[*right_position] = frame[1] + damped[0] * feedback;
                    *left_position = (*left_position + 1) % left.len();
                    *right_position = (*right_position + 1) % right.len();
                    frame[0] += (wet[0] - frame[0]) * mix;
                    frame[1] += (wet[1] - frame[1]) * mix;
                }
                _ => unreachable!("effect state must match its stack slot"),
            }
        }
        frame
    }
}

fn held_envelope(synth: &SimpleWaveformSynth, elapsed: u64, sample_rate: f32) -> f32 {
    let attack = ms_samples(synth.attack_ms, sample_rate);
    let decay = ms_samples(synth.decay_ms, sample_rate);
    if elapsed < attack && attack > 0 {
        elapsed as f32 / attack as f32
    } else if elapsed < attack + decay && decay > 0 {
        let progress = (elapsed - attack) as f32 / decay as f32;
        1.0 + (synth.sustain - 1.0) * progress
    } else {
        synth.sustain
    }
}

fn held_sample_envelope(sampler: &SampleSynth, elapsed: u64, sample_rate: f32) -> f32 {
    let attack = ms_samples(sampler.attack_ms, sample_rate);
    let decay = ms_samples(sampler.decay_ms, sample_rate);
    if elapsed < attack && attack > 0 {
        elapsed as f32 / attack as f32
    } else if elapsed < attack + decay && decay > 0 {
        let progress = (elapsed - attack) as f32 / decay as f32;
        1.0 + (sampler.sustain - 1.0) * progress
    } else {
        sampler.sustain
    }
}

fn note_envelope(
    synth: &SimpleWaveformSynth,
    elapsed: u64,
    note_length: u64,
    sample_rate: f32,
) -> f32 {
    if elapsed < note_length {
        held_envelope(synth, elapsed, sample_rate)
    } else {
        let release = ms_samples(synth.release_ms, sample_rate);
        if release == 0 {
            0.0
        } else {
            held_envelope(synth, note_length, sample_rate)
                * (1.0 - (elapsed - note_length) as f32 / release as f32).clamp(0.0, 1.0)
        }
    }
}

fn oscillator_sample(
    synth: &SimpleWaveformSynth,
    elapsed: u64,
    frequency: f32,
    sample_rate: f32,
) -> f32 {
    let noise = noise_sample(elapsed as u32 ^ frequency.to_bits());
    synth
        .layers
        .iter()
        .take(usize::from(synth.layer_count))
        .map(|layer| {
            let detuned_frequency = frequency * 2.0_f32.powf(layer.detune_cents / 1_200.0);
            let phase = elapsed as f32 * detuned_frequency / sample_rate;
            layer.waveform.sample(phase, noise) * layer.level
        })
        .sum::<f32>()
        / f32::from(synth.layer_count)
}

fn drum_sample(synth: &DrumMachineSynth, pitch: u8, elapsed: u64, sample_rate: f32) -> f32 {
    let Some(kind) = DrumVoiceKind::from_midi_pitch(pitch) else {
        return 0.0;
    };
    let voice = synth.voices[kind.index()];
    let time = elapsed as f32 / sample_rate;
    let tone_decay = (-time * 1_000.0 / voice.tone_decay_ms.max(1.0)).exp();
    let noise_decay = (-time * 1_000.0 / voice.noise_decay_ms.max(1.0)).exp();
    if tone_decay < 0.000_1 && noise_decay < 0.000_1 {
        return 0.0;
    }
    let pitch_envelope = (-time * 35.0).exp();
    let frequency = voice.tone_hz + voice.pitch_drop_hz * pitch_envelope;
    let phase = std::f32::consts::TAU * time * frequency;
    let tone = match kind {
        DrumVoiceKind::ClosedHat
        | DrumVoiceKind::OpenHat
        | DrumVoiceKind::Crash
        | DrumVoiceKind::Ride => {
            (phase.sin() + (phase * 1.447).sin() + (phase * 1.731).sin()) / 3.0
        }
        _ => phase.sin(),
    };
    let noise = noise_sample(elapsed as u32 ^ (u32::from(pitch) << 24));
    (tone * voice.tone_level * tone_decay + noise * voice.noise_level * noise_decay).tanh()
}

fn drum_voice_duration_samples(synth: &DrumMachineSynth, pitch: u8, sample_rate: f32) -> u64 {
    let Some(kind) = DrumVoiceKind::from_midi_pitch(pitch) else {
        return 0;
    };
    let voice = synth.voices[kind.index()];
    ms_samples(
        voice.tone_decay_ms.max(voice.noise_decay_ms) * 7.0,
        sample_rate,
    )
}

fn fm_operator_frequencies(synth: &FmSynth, frequency: f32) -> [f32; 4] {
    synth
        .operators
        .map(|operator| frequency * operator.ratio * 2.0_f32.powf(operator.detune_cents / 1_200.0))
}

fn fm_sample(
    synth: &FmSynth,
    elapsed: u64,
    note_length: u64,
    operator_frequencies: [f32; 4],
    sample_rate: f32,
    feedback_state: &mut f32,
) -> f32 {
    let mut phases = [0.0; 4];
    let mut levels = [0.0; 4];
    for (index, (operator, operator_frequency)) in
        synth.operators.iter().zip(operator_frequencies).enumerate()
    {
        phases[index] =
            std::f32::consts::TAU * (elapsed as f32 * operator_frequency / sample_rate).fract();
        levels[index] =
            operator.level * fm_operator_envelope(operator, elapsed, note_length, sample_rate);
    }
    let feedback_operator = (phases[3] + *feedback_state * synth.feedback * 6.0).sin() * levels[3];
    *feedback_state = feedback_operator;
    match synth.algorithm {
        FmAlgorithm::Stack => {
            let operator_3 = (phases[2] + feedback_operator * 6.0).sin() * levels[2];
            let operator_2 = (phases[1] + operator_3 * 6.0).sin() * levels[1];
            (phases[0] + operator_2 * 6.0).sin() * levels[0]
        }
        FmAlgorithm::TwoPairs => {
            let first = (phases[0] + phases[1].sin() * levels[1] * 6.0).sin() * levels[0];
            let second = (phases[2] + feedback_operator * 6.0).sin() * levels[2];
            (first + second) * 0.5
        }
        FmAlgorithm::ThreeModulators => {
            let modulation =
                phases[1].sin() * levels[1] + phases[2].sin() * levels[2] + feedback_operator;
            (phases[0] + modulation * 4.0).sin() * levels[0]
        }
        FmAlgorithm::Additive => {
            (phases[0].sin() * levels[0]
                + phases[1].sin() * levels[1]
                + phases[2].sin() * levels[2]
                + feedback_operator)
                * 0.25
        }
    }
}

fn fm_operator_envelope(
    operator: &FmOperator,
    elapsed: u64,
    note_length: u64,
    sample_rate: f32,
) -> f32 {
    let attack = ms_samples(operator.attack_ms, sample_rate);
    let decay = ms_samples(operator.decay_ms, sample_rate);
    let held = |time: u64| {
        if time < attack && attack > 0 {
            time as f32 / attack as f32
        } else if time < attack + decay && decay > 0 {
            let progress = (time - attack) as f32 / decay as f32;
            1.0 + (operator.sustain - 1.0) * progress
        } else {
            operator.sustain
        }
    };
    if elapsed < note_length {
        held(elapsed)
    } else {
        let release = ms_samples(operator.release_ms, sample_rate);
        if release == 0 {
            0.0
        } else {
            held(note_length)
                * (1.0 - (elapsed - note_length) as f32 / release as f32).clamp(0.0, 1.0)
        }
    }
}

fn pattern_articulation<'a>(pattern: &'a Pattern, step: u16, default: &'a str) -> &'a str {
    pattern
        .automation
        .iter()
        .find(|lane| lane.parameter == AutomationParameter::SamplerArticulation)
        .and_then(|lane| lane.value_at(step))
        .and_then(|value| match value {
            AutomationValue::Choice(articulation) => Some(articulation.as_str()),
            AutomationValue::Continuous(_) => None,
        })
        .unwrap_or(default)
}

fn pattern_filter_cutoff(
    pattern: &Pattern,
    step: u16,
    instrument: &RenderInstrument,
) -> Option<f32> {
    let parameter = match instrument {
        RenderInstrument::Synth(_) => AutomationParameter::SynthFilterCutoff,
        RenderInstrument::Sampler { .. } => AutomationParameter::SamplerFilterCutoff,
        RenderInstrument::Fm(_) | RenderInstrument::DrumMachine(_) => return None,
    };
    pattern
        .automation
        .iter()
        .find(|lane| lane.parameter == parameter)
        .and_then(|lane| lane.continuous_value_at(step))
}

fn select_sample_region<'a>(
    sampler: &'a SampleSynth,
    pitch: u8,
    velocity: u8,
    articulation: &str,
) -> Option<(&'a Path, u8)> {
    let matching_velocity = sampler
        .regions
        .iter()
        .filter(|region| region.articulation == articulation)
        .filter(|region| (region.key_min..=region.key_max).contains(&pitch))
        .filter(|region| (region.velocity_min..=region.velocity_max).contains(&velocity));
    if let Some(region) = matching_velocity.min_by_key(|region| region.root_pitch.abs_diff(pitch)) {
        return Some((&region.path, region.root_pitch));
    }
    sampler
        .regions
        .iter()
        .filter(|region| region.articulation == articulation)
        .min_by_key(|region| region.root_pitch.abs_diff(pitch))
        .map(|region| (region.path.as_path(), region.root_pitch))
        .or_else(|| {
            sampler
                .path
                .as_deref()
                .map(|path| (path, sampler.root_pitch))
        })
}

fn sampler_frame(
    sample: &SampleBuffer,
    sampler: &SampleSynth,
    elapsed: u64,
    playback_rate: f32,
    output_sample_rate: f32,
) -> Option<[f32; 2]> {
    if sample.frames.is_empty() {
        return None;
    }
    let frame_count = sample.frames.len();
    let start = (sampler.trim_start.clamp(0.0, 1.0) * frame_count as f32).floor() as usize;
    let end = (sampler.trim_end.clamp(0.0, 1.0) * frame_count as f32)
        .ceil()
        .clamp(1.0, frame_count as f32) as usize;
    if start >= end {
        return None;
    }
    let position = elapsed as f32 * playback_rate * sample.sample_rate / output_sample_rate;
    let length = (end - start) as f32;
    let position = if sampler.looping {
        match sampler.loop_mode {
            SampleLoopMode::Forward => position % length,
            SampleLoopMode::PingPong => {
                let span = (length - 1.0).max(0.0);
                if span == 0.0 {
                    0.0
                } else {
                    let phase = position % (span * 2.0);
                    if phase <= span {
                        phase
                    } else {
                        span * 2.0 - phase
                    }
                }
            }
        }
    } else if position >= length {
        return None;
    } else {
        position
    };
    let source_position = if sampler.reverse {
        end as f32 - 1.0 - position
    } else {
        start as f32 + position
    };
    let first = source_position.floor() as usize;
    let second = (first + 1).min(end - 1);
    let fraction = source_position.fract();
    Some([
        sample.frames[first][0] + (sample.frames[second][0] - sample.frames[first][0]) * fraction,
        sample.frames[first][1] + (sample.frames[second][1] - sample.frames[first][1]) * fraction,
    ])
}

pub(crate) fn load_waveform_preview(path: &Path, columns: usize) -> Result<Vec<[f32; 2]>, String> {
    let sample = load_wav(path)?;
    if sample.frames.is_empty() || columns == 0 {
        return Ok(Vec::new());
    }
    let preview_columns = columns.min(sample.frames.len());
    let mut preview = Vec::with_capacity(preview_columns);
    for column in 0..preview_columns {
        let start = column * sample.frames.len() / preview_columns;
        let end = ((column + 1) * sample.frames.len() / preview_columns).max(start + 1);
        let mut minimum = 1.0_f32;
        let mut maximum = -1.0_f32;
        for frame in &sample.frames[start..end.min(sample.frames.len())] {
            let mono = (frame[0] + frame[1]) * 0.5;
            minimum = minimum.min(mono);
            maximum = maximum.max(mono);
        }
        preview.push([minimum, maximum]);
    }
    Ok(preview)
}

fn load_wav(path: &Path) -> Result<SampleBuffer, String> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|error| format!("Could not open sample {}: {error}", path.display()))?;
    let specification = reader.spec();
    let channels = usize::from(specification.channels);
    if channels == 0 {
        return Err("The WAV file has no audio channels".to_owned());
    }
    let samples = match specification.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|sample| sample.map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int => {
            let maximum = ((1_u64 << (specification.bits_per_sample - 1)) - 1) as f32;
            reader
                .samples::<i32>()
                .map(|sample| {
                    sample
                        .map(|value| value as f32 / maximum)
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?
        }
    };
    let frames = samples
        .chunks(channels)
        .map(|frame| {
            let left = frame[0];
            let right = frame.get(1).copied().unwrap_or(left);
            [left, right]
        })
        .collect();
    Ok(SampleBuffer {
        frames,
        sample_rate: specification.sample_rate as f32,
    })
}

fn pitch_frequency(pitch: u8, shift: i8) -> f32 {
    440.0 * 2.0_f32.powf((f32::from(pitch) + f32::from(shift) - 69.0) / 12.0)
}

fn glide_frequency(from: f32, to: f32, elapsed: u64, glide_samples: u64) -> f32 {
    if glide_samples == 0 || elapsed >= glide_samples {
        to
    } else {
        from * (to / from).powf(elapsed as f32 / glide_samples as f32)
    }
}

fn arpeggiator_sequence(
    held: &[HeldArpeggiatorNote],
    settings: ArpeggiatorSettings,
) -> Vec<HeldArpeggiatorNote> {
    let mut chord = held.to_vec();
    chord.sort_by_key(|note| note.pitch);
    let mut ascending = Vec::with_capacity(chord.len() * usize::from(settings.octaves.max(1)));
    for octave in 0..settings.octaves.max(1) {
        for note in &chord {
            let Some(pitch) = note.pitch.checked_add(octave.saturating_mul(12)) else {
                continue;
            };
            if pitch <= 127 {
                let mut note = note.clone();
                note.pitch = pitch;
                ascending.push(note);
            }
        }
    }
    match settings.order {
        ArpeggiatorOrder::Up => ascending,
        ArpeggiatorOrder::Down => {
            ascending.reverse();
            ascending
        }
        ArpeggiatorOrder::UpDown => {
            if ascending.len() > 2 {
                let descending = ascending[1..ascending.len() - 1]
                    .iter()
                    .rev()
                    .cloned()
                    .collect::<Vec<_>>();
                ascending.extend(descending);
            }
            ascending
        }
    }
}

fn ms_samples(milliseconds: f32, sample_rate: f32) -> u64 {
    (milliseconds * sample_rate / 1_000.0).round() as u64
}

fn add_panned(output: &mut [f32; 2], sample: f32, pan: f32) {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    output[0] += sample * angle.cos();
    output[1] += sample * angle.sin();
}

#[cfg(test)]
mod tests {
    use super::{
        EffectChain, HeldArpeggiatorInstrument, HeldArpeggiatorNote, PlaybackPlan,
        RenderInstrument, Renderer, SampleBuffer, add_panned, arpeggiator_sequence, drum_sample,
        export_wav, fm_operator_frequencies, fm_sample, glide_frequency, held_envelope,
        held_sample_envelope, note_envelope, pattern_articulation, pitch_frequency, sampler_frame,
        select_sample_region,
    };
    use crate::model::{
        ArpeggiatorOrder, ArpeggiatorSettings, AutomationLane, AutomationParameter,
        AutomationPoint, AutomationValue, DEFAULT_EFFECTS, EffectKind, Pattern, Project, TrackKind,
    };
    use crate::synths::{
        DrumMachineSynth, FmAlgorithm, FmSynth, SampleLoopMode, SampleRegion, SampleSynth,
        SimpleWaveformSynth,
    };
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    fn multisampler_selects_velocity_layer_then_nearest_root() {
        let sampler = SampleSynth {
            regions: vec![
                SampleRegion {
                    path: PathBuf::from("soft-c4.wav"),
                    root_pitch: 60,
                    key_min: 12,
                    key_max: 132,
                    velocity_min: 1,
                    velocity_max: 84,
                    articulation: "Standard".to_owned(),
                },
                SampleRegion {
                    path: PathBuf::from("loud-c4.wav"),
                    root_pitch: 60,
                    key_min: 12,
                    key_max: 63,
                    velocity_min: 85,
                    velocity_max: 127,
                    articulation: "Standard".to_owned(),
                },
                SampleRegion {
                    path: PathBuf::from("loud-g4.wav"),
                    root_pitch: 67,
                    key_min: 64,
                    key_max: 132,
                    velocity_min: 85,
                    velocity_max: 127,
                    articulation: "Standard".to_owned(),
                },
            ],
            ..SampleSynth::default()
        };

        let (path, root) = select_sample_region(&sampler, 65, 100, "Standard")
            .expect("a matching multisample region should exist");
        assert_eq!(path, std::path::Path::new("loud-g4.wav"));
        assert_eq!(root, 67);
    }

    #[test]
    fn fm_algorithms_produce_distinct_finite_samples() {
        let mut synth = FmSynth::default();
        let mut feedback = 0.0;
        let frequencies = fm_operator_frequencies(&synth, 220.0);
        let stack = fm_sample(&synth, 137, 1_000, frequencies, 44_100.0, &mut feedback);
        synth.algorithm = FmAlgorithm::Additive;
        feedback = 0.0;
        let additive = fm_sample(&synth, 137, 1_000, frequencies, 44_100.0, &mut feedback);

        assert!(stack.is_finite());
        assert!(additive.is_finite());
        assert!((stack - additive).abs() > f32::EPSILON);
    }

    #[test]
    fn drum_kits_produce_finite_distinct_kicks_that_decay() {
        let rock = DrumMachineSynth::PRESETS[0].synth;
        let house = DrumMachineSynth::PRESETS
            .iter()
            .find(|preset| preset.name == "House")
            .expect("the house kit should exist")
            .synth;

        let rock_attack = drum_sample(&rock, 36, 8, 48_000.0);
        let house_attack = drum_sample(&house, 36, 8, 48_000.0);
        let rock_tail = drum_sample(&rock, 36, 96_000, 48_000.0);

        assert!(rock_attack.is_finite());
        assert!(house_attack.is_finite());
        assert!((rock_attack - house_attack).abs() > 0.001);
        assert!(rock_tail.abs() < 0.001);
        assert_eq!(drum_sample(&rock, 37, 8, 48_000.0), 0.0);
    }

    #[test]
    fn articulation_automation_latches_until_the_next_event() {
        let mut pattern = Pattern::default();
        pattern.automation.push(AutomationLane {
            parameter: AutomationParameter::SamplerArticulation,
            points: vec![
                AutomationPoint {
                    step: 4,
                    value: AutomationValue::Choice("pizz".to_owned()),
                },
                AutomationPoint {
                    step: 12,
                    value: AutomationValue::Choice("arco".to_owned()),
                },
            ],
        });

        assert_eq!(pattern_articulation(&pattern, 0, "arco"), "arco");
        assert_eq!(pattern_articulation(&pattern, 8, "arco"), "pizz");
        assert_eq!(pattern_articulation(&pattern, 16, "arco"), "arco");
    }

    #[test]
    fn sampler_adsr_decays_to_its_sustain_level() {
        let sampler = SampleSynth {
            attack_ms: 100.0,
            decay_ms: 100.0,
            sustain: 0.4,
            ..SampleSynth::default()
        };

        assert!((held_sample_envelope(&sampler, 50, 1_000.0) - 0.5).abs() < f32::EPSILON);
        assert!((held_sample_envelope(&sampler, 150, 1_000.0) - 0.7).abs() < f32::EPSILON);
        assert!((held_sample_envelope(&sampler, 250, 1_000.0) - 0.4).abs() < f32::EPSILON);
    }

    #[test]
    fn playback_plan_places_and_trims_notes_with_the_clip() {
        let mut project = Project::default();
        project.add_instrument();
        project.bpm = 60.0;
        let pattern_id = project.tracks[0].source_id;
        project
            .add_note(pattern_id, 69, 2, 4, 127)
            .expect("primary pattern should exist");
        project.ensure_primary_pattern_clip(project.tracks[0].id);
        project.tracks[0].clips[0].start_step = 4;
        project.tracks[0].clips[0].length_steps = 3;

        let plan = PlaybackPlan::from_project(&project, 100.0)
            .expect("synth project should build a playback plan");

        assert_eq!(plan.channels[0].voices.len(), 1);
        assert_eq!(plan.channels[0].voices[0].start_sample, 75);
        assert_eq!(plan.channels[0].voices[0].note_off_sample, 88);
        assert!((plan.channels[0].voices[0].frequency - 440.0).abs() < f32::EPSILON);
    }

    #[test]
    fn muted_tracks_are_not_scheduled() {
        let mut project = Project::default();
        project.add_instrument();
        let pattern_id = project.tracks[0].source_id;
        project
            .add_note(pattern_id, 60, 0, 1, 100)
            .expect("primary pattern should exist");
        project.ensure_primary_pattern_clip(project.tracks[0].id);
        project.tracks[0].muted = true;

        assert!(
            PlaybackPlan::from_project(&project, 48_000.0)
                .expect("synth project should build a playback plan")
                .channels
                .is_empty()
        );
    }

    #[test]
    fn playback_uses_the_instrument_tracks_effect_chain() {
        let mut project = Project::default();
        project.add_fm();
        let pattern_id = project.tracks[0].source_id;
        project
            .add_note(pattern_id, 60, 0, 1, 100)
            .expect("primary pattern should exist");
        project.ensure_primary_pattern_clip(project.tracks[0].id);
        project.tracks[0].effects[4].enabled = true;

        let plan = PlaybackPlan::from_project(&project, 48_000.0)
            .expect("FM project should build a playback plan");

        assert!(plan.channels[0].effects.slots[4].enabled);
        assert!(matches!(
            plan.channels[0].effects.slots[4].kind,
            EffectKind::Reverb { .. }
        ));
    }

    #[test]
    fn drum_machine_pattern_schedules_mapped_drum_voices() {
        let mut project = Project::default();
        project.add_drum_machine();
        let pattern_id = project.tracks[0].source_id;
        project
            .add_note(pattern_id, 36, 0, 1, 127)
            .expect("drum pattern should exist");
        project.ensure_primary_pattern_clip(project.tracks[0].id);

        let plan = PlaybackPlan::from_project(&project, 48_000.0)
            .expect("drum project should build a playback plan");

        assert!(matches!(
            plan.channels[0].instrument,
            RenderInstrument::DrumMachine(_)
        ));
        assert_eq!(plan.channels[0].voices[0].pitch, 36);
    }

    #[test]
    fn drum_renderer_removes_hits_after_their_synthesized_tail() {
        let mut project = Project::default();
        project.add_drum_machine();
        let pattern_id = project.tracks[0].source_id;
        project
            .add_note(pattern_id, 36, 0, 1, 127)
            .expect("drum pattern should exist");
        project.ensure_primary_pattern_clip(project.tracks[0].id);
        let plan = PlaybackPlan::from_project(&project, 1_000.0)
            .expect("drum project should build a playback plan");
        let (_sender, receiver) = std::sync::mpsc::channel();
        let mut renderer = Renderer::new(receiver, 1_000.0);
        renderer.plan = Some(plan);

        for _ in 0..3_000 {
            renderer.next_frame();
        }

        let channel = &renderer
            .plan
            .as_ref()
            .expect("renderer should retain its plan")
            .channels[0];
        assert_eq!(channel.next_drum_voice, 1);
        assert!(channel.active_drum_voices.is_empty());
    }

    #[test]
    fn arpeggiator_only_sequences_held_pitch_classes_and_their_octaves() {
        let held = [60, 64, 67].map(|pitch| HeldArpeggiatorNote {
            pitch,
            instrument: HeldArpeggiatorInstrument::Synth(SimpleWaveformSynth::default()),
        });
        let settings = ArpeggiatorSettings {
            enabled: true,
            order: ArpeggiatorOrder::UpDown,
            octaves: 2,
            ..ArpeggiatorSettings::default()
        };

        let pitches = arpeggiator_sequence(&held, settings)
            .iter()
            .map(|note| note.pitch)
            .collect::<Vec<_>>();

        assert_eq!(pitches, [60, 64, 67, 72, 76, 79, 76, 72, 67, 64]);
        assert!(
            pitches
                .iter()
                .all(|pitch| [0, 4, 7].contains(&(pitch % 12)))
        );
    }

    #[test]
    fn adsr_reaches_sustain_after_attack_and_decay() {
        let synth = SimpleWaveformSynth {
            attack_ms: 100.0,
            decay_ms: 100.0,
            sustain: 0.4,
            release_ms: 100.0,
            ..SimpleWaveformSynth::default()
        };

        assert_eq!(held_envelope(&synth, 0, 1_000.0), 0.0);
        assert!((held_envelope(&synth, 100, 1_000.0) - 1.0).abs() < f32::EPSILON);
        assert!((held_envelope(&synth, 200, 1_000.0) - 0.4).abs() < f32::EPSILON);
        assert!((note_envelope(&synth, 250, 200, 1_000.0) - 0.2).abs() < f32::EPSILON);
    }

    #[test]
    fn pitch_shift_and_glide_are_exponential() {
        assert!((pitch_frequency(69, 12) - 880.0).abs() < f32::EPSILON);
        assert!((glide_frequency(220.0, 880.0, 50, 100) - 440.0).abs() < 0.001);
    }

    #[test]
    fn hard_pan_routes_to_only_one_channel() {
        let mut left = [0.0, 0.0];
        let mut right = [0.0, 0.0];

        add_panned(&mut left, 1.0, -1.0);
        add_panned(&mut right, 1.0, 1.0);

        assert!((left[0] - 1.0).abs() < f32::EPSILON);
        assert!(left[1].abs() < f32::EPSILON);
        assert!(right[0].abs() < 0.000_001);
        assert!((right[1] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn mono_arrangement_voices_glide_from_the_previous_note() {
        let mut project = Project::default();
        project.add_instrument();
        let track = &mut project.tracks[0];
        let TrackKind::Instrument { synth } = &mut track.kind else {
            panic!("the default track is an instrument");
        };
        synth.mono = true;
        let channel_id = track.id;
        let pattern_id = track.source_id;
        project
            .add_note(pattern_id, 60, 0, 8, 100)
            .expect("primary pattern should exist");
        project
            .add_note(pattern_id, 72, 4, 4, 100)
            .expect("primary pattern should exist");
        project.ensure_primary_pattern_clip(channel_id);

        let plan = PlaybackPlan::from_project(&project, 48_000.0)
            .expect("synth project should build a playback plan");

        assert_eq!(plan.channels[0].voices.len(), 2);
        assert_eq!(
            plan.channels[0].voices[0].note_off_sample,
            plan.channels[0].voices[1].start_sample
        );
        assert_eq!(
            plan.channels[0].voices[1].glide_from_frequency,
            plan.channels[0].voices[0].frequency
        );
    }

    #[test]
    fn wav_export_writes_stereo_pcm_audio() {
        let mut project = Project::default();
        project.add_instrument();
        project.bpm = 300.0;
        let pattern_id = project.tracks[0].source_id;
        project
            .add_note(pattern_id, 69, 0, 4, 127)
            .expect("primary pattern should exist");
        project.ensure_primary_pattern_clip(project.tracks[0].id);
        let path =
            std::env::temp_dir().join(format!("donttrackme-wav-export-{}.wav", std::process::id()));

        export_wav(&project, &path).expect("test WAV should export");
        let reader = hound::WavReader::open(&path).expect("exported WAV should open");
        let specification = reader.spec();
        let duration = reader.duration();
        std::fs::remove_file(path).expect("test WAV should be removable");

        assert_eq!(specification.channels, 2);
        assert_eq!(specification.sample_rate, 44_100);
        assert_eq!(specification.bits_per_sample, 16);
        assert!(duration > 0);
    }

    #[test]
    fn pattern_plays_from_its_channel_when_placed_on_another_lane() {
        let mut project = Project::default();
        project.add_instrument();
        let source_id = project.tracks[0].source_id;
        project
            .add_note(source_id, 69, 0, 2, 127)
            .expect("primary pattern should exist");
        let other_lane = project.add_instrument();
        project
            .tracks
            .iter_mut()
            .find(|track| track.id == other_lane)
            .expect("new lane should exist")
            .add_clip(source_id, 0, 8);

        let plan = PlaybackPlan::from_project(&project, 48_000.0)
            .expect("synth project should build a playback plan");

        assert_eq!(plan.channels[0].voices.len(), 1);
        assert!((plan.channels[0].voices[0].frequency - 440.0).abs() < f32::EPSILON);
    }

    #[test]
    fn delay_emits_audio_after_the_dry_impulse() {
        let mut slots = DEFAULT_EFFECTS;
        for slot in &mut slots {
            slot.enabled = false;
        }
        slots[3].enabled = true;
        slots[3].kind = EffectKind::Delay {
            time_ms: 1.0,
            feedback: 0.0,
            mix: 1.0,
        };
        let mut effects = EffectChain::new(slots, 1_000.0);

        assert_eq!(effects.process([1.0, 1.0], 1_000.0), [0.0, 0.0]);
        assert_eq!(effects.process([0.0, 0.0], 1_000.0), [1.0, 1.0]);
    }

    #[test]
    fn changing_effect_order_changes_the_signal() {
        let mut first_order = DEFAULT_EFFECTS;
        for slot in &mut first_order {
            slot.enabled = false;
        }
        first_order[0].enabled = true;
        first_order[0].kind = EffectKind::Distortion {
            drive: 10.0,
            mix: 1.0,
        };
        first_order[2].enabled = true;
        first_order[2].kind = EffectKind::Tremolo {
            rate_hz: 1.0,
            depth: 1.0,
        };
        let mut second_order = first_order;
        second_order.swap(0, 2);
        let mut distortion_then_tremolo = EffectChain::new(first_order, 1_000.0);
        let mut tremolo_then_distortion = EffectChain::new(second_order, 1_000.0);

        let first = distortion_then_tremolo.process([0.8, 0.8], 1_000.0);
        let second = tremolo_then_distortion.process([0.8, 0.8], 1_000.0);

        assert_ne!(first, second);
    }

    #[test]
    fn sampler_trimming_and_reverse_choose_the_expected_frames() {
        let sample = SampleBuffer {
            frames: vec![[0.0, 0.0], [0.25, 0.25], [0.5, 0.5], [0.75, 0.75]],
            sample_rate: 4.0,
        };
        let mut sampler = SampleSynth {
            trim_start: 0.25,
            trim_end: 0.75,
            ..SampleSynth::default()
        };

        assert_eq!(
            sampler_frame(&sample, &sampler, 0, 1.0, 4.0),
            Some([0.25, 0.25])
        );
        sampler.reverse = true;
        assert_eq!(
            sampler_frame(&sample, &sampler, 0, 1.0, 4.0),
            Some([0.5, 0.5])
        );
    }

    #[test]
    fn sampler_loop_modes_restart_or_bounce_inside_the_trimmed_region() {
        let sample = SampleBuffer {
            frames: vec![[0.0, 0.0], [0.25, 0.25], [0.5, 0.5], [0.75, 0.75]],
            sample_rate: 4.0,
        };
        let mut sampler = SampleSynth {
            looping: true,
            ..SampleSynth::default()
        };

        assert_eq!(
            sampler_frame(&sample, &sampler, 4, 1.0, 4.0),
            Some([0.0, 0.0])
        );
        sampler.loop_mode = SampleLoopMode::PingPong;
        assert_eq!(
            sampler_frame(&sample, &sampler, 4, 1.0, 4.0),
            Some([0.5, 0.5])
        );
        assert_eq!(
            sampler_frame(&sample, &sampler, 6, 1.0, 4.0),
            Some([0.0, 0.0])
        );
    }

    #[test]
    fn sampler_channel_loads_wav_and_schedules_pattern_notes() {
        let path =
            std::env::temp_dir().join(format!("donttrackme-sampler-{}.wav", std::process::id()));
        let specification = hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, specification)
            .expect("test sample should be creatable");
        for sample in [0_i16, 1_000, -1_000, 0] {
            writer
                .write_sample(sample)
                .expect("test sample should be writable");
        }
        writer
            .finalize()
            .expect("test sample should be finalizable");

        let mut project = Project::default();
        project.add_instrument();
        let channel_id = project.add_sampler();
        let channel = project
            .tracks
            .iter_mut()
            .find(|track| track.id == channel_id)
            .expect("sampler channel should exist");
        let pattern_id = channel.source_id;
        let TrackKind::Sampler { sampler } = &mut channel.kind else {
            panic!("new sampler channel should contain a sampler");
        };
        sampler.path = Some(path.clone());
        project
            .add_note(pattern_id, 60, 0, 4, 127)
            .expect("sampler pattern should exist");
        project.ensure_primary_pattern_clip(channel_id);

        let plan = PlaybackPlan::from_project(&project, 8_000.0)
            .expect("sampler project should build a playback plan");
        std::fs::remove_file(path).expect("test sample should be removable");

        assert_eq!(plan.channels.len(), 1);
        assert_eq!(plan.channels[0].voices.len(), 1);
        assert!(matches!(
            plan.channels[0].instrument,
            RenderInstrument::Sampler { .. }
        ));
    }

    #[test]
    fn sample_tracks_play_clips_and_share_their_decoded_buffer() {
        let path = std::env::temp_dir().join(format!(
            "donttrackme-audio-track-{}.wav",
            std::process::id()
        ));
        let specification = hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, specification)
            .expect("audio track fixture should be created");
        writer
            .write_sample(i16::MAX / 2)
            .expect("audio track fixture should contain a sample");
        writer
            .finalize()
            .expect("audio track fixture should be finalized");

        let mut project = Project::default();
        project.add_instrument();
        let track_id = project.add_sample_with_length(path.clone(), 8);
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .expect("sample track should exist");
        track.add_clip(track.source_id, 8, 8);
        let plan = PlaybackPlan::from_project(&project, 8_000.0)
            .expect("sample clips should build a playback plan");
        std::fs::remove_file(path).expect("audio track fixture should be removable");

        let channel = plan
            .channels
            .iter()
            .find(|channel| channel.voices.len() == 2)
            .expect("both audio clips should be scheduled");
        assert!(Arc::ptr_eq(
            channel.voices[0]
                .sample
                .as_ref()
                .expect("sample voice should have decoded audio"),
            channel.voices[1]
                .sample
                .as_ref()
                .expect("sample voice should have decoded audio")
        ));
    }

    #[test]
    fn pattern_playback_ignores_the_rest_of_the_arrangement() {
        let mut project = Project::default();
        project.add_instrument();
        let first_pattern = project.tracks[0].source_id;
        project
            .add_note(first_pattern, 60, 0, 2, 100)
            .expect("first pattern should exist");
        let second_channel = project.add_instrument();
        let second_pattern = project
            .tracks
            .iter()
            .find(|track| track.id == second_channel)
            .expect("second channel should exist")
            .source_id;
        project
            .add_note(second_pattern, 72, 0, 2, 100)
            .expect("second pattern should exist");
        project.ensure_primary_pattern_clip(second_channel);

        let plan = PlaybackPlan::from_pattern(&project, first_pattern, 48_000.0)
            .expect("pattern should build a playback plan");

        assert_eq!(plan.channels.len(), 1);
        assert_eq!(plan.channels[0].voices.len(), 1);
        assert_eq!(plan.channels[0].voices[0].frequency, pitch_frequency(60, 0));
    }

    #[test]
    fn pausing_does_not_advance_the_playback_position() {
        let mut project = Project::default();
        project.add_instrument();
        let pattern_id = project.tracks[0].source_id;
        project
            .add_note(pattern_id, 60, 0, 2, 100)
            .expect("pattern should exist");
        let plan = PlaybackPlan::from_pattern(&project, pattern_id, 48_000.0)
            .expect("pattern should build a playback plan");
        let (_sender, receiver) = std::sync::mpsc::channel();
        let mut renderer = super::Renderer::new(receiver, 48_000.0);
        renderer.plan = Some(plan);

        renderer.next_frame();
        renderer.paused = true;
        let paused_at = renderer.position;
        renderer.next_frame();

        assert_eq!(renderer.position, paused_at);
    }

    #[test]
    fn renderer_wraps_to_the_selected_loop_start() {
        let mut project = Project::default();
        project.add_instrument();
        let pattern_id = project.tracks[0].source_id;
        let plan = PlaybackPlan::from_pattern(&project, pattern_id, 48_000.0)
            .expect("pattern should build a playback plan");
        let (_sender, receiver) = std::sync::mpsc::channel();
        let mut renderer = super::Renderer::new(receiver, 48_000.0);
        renderer.plan = Some(plan);
        renderer.position = 3;
        renderer.loop_range = Some((2, 4));

        renderer.next_frame();

        assert_eq!(renderer.position, 2);
    }
}
