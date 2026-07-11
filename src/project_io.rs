use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::model::Project;

const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    format_version: u32,
    project: Project,
}

#[derive(Serialize)]
struct ProjectFileRef<'a> {
    format_version: u32,
    project: &'a Project,
}

pub fn save(project: &Project, path: &Path) -> Result<(), String> {
    let contents = serde_json::to_string_pretty(&ProjectFileRef {
        format_version: FORMAT_VERSION,
        project,
    })
    .map_err(|error| format!("Could not encode the project: {error}"))?;
    fs::write(path, contents).map_err(|error| format!("Could not save the project: {error}"))
}

pub fn load(path: &Path) -> Result<Project, String> {
    let contents =
        fs::read_to_string(path).map_err(|error| format!("Could not read the project: {error}"))?;
    let file: ProjectFile = serde_json::from_str(&contents)
        .map_err(|error| format!("Could not decode the project: {error}"))?;
    if file.format_version != FORMAT_VERSION {
        return Err(format!(
            "Project format {} is unsupported; this build supports format {FORMAT_VERSION}",
            file.format_version
        ));
    }
    Ok(file.project)
}

#[cfg(test)]
mod tests {
    use super::{load, save};
    use crate::model::Project;

    #[test]
    fn project_round_trip_preserves_editable_state() {
        let mut project = Project::default();
        project.bpm = 137.5;
        project.tracks[0].add_note(72, 3, 5, 91);
        project.tracks[0].ensure_pattern_clip();
        let path = std::env::temp_dir().join(format!(
            "donttrackme-project-round-trip-{}.dtm",
            std::process::id()
        ));

        save(&project, &path).expect("test project should save");
        let mut loaded = load(&path).expect("test project should load");
        std::fs::remove_file(path).expect("test project should be removable");

        assert_eq!(loaded.bpm, 137.5);
        assert_eq!(loaded.tracks[0].notes.len(), 1);
        assert_eq!(loaded.tracks[0].notes[0].velocity, 91);
        assert_eq!(loaded.tracks[0].clips.len(), 1);
        assert_eq!(loaded.tracks[0].add_note(60, 0, 1, 100), 2);
        assert_eq!(loaded.add_instrument(), 2);
    }
}
