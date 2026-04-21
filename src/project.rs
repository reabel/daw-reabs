use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::track::Track;

/// Persistent project state serialized to/from JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    /// Sample rate used when the project was created.
    pub sample_rate: u32,
    /// BPM for the tempo map (single tempo for MVP).
    pub bpm: f64,
    /// Numerator of the time signature (e.g. 4 in 4/4).
    pub time_sig_numerator: u8,
    /// Denominator of the time signature as a power of two (e.g. 4 in 4/4).
    pub time_sig_denominator: u8,
    pub tracks: Vec<Track>,
    /// Path of the most recent save location (not serialized itself).
    #[serde(skip)]
    pub path: Option<PathBuf>,
}

impl Project {
    /// Create a blank project with sensible defaults.
    pub fn new(name: impl Into<String>, sample_rate: u32) -> Self {
        Self {
            name: name.into(),
            sample_rate,
            bpm: 120.0,
            time_sig_numerator: 4,
            time_sig_denominator: 4,
            tracks: Vec::new(),
            path: None,
        }
    }

    /// Load a project from a `.dawproj` JSON file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let data = fs::read_to_string(path)
            .with_context(|| format!("Failed to read project file: {}", path.display()))?;
        let mut project: Project = serde_json::from_str(&data)
            .with_context(|| format!("Failed to parse project file: {}", path.display()))?;
        project.path = Some(path.to_path_buf());
        Ok(project)
    }

    /// Save the project to its current path or the given path.
    pub fn save(&mut self, path: Option<impl AsRef<Path>>) -> Result<PathBuf> {
        let target = match path {
            Some(p) => {
                let p = p.as_ref().to_path_buf();
                self.path = Some(p.clone());
                p
            }
            None => self
                .path
                .clone()
                .context("No save path set; provide a path for the first save")?,
        };

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        let json = serde_json::to_string_pretty(self).context("Failed to serialize project")?;
        fs::write(&target, json)
            .with_context(|| format!("Failed to write project: {}", target.display()))?;

        Ok(target)
    }

    /// Add a track to the project and return its index.
    pub fn add_track(&mut self, track: Track) -> usize {
        let idx = self.tracks.len();
        self.tracks.push(track);
        idx
    }

    /// Remove a track by index.
    pub fn remove_track(&mut self, index: usize) -> Option<Track> {
        if index < self.tracks.len() {
            Some(self.tracks.remove(index))
        } else {
            None
        }
    }

    /// Duration of the longest track in samples.
    pub fn length_samples(&self) -> u64 {
        self.tracks
            .iter()
            .map(|t| t.length_samples())
            .max()
            .unwrap_or(0)
    }

    /// Duration in seconds.
    pub fn length_seconds(&self) -> f64 {
        self.length_samples() as f64 / self.sample_rate as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn round_trip_save_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.dawproj");

        let mut project = Project::new("Test", 44100);
        project.bpm = 140.0;
        project.save(Some(&path)).unwrap();

        let loaded = Project::open(&path).unwrap();
        assert_eq!(loaded.name, "Test");
        assert_eq!(loaded.bpm, 140.0);
    }
}
