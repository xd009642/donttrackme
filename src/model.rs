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
    Sample,
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
    pub source_id: u64,
    pub start_step: u16,
    pub length_steps: u16,
}

#[derive(Clone, Debug)]
pub struct ClipSource {
    pub id: u64,
    pub track_id: u64,
    pub name: String,
    pub length_steps: u16,
    pub kind: ClipSourceKind,
}

#[derive(Clone, Debug)]
pub enum ClipSourceKind {
    Pattern,
    Sample { path: PathBuf },
}

#[derive(Debug)]
pub struct Track {
    pub id: u64,
    pub name: String,
    pub kind: TrackKind,
    pub source_id: u64,
    pub notes: Vec<Note>,
    pub clips: Vec<Clip>,
    pub muted: bool,
    pub solo: bool,
    next_note_id: u64,
    next_clip_id: u64,
}

impl Track {
    pub fn instrument(id: u64, source_id: u64, name: String) -> Self {
        Self {
            id,
            name,
            kind: TrackKind::Instrument {
                waveform: Waveform::Sine,
            },
            source_id,
            notes: Vec::new(),
            clips: Vec::new(),
            muted: false,
            solo: false,
            next_note_id: 1,
            next_clip_id: 1,
        }
    }

    pub fn sample(id: u64, source_id: u64, path: PathBuf) -> Self {
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
        self.clips.push(Clip {
            id,
            source_id: self.source_id,
            start_step,
            length_steps,
        });
        id
    }

    pub fn ensure_pattern_clip(&mut self) {
        if self.clips.is_empty() {
            self.add_clip(0, 32);
        }
    }
}

#[derive(Debug)]
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
            track_id: id,
            name: format!("Pattern {number}"),
            length_steps: 32,
            kind: ClipSourceKind::Pattern,
        });
        self.tracks.push(Track::instrument(id, source_id, name));
        id
    }

    pub fn add_sample(&mut self, path: PathBuf) -> u64 {
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
            track_id: id,
            name,
            length_steps: 8,
            kind: ClipSourceKind::Sample { path: path.clone() },
        });
        self.tracks.push(Track::sample(id, source_id, path));
        id
    }

    pub fn source(&self, id: u64) -> Option<&ClipSource> {
        self.clip_library.iter().find(|source| source.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::{Project, Track};

    /// Note identities remain distinct so selection survives moving and resizing notes.
    #[test]
    fn notes_receive_stable_unique_ids() {
        let mut track = Track::instrument(1, 1, "Synth".to_owned());

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
        let mut track = Track::instrument(1, 1, "Synth".to_owned());

        track.ensure_pattern_clip();
        track.ensure_pattern_clip();

        assert_eq!(track.clips.len(), 1);
        assert_eq!(track.clips[0].start_step, 0);
        assert_eq!(track.clips[0].length_steps, 32);
    }

    /// Trimming an arrangement instance never changes its reusable library source.
    #[test]
    fn trimming_clip_preserves_source_length() {
        let mut project = Project::default();
        let track = &mut project.tracks[0];
        track.ensure_pattern_clip();
        let source_id = track.clips[0].source_id;
        track.clips[0].length_steps = 8;

        assert_eq!(
            project.source(source_id).map(|source| source.length_steps),
            Some(32)
        );
        assert_eq!(project.tracks[0].clips[0].length_steps, 8);
    }
}
