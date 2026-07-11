use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

#[derive(Debug)]
pub enum TrackKind {
    Instrument { waveform: Waveform },
    Sample { path: PathBuf },
}

#[derive(Clone, Copy, Debug)]
pub struct Note {
    pub id: u64,
    pub pitch: u8,
    pub start_step: u16,
    pub length_steps: u16,
    pub velocity: u8,
}

#[derive(Clone, Debug)]
pub struct Clip {
    pub id: u64,
    pub name: String,
    pub start_step: u16,
    pub length_steps: u16,
}

#[derive(Debug)]
pub struct Track {
    pub id: u64,
    pub name: String,
    pub kind: TrackKind,
    pub notes: Vec<Note>,
    pub clips: Vec<Clip>,
    pub muted: bool,
    pub solo: bool,
    next_note_id: u64,
    next_clip_id: u64,
}

impl Track {
    pub fn instrument(id: u64, name: String) -> Self {
        Self {
            id,
            name,
            kind: TrackKind::Instrument {
                waveform: Waveform::Sine,
            },
            notes: Vec::new(),
            clips: Vec::new(),
            muted: false,
            solo: false,
            next_note_id: 1,
            next_clip_id: 1,
        }
    }

    pub fn sample(id: u64, path: PathBuf) -> Self {
        let name = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("Sample")
            .to_owned();
        let mut track = Self {
            id,
            name,
            kind: TrackKind::Sample { path },
            notes: Vec::new(),
            clips: Vec::new(),
            muted: false,
            solo: false,
            next_note_id: 1,
            next_clip_id: 1,
        };
        track.add_clip(0, 8);
        track
    }

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

    pub fn add_clip(&mut self, start_step: u16, length_steps: u16) -> u64 {
        let id = self.next_clip_id;
        self.next_clip_id += 1;
        let name = match self.kind {
            TrackKind::Instrument { .. } => format!("Pattern {id}"),
            TrackKind::Sample { .. } => self.name.clone(),
        };
        self.clips.push(Clip {
            id,
            name,
            start_step,
            length_steps,
        });
        id
    }

    pub fn ensure_pattern_clip(&mut self) {
        if self.clips.is_empty() {
            self.add_clip(0, 16);
        }
    }
}

#[derive(Debug)]
pub struct Project {
    pub bpm: f32,
    pub tracks: Vec<Track>,
    next_track_id: u64,
}

impl Default for Project {
    fn default() -> Self {
        let mut project = Self {
            bpm: 120.0,
            tracks: Vec::new(),
            next_track_id: 1,
        };
        project.add_instrument();
        project
    }
}

impl Project {
    pub fn add_instrument(&mut self) -> u64 {
        let id = self.next_track_id;
        self.next_track_id += 1;
        let number = self
            .tracks
            .iter()
            .filter(|track| matches!(track.kind, TrackKind::Instrument { .. }))
            .count()
            + 1;
        self.tracks
            .push(Track::instrument(id, format!("Simple waveform {number}")));
        id
    }

    pub fn add_sample(&mut self, path: PathBuf) -> u64 {
        let id = self.next_track_id;
        self.next_track_id += 1;
        self.tracks.push(Track::sample(id, path));
        id
    }
}

#[cfg(test)]
mod tests {
    use super::Track;

    /// Note identities remain distinct so selection survives moving and resizing notes.
    #[test]
    fn notes_receive_stable_unique_ids() {
        let mut track = Track::instrument(1, "Synth".to_owned());

        let first = track.add_note(60, 0, 1, 100);
        let second = track.add_note(64, 4, 2, 90);
        track.notes[0].start_step = 8;

        assert_ne!(first, second);
        assert_eq!(track.notes[0].id, first);
        assert_eq!(track.notes[0].start_step, 8);
    }

    /// Adding more notes reuses the existing pattern clip instead of obscuring it with duplicates.
    #[test]
    fn pattern_clip_is_created_once() {
        let mut track = Track::instrument(1, "Synth".to_owned());

        track.ensure_pattern_clip();
        track.ensure_pattern_clip();

        assert_eq!(track.clips.len(), 1);
        assert_eq!(track.clips[0].start_step, 0);
        assert_eq!(track.clips[0].length_steps, 16);
    }
}
