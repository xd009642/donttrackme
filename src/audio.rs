use std::sync::mpsc::{self, Receiver, Sender};

use cpal::{
    FromSample, Sample, SampleFormat, SizedSample, Stream,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

use crate::model::{Project, SimpleWaveformSynth, TrackKind};

const ARRANGEMENT_STEPS: u16 = 128;

enum Command {
    Play(PlaybackPlan),
    Stop,
}

struct PlaybackPlan {
    voices: Vec<Voice>,
    loop_samples: u64,
}

struct Voice {
    start_sample: u64,
    note_off_sample: u64,
    release_samples: u64,
    attack_samples: u64,
    frequency: f32,
    gain: f32,
    synth: SimpleWaveformSynth,
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
}

impl PlaybackPlan {
    fn from_project(project: &Project, sample_rate: f32) -> Self {
        let seconds_per_step = 60.0 / project.bpm / 4.0;
        let samples_per_step = sample_rate * seconds_per_step;
        let any_solo = project.tracks.iter().any(|track| track.solo);
        let mut voices = Vec::new();

        for track in &project.tracks {
            if track.muted || (any_solo && !track.solo) {
                continue;
            }
            let TrackKind::Instrument { synth } = track.kind else {
                continue;
            };
            for clip in &track.clips {
                for note in &track.notes {
                    if note.start_step >= clip.length_steps {
                        continue;
                    }
                    let start_step = clip.start_step + note.start_step;
                    if start_step >= ARRANGEMENT_STEPS {
                        continue;
                    }
                    let note_end = note.start_step + note.length_steps;
                    let end_step = clip.start_step + note_end.min(clip.length_steps);
                    let start_sample = (f32::from(start_step) * samples_per_step).round() as u64;
                    let note_off_sample = (f32::from(end_step) * samples_per_step).round() as u64;
                    voices.push(Voice {
                        start_sample,
                        note_off_sample,
                        release_samples: (synth.release_ms * sample_rate / 1_000.0).round() as u64,
                        attack_samples: (synth.attack_ms * sample_rate / 1_000.0).round() as u64,
                        frequency: 440.0 * 2.0_f32.powf((f32::from(note.pitch) - 69.0) / 12.0),
                        gain: f32::from(note.velocity) / 127.0,
                        synth,
                    });
                }
            }
        }

        Self {
            voices,
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
    let sample_rate = config.sample_rate as f32;
    let mut renderer = Renderer::new(receiver, sample_rate);
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
}

impl Renderer {
    fn new(receiver: Receiver<Command>, sample_rate: f32) -> Self {
        Self {
            receiver,
            plan: None,
            position: 0,
            sample_rate,
        }
    }

    fn render<T>(&mut self, output: &mut [T], channels: usize)
    where
        T: Sample + FromSample<f32>,
    {
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
            }
        }

        for frame in output.chunks_mut(channels) {
            let value = self.next_sample();
            for sample in frame {
                *sample = T::from_sample(value);
            }
        }
    }

    fn next_sample(&mut self) -> f32 {
        let Some(plan) = &self.plan else {
            return 0.0;
        };
        let mut mixed = 0.0;
        for voice in &plan.voices {
            if self.position < voice.start_sample
                || self.position >= voice.note_off_sample + voice.release_samples
            {
                continue;
            }
            let elapsed = self.position - voice.start_sample;
            let envelope = if elapsed < voice.attack_samples && voice.attack_samples > 0 {
                elapsed as f32 / voice.attack_samples as f32
            } else if self.position < voice.note_off_sample {
                1.0
            } else if voice.release_samples > 0 {
                1.0 - (self.position - voice.note_off_sample) as f32 / voice.release_samples as f32
            } else {
                0.0
            };
            let phase = elapsed as f32 * voice.frequency / self.sample_rate;
            let hash = (elapsed as u32)
                .wrapping_mul(747_796_405)
                .wrapping_add(voice.start_sample as u32);
            let noise = hash as f32 / u32::MAX as f32 * 2.0 - 1.0;
            mixed += voice.synth.waveform.sample(phase, noise)
                * envelope
                * voice.gain
                * voice.synth.level;
        }
        self.position += 1;
        if self.position >= plan.loop_samples {
            self.position = 0;
        }
        mixed.tanh()
    }
}

#[cfg(test)]
mod tests {
    use super::PlaybackPlan;
    use crate::model::Project;

    #[test]
    fn playback_plan_places_and_trims_notes_with_the_clip() {
        let mut project = Project::default();
        project.bpm = 60.0;
        let track = &mut project.tracks[0];
        track.add_note(69, 2, 4, 127);
        track.ensure_pattern_clip();
        track.clips[0].start_step = 4;
        track.clips[0].length_steps = 3;

        let plan = PlaybackPlan::from_project(&project, 100.0);

        assert_eq!(plan.voices.len(), 1);
        assert_eq!(plan.voices[0].start_sample, 150);
        assert_eq!(plan.voices[0].note_off_sample, 175);
        assert!((plan.voices[0].frequency - 440.0).abs() < f32::EPSILON);
    }

    #[test]
    fn muted_tracks_are_not_scheduled() {
        let mut project = Project::default();
        let track = &mut project.tracks[0];
        track.add_note(60, 0, 1, 100);
        track.ensure_pattern_clip();
        track.muted = true;

        let plan = PlaybackPlan::from_project(&project, 48_000.0);

        assert!(plan.voices.is_empty());
    }
}
