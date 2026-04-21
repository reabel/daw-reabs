use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A single audio clip (region) on a track, backed by a WAV file on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioClip {
    /// Display name of the clip.
    pub name: String,
    /// Absolute or project-relative path to the source WAV file.
    pub source_path: PathBuf,
    /// Start position of the clip on the timeline (in samples).
    pub timeline_start: u64,
    /// Offset into the source file where playback begins (in samples).
    pub source_offset: u64,
    /// Number of samples from `source_offset` to play. `None` = entire file.
    pub length: Option<u64>,
    /// Linear gain applied to this clip (1.0 = unity).
    pub gain: f32,
    /// Fade-in duration in samples.
    pub fade_in_samples: u64,
    /// Fade-out duration in samples.
    pub fade_out_samples: u64,
    /// When true the clip is excluded from playback and export.
    pub muted: bool,
}

impl AudioClip {
    pub fn new(name: impl Into<String>, source_path: PathBuf, timeline_start: u64) -> Self {
        Self {
            name: name.into(),
            source_path,
            timeline_start,
            source_offset: 0,
            length: None,
            gain: 1.0,
            fade_in_samples: 0,
            fade_out_samples: 0,
            muted: false,
        }
    }

    /// The clip's effective length in samples. Returns `length` if set,
    /// otherwise falls back to the source file duration.
    pub fn effective_length(&self) -> u64 {
        match self.length {
            Some(l) => l,
            None => hound::WavReader::open(&self.source_path)
                .map(|r| {
                    let spec = r.spec();
                    let n = r.len() as u64;
                    // `len` returns total samples across all channels; divide by channels.
                    n / spec.channels as u64
                })
                .unwrap_or(0),
        }
    }

    /// Timeline end position (exclusive) in samples.
    pub fn timeline_end(&self) -> u64 {
        self.timeline_start + self.effective_length()
    }

    /// Compute the gain envelope at a position *within* the clip (0-based from
    /// `timeline_start`). Handles fade-in and fade-out.
    #[inline]
    pub fn envelope_at(&self, clip_pos: u64) -> f32 {
        let len = self.effective_length();
        let mut env = self.gain;

        if self.fade_in_samples > 0 && clip_pos < self.fade_in_samples {
            env *= clip_pos as f32 / self.fade_in_samples as f32;
        }

        if self.fade_out_samples > 0 && len > 0 {
            let fade_start = len.saturating_sub(self.fade_out_samples);
            if clip_pos >= fade_start {
                let offset = clip_pos - fade_start;
                env *= 1.0 - (offset as f32 / self.fade_out_samples as f32);
            }
        }

        env
    }
}

/// Track kind used to select appropriate processing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackKind {
    Audio,
    Bus,
}

/// A single track in a DAW project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    pub kind: TrackKind,
    /// Linear volume scalar (0.0 – 2.0 typical, 1.0 = 0 dBFS).
    pub volume: f32,
    /// Pan position in [-1.0, 1.0] (0.0 = centre).
    pub pan: f32,
    pub muted: bool,
    pub soloed: bool,
    pub arm: bool,
    /// The clips arranged on this track's timeline.
    pub clips: Vec<AudioClip>,
}

impl Track {
    pub fn audio(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: TrackKind::Audio,
            volume: 1.0,
            pan: 0.0,
            muted: false,
            soloed: false,
            arm: false,
            clips: Vec::new(),
        }
    }

    pub fn bus(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: TrackKind::Bus,
            volume: 1.0,
            pan: 0.0,
            muted: false,
            soloed: false,
            arm: false,
            clips: Vec::new(),
        }
    }

    /// Add a clip and return its index.
    pub fn add_clip(&mut self, clip: AudioClip) -> usize {
        let idx = self.clips.len();
        self.clips.push(clip);
        idx
    }

    /// Split a clip at `split_point` (absolute timeline position).
    /// The original clip is replaced with two clips. Returns an error if
    /// `split_point` is not inside the clip.
    pub fn split_clip(&mut self, clip_idx: usize, split_point: u64) -> Result<()> {
        let clip = self
            .clips
            .get(clip_idx)
            .context("Clip index out of range")?;

        let start = clip.timeline_start;
        let end = clip.timeline_end();

        if split_point <= start || split_point >= end {
            anyhow::bail!("Split point {} is outside clip [{}, {})", split_point, start, end);
        }

        let mut left = clip.clone();
        let mut right = clip.clone();

        let split_offset = split_point - start;
        left.length = Some(split_offset);

        right.timeline_start = split_point;
        right.source_offset = clip.source_offset + split_offset;
        right.length = clip.length.map(|l| l - split_offset);

        self.clips.remove(clip_idx);
        self.clips.insert(clip_idx, right);
        self.clips.insert(clip_idx, left);

        Ok(())
    }

    /// Total length of this track: the end sample of the last clip.
    pub fn length_samples(&self) -> u64 {
        self.clips.iter().map(|c| c.timeline_end()).max().unwrap_or(0)
    }

    /// Find which clips overlap the given timeline range `[start, start+len)`.
    pub fn clips_in_range(&self, start: u64, len: u64) -> impl Iterator<Item = &AudioClip> {
        let end = start + len;
        self.clips
            .iter()
            .filter(move |c| !c.muted && c.timeline_start < end && c.timeline_end() > start)
    }

    /// Per-channel (L/R) volume scalars derived from `volume` and `pan`
    /// using the constant-power pan law.
    #[inline]
    pub fn stereo_gains(&self) -> (f32, f32) {
        let pan = self.pan.clamp(-1.0, 1.0);
        let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4; // 0..PI/2
        let left = angle.cos() * self.volume;
        let right = angle.sin() * self.volume;
        (left, right)
    }
}
