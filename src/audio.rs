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
    FromSample, Sample, SampleFormat, SizedSample, Stream,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

use crate::model::{
    ARRANGEMENT_STEPS, DEFAULT_EFFECTS, EffectKind, EffectSlot, FilterKind, Project,
    STEPS_PER_BEAT, SampleSynth, SimpleWaveformSynth, TrackKind, noise_sample,
};

enum Command {
    Play(PlaybackPlan),
    Stop,
    Pause,
    Resume,
    AuditionStart {
        pitch: u8,
        synth: SimpleWaveformSynth,
    },
    AuditionStop {
        pitch: u8,
    },
    AuditionSampleStart {
        pitch: u8,
        sampler: SampleSynth,
        sample: Arc<SampleBuffer>,
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
}

enum RenderInstrument {
    Synth(SimpleWaveformSynth),
    Sampler {
        sampler: SampleSynth,
        sample: Arc<SampleBuffer>,
    },
}

struct SampleBuffer {
    frames: Vec<[f32; 2]>,
    sample_rate: f32,
}

struct Voice {
    start_sample: u64,
    note_off_sample: u64,
    frequency: f32,
    glide_from_frequency: f32,
    gain: f32,
    filter: FilterState,
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
        let config = supported.config();
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

    pub fn play_pattern(&self, project: &Project, pattern_id: u64) -> Result<(), String> {
        self.commands
            .send(Command::Play(PlaybackPlan::from_pattern(
                project,
                pattern_id,
                self.sample_rate,
            )?))
            .map_err(|_| "The audio output stream has stopped".to_owned())
    }

    pub fn audition_start(&self, pitch: u8, synth: SimpleWaveformSynth) -> Result<(), String> {
        self.commands
            .send(Command::AuditionStart { pitch, synth })
            .map_err(|_| "The audio output stream has stopped".to_owned())
    }

    pub fn audition_stop(&self, pitch: u8) -> Result<(), String> {
        self.commands
            .send(Command::AuditionStop { pitch })
            .map_err(|_| "The audio output stream has stopped".to_owned())
    }

    pub fn audition_sample_start(&self, pitch: u8, sampler: SampleSynth) -> Result<(), String> {
        let Some(path) = &sampler.path else {
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
            cache.insert(path.clone(), Arc::clone(&sample));
            sample
        };
        self.commands
            .send(Command::AuditionSampleStart {
                pitch,
                sampler,
                sample,
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

impl PlaybackPlan {
    fn from_project(project: &Project, sample_rate: f32) -> Result<Self, String> {
        let samples_per_step = sample_rate * 60.0 / project.bpm / f32::from(STEPS_PER_BEAT);
        let any_solo = project.tracks.iter().any(|track| track.solo);
        let mut channels = Vec::new();

        for channel in &project.tracks {
            if channel.muted || (any_solo && !channel.solo) {
                continue;
            }
            let instrument = match &channel.kind {
                TrackKind::Instrument { synth } => RenderInstrument::Synth(*synth),
                TrackKind::Sampler { sampler } => {
                    let Some(path) = &sampler.path else {
                        continue;
                    };
                    RenderInstrument::Sampler {
                        sampler: sampler.clone(),
                        sample: Arc::new(load_wav(path)?),
                    }
                }
                TrackKind::Sample => continue,
            };
            let mut channel_voices = Vec::new();
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
                            RenderInstrument::Sampler { sampler, .. } => {
                                2.0_f32.powf(
                                    (f32::from(note.pitch) - f32::from(sampler.root_pitch)) / 12.0,
                                ) * sampler.speed
                            }
                        };
                        channel_voices.push(Voice {
                            start_sample: (f32::from(start_step) * samples_per_step).round() as u64,
                            note_off_sample: (f32::from(end_step) * samples_per_step).round()
                                as u64,
                            frequency,
                            glide_from_frequency: frequency,
                            gain: f32::from(note.velocity) / 127.0,
                            filter: FilterState::default(),
                        });
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
                    effects: match &instrument {
                        RenderInstrument::Synth(synth) => {
                            EffectChain::new(synth.effects, sample_rate)
                        }
                        RenderInstrument::Sampler { .. } => {
                            EffectChain::new(DEFAULT_EFFECTS, sample_rate)
                        }
                    },
                    instrument,
                    voices: channel_voices,
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
            TrackKind::Sampler { sampler } => {
                let path = sampler
                    .path
                    .as_ref()
                    .ok_or_else(|| "Load a WAV into the sampler first".to_owned())?;
                RenderInstrument::Sampler {
                    sampler: sampler.clone(),
                    sample: Arc::new(load_wav(path)?),
                }
            }
            TrackKind::Sample => return Err("Sample tracks do not have patterns".to_owned()),
        };
        let samples_per_step = sample_rate * 60.0 / project.bpm / f32::from(STEPS_PER_BEAT);
        let mut voices = pattern
            .notes
            .iter()
            .map(|note| {
                let frequency = match &instrument {
                    RenderInstrument::Synth(synth) => {
                        pitch_frequency(note.pitch, synth.pitch_shift)
                    }
                    RenderInstrument::Sampler { sampler, .. } => {
                        2.0_f32.powf((f32::from(note.pitch) - f32::from(sampler.root_pitch)) / 12.0)
                            * sampler.speed
                    }
                };
                Voice {
                    start_sample: (f32::from(note.start_step) * samples_per_step).round() as u64,
                    note_off_sample: (f32::from(note.start_step + note.length_steps)
                        * samples_per_step)
                        .round() as u64,
                    frequency,
                    glide_from_frequency: frequency,
                    gain: f32::from(note.velocity) / 127.0,
                    filter: FilterState::default(),
                }
            })
            .collect::<Vec<_>>();
        voices.sort_by_key(|voice| voice.start_sample);
        if matches!(&instrument, RenderInstrument::Synth(synth) if synth.mono) {
            for index in 1..voices.len() {
                voices[index].glide_from_frequency = voices[index - 1].frequency;
                voices[index - 1].note_off_sample = voices[index - 1]
                    .note_off_sample
                    .min(voices[index].start_sample);
            }
        }
        let effects = match &instrument {
            RenderInstrument::Synth(synth) => EffectChain::new(synth.effects, sample_rate),
            RenderInstrument::Sampler { .. } => EffectChain::new(DEFAULT_EFFECTS, sample_rate),
        };
        Ok(Self {
            channels: vec![ChannelPlan {
                instrument,
                voices,
                effects,
            }],
            loop_samples: (f32::from(source.length_steps) * samples_per_step).round() as u64,
        })
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
    sample_rate: f32,
    audition_voices: Vec<AuditionVoice>,
    audition_effects: Option<EffectChain>,
    audition_samples: Vec<AuditionSampleVoice>,
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
    released_at: Option<u64>,
    filter: FilterState,
    finished: bool,
}

impl Renderer {
    fn new(receiver: Receiver<Command>, sample_rate: f32) -> Self {
        Self {
            receiver,
            plan: None,
            position: 0,
            paused: false,
            sample_rate,
            audition_voices: Vec::with_capacity(40),
            audition_effects: None,
            audition_samples: Vec::with_capacity(40),
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
                Command::AuditionStart { pitch, synth } => self.start_audition(pitch, synth),
                Command::AuditionStop { pitch } => {
                    for voice in self
                        .audition_voices
                        .iter_mut()
                        .filter(|voice| voice.pitch == pitch && voice.released_at.is_none())
                    {
                        let level = held_envelope(&voice.synth, voice.elapsed, self.sample_rate);
                        voice.released_at = Some((voice.elapsed, level));
                    }
                    for voice in self
                        .audition_samples
                        .iter_mut()
                        .filter(|voice| voice.pitch == pitch && voice.released_at.is_none())
                    {
                        voice.released_at = Some(voice.elapsed);
                    }
                }
                Command::AuditionSampleStart {
                    pitch,
                    sampler,
                    sample,
                } => {
                    let playback_rate = 2.0_f32
                        .powf((f32::from(pitch) - f32::from(sampler.root_pitch)) / 12.0)
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

    fn start_audition(&mut self, pitch: u8, synth: SimpleWaveformSynth) {
        if self.audition_voices.is_empty() {
            self.audition_effects = Some(EffectChain::new(synth.effects, self.sample_rate));
        }
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

    fn next_frame(&mut self) -> [f32; 2] {
        let mut output = [0.0, 0.0];
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
                            let filtered = voice.filter.process(raw, synth, self.sample_rate);
                            add_panned(&mut channel_output, filtered, synth.pan);
                        }
                    }
                    RenderInstrument::Sampler { sampler, sample } => {
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
                            let attack = ms_samples(sampler.attack_ms, self.sample_rate);
                            let envelope = if elapsed < attack && attack > 0 {
                                elapsed as f32 / attack as f32
                            } else if elapsed < note_length {
                                1.0
                            } else if release > 0 {
                                (1.0 - (elapsed - note_length) as f32 / release as f32)
                                    .clamp(0.0, 1.0)
                            } else {
                                0.0
                            };
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
                                sampler.filter_cutoff_hz,
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
            if self.position >= plan.loop_samples {
                self.position = 0;
                for channel in &mut plan.channels {
                    for voice in &mut channel.voices {
                        voice.filter = FilterState::default();
                    }
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
        if let Some(effects) = &mut self.audition_effects {
            let effected = effects.process(audition_output, self.sample_rate);
            output[0] += effected[0];
            output[1] += effected[1];
        }
        for voice in &mut self.audition_samples {
            let envelope = match voice.released_at {
                Some(released_at) => {
                    let release = ms_samples(voice.sampler.release_ms, self.sample_rate);
                    if release == 0 {
                        0.0
                    } else {
                        (1.0 - (voice.elapsed - released_at) as f32 / release as f32)
                            .clamp(0.0, 1.0)
                    }
                }
                None => {
                    let attack = ms_samples(voice.sampler.attack_ms, self.sample_rate);
                    if voice.elapsed < attack && attack > 0 {
                        voice.elapsed as f32 / attack as f32
                    } else {
                        1.0
                    }
                }
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
            add_panned(&mut output, filtered, voice.sampler.pan);
            voice.elapsed += 1;
        }
        self.audition_samples.retain(|voice| {
            !voice.finished
                && voice.released_at.is_none_or(|released_at| {
                    voice.elapsed
                        < released_at + ms_samples(voice.sampler.release_ms, self.sample_rate)
                })
        });

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
            crate::model::SampleLoopMode::Forward => position % length,
            crate::model::SampleLoopMode::PingPong => {
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
        EffectChain, PlaybackPlan, RenderInstrument, SampleBuffer, add_panned, export_wav,
        glide_frequency, held_envelope, note_envelope, pitch_frequency, sampler_frame,
    };
    use crate::model::{DEFAULT_EFFECTS, EffectKind, Project, SimpleWaveformSynth, TrackKind};

    #[test]
    fn playback_plan_places_and_trims_notes_with_the_clip() {
        let mut project = Project::default();
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
        let mut sampler = crate::model::SampleSynth {
            trim_start: 0.25,
            trim_end: 0.75,
            ..crate::model::SampleSynth::default()
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
        let mut sampler = crate::model::SampleSynth {
            looping: true,
            ..crate::model::SampleSynth::default()
        };

        assert_eq!(
            sampler_frame(&sample, &sampler, 4, 1.0, 4.0),
            Some([0.0, 0.0])
        );
        sampler.loop_mode = crate::model::SampleLoopMode::PingPong;
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
    fn pattern_playback_ignores_the_rest_of_the_arrangement() {
        let mut project = Project::default();
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
}
