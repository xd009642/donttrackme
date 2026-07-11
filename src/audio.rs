use std::{
    path::Path,
    sync::mpsc::{self, Receiver, Sender},
};

use cpal::{
    FromSample, Sample, SampleFormat, SizedSample, Stream,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

use crate::model::{
    ARRANGEMENT_STEPS, EffectKind, EffectSlot, FilterKind, Project, STEPS_PER_BEAT,
    SimpleWaveformSynth, TrackKind, noise_sample,
};

enum Command {
    Play(PlaybackPlan),
    Stop,
    AuditionStart {
        pitch: u8,
        synth: SimpleWaveformSynth,
    },
    AuditionStop {
        pitch: u8,
    },
}

struct PlaybackPlan {
    channels: Vec<ChannelPlan>,
    loop_samples: u64,
}

struct ChannelPlan {
    synth: SimpleWaveformSynth,
    voices: Vec<Voice>,
    effects: EffectChain,
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
        })
    }

    pub fn play(&self, project: &Project) -> Result<(), String> {
        self.commands
            .send(Command::Play(PlaybackPlan::from_project(
                project,
                self.sample_rate,
            )))
            .map_err(|_| "The audio output stream has stopped".to_owned())
    }

    pub fn stop(&self) -> Result<(), String> {
        self.commands
            .send(Command::Stop)
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
}

pub fn export_wav(project: &Project, path: &Path) -> Result<(), String> {
    const SAMPLE_RATE: u32 = 44_100;
    let plan = PlaybackPlan::from_project(project, SAMPLE_RATE as f32);
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
    fn from_project(project: &Project, sample_rate: f32) -> Self {
        let samples_per_step = sample_rate * 60.0 / project.bpm / f32::from(STEPS_PER_BEAT);
        let any_solo = project.tracks.iter().any(|track| track.solo);
        let mut channels = Vec::new();

        for channel in &project.tracks {
            if channel.muted || (any_solo && !channel.solo) {
                continue;
            }
            let TrackKind::Instrument { synth } = channel.kind else {
                continue;
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
                        let frequency = pitch_frequency(note.pitch, synth.pitch_shift);
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
            if synth.mono {
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
                    synth,
                    voices: channel_voices,
                    effects: EffectChain::new(synth.effects, sample_rate),
                });
            }
        }

        Self {
            channels,
            loop_samples: (f32::from(ARRANGEMENT_STEPS) * samples_per_step).round() as u64,
        }
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
    sample_rate: f32,
    audition_voices: Vec<AuditionVoice>,
    audition_effects: Option<EffectChain>,
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

impl Renderer {
    fn new(receiver: Receiver<Command>, sample_rate: f32) -> Self {
        Self {
            receiver,
            plan: None,
            position: 0,
            sample_rate,
            audition_voices: Vec::with_capacity(40),
            audition_effects: None,
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
                }
                Command::Stop => {
                    self.plan = None;
                    self.position = 0;
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
        if let Some(plan) = &mut self.plan {
            for channel in &mut plan.channels {
                let mut channel_output = [0.0, 0.0];
                for voice in &mut channel.voices {
                    if self.position < voice.start_sample {
                        continue;
                    }
                    let elapsed = self.position - voice.start_sample;
                    let release_samples = ms_samples(channel.synth.release_ms, self.sample_rate);
                    if self.position >= voice.note_off_sample + release_samples {
                        continue;
                    }
                    let note_length = voice.note_off_sample - voice.start_sample;
                    let envelope =
                        note_envelope(&channel.synth, elapsed, note_length, self.sample_rate);
                    let frequency = glide_frequency(
                        voice.glide_from_frequency,
                        voice.frequency,
                        elapsed,
                        ms_samples(channel.synth.glide_ms, self.sample_rate),
                    );
                    let raw =
                        oscillator_sample(&channel.synth, elapsed, frequency, self.sample_rate)
                            * envelope
                            * voice.gain
                            * channel.synth.master_level;
                    let filtered = voice.filter.process(raw, &channel.synth, self.sample_rate);
                    add_panned(&mut channel_output, filtered, channel.synth.pan);
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

        [output[0].tanh(), output[1].tanh()]
    }
}

impl FilterState {
    fn process(&mut self, input: f32, synth: &SimpleWaveformSynth, sample_rate: f32) -> f32 {
        if synth.filter == FilterKind::Off {
            return input;
        }
        let cutoff = synth.filter_cutoff_hz.clamp(20.0, sample_rate * 0.45);
        let frequency = ((std::f32::consts::PI * cutoff / sample_rate).sin() * 2.0).min(0.99);
        let damping = (2.0 * (1.0 - synth.filter_resonance.powf(0.25))).clamp(0.05, 2.0);
        self.low += frequency * self.band;
        let high = input - self.low - damping * self.band;
        self.band += frequency * high;
        match synth.filter {
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
        EffectChain, PlaybackPlan, add_panned, export_wav, glide_frequency, held_envelope,
        note_envelope, pitch_frequency,
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

        let plan = PlaybackPlan::from_project(&project, 100.0);

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

        let plan = PlaybackPlan::from_project(&project, 48_000.0);

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

        let plan = PlaybackPlan::from_project(&project, 48_000.0);

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
}
