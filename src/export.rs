use anyhow::{Context, Result};
use hound::{WavSpec, WavWriter};
use std::path::Path;

use crate::project::Project;

/// Export options for a bounce/render.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// Output sample rate (often matches project sample rate).
    pub sample_rate: u32,
    /// Bit depth: 16 or 24 (integer PCM) or 32 (float).
    pub bit_depth: BitDepth,
    /// Number of output channels (1 = mono, 2 = stereo).
    pub channels: u16,
    /// Optional range to export; `None` = entire project.
    pub range: Option<(u64, u64)>,
    /// Apply a final ceiling (peak normalization) if `Some(db)`.
    pub normalize_peak_db: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitDepth {
    Int16,
    Int24,
    Float32,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            sample_rate: 44100,
            bit_depth: BitDepth::Int24,
            channels: 2,
            range: None,
            normalize_peak_db: None,
        }
    }
}

/// Offline stereo bounce of `project` to a WAV file at `output_path`.
///
/// This is a simple offline render that mixes all un-muted audio clips
/// in sample-accurate fashion. It does not run through the real-time
/// audio callback so it is safe to call from any thread.
pub fn export_wav(project: &Project, output_path: &Path, opts: &ExportOptions) -> Result<()> {
    let total_len = project.length_samples();
    if total_len == 0 {
        anyhow::bail!("Project has no audio content to export");
    }

    let (start, end) = opts.range.unwrap_or((0, total_len));
    if start >= end {
        anyhow::bail!("Export range is empty or inverted");
    }
    let export_len = (end - start) as usize;

    // Allocate interleaved stereo output buffer.
    let channels = opts.channels as usize;
    let mut mix: Vec<f32> = vec![0.0f32; export_len * channels];

    // Mix all tracks.
    let any_solo = project.tracks.iter().any(|t| t.soloed);

    for track in &project.tracks {
        if track.muted {
            continue;
        }
        if any_solo && !track.soloed {
            continue;
        }

        let (gain_l, gain_r) = track.stereo_gains();

        for clip in &track.clips {
            if clip.muted {
                continue;
            }
            if clip.timeline_start >= end || clip.timeline_end() <= start {
                continue;
            }

            let src_samples = load_wav_to_f32_inner(&clip.source_path)
                .with_context(|| format!("Failed to load clip: {}", clip.source_path.display()))?;

            let src_ch = hound::WavReader::open(&clip.source_path)
                .map(|r| r.spec().channels as usize)
                .unwrap_or(1);

            let clip_start_in_export =
                clip.timeline_start.saturating_sub(start) as usize;
            let clip_end_in_export =
                (clip.timeline_end().min(end) - start) as usize;

            for out_frame in clip_start_in_export..clip_end_in_export {
                let tl_pos = start + out_frame as u64;
                let clip_pos = tl_pos - clip.timeline_start;
                let src_pos = clip.source_offset + clip_pos;
                let env = clip.envelope_at(clip_pos);

                let left = src_samples
                    .get((src_pos as usize) * src_ch)
                    .copied()
                    .unwrap_or(0.0)
                    * env;
                let right = if src_ch > 1 {
                    src_samples
                        .get((src_pos as usize) * src_ch + 1)
                        .copied()
                        .unwrap_or(0.0)
                        * env
                } else {
                    left
                };

                let base = out_frame * channels;
                mix[base] += left * gain_l;
                if channels > 1 {
                    mix[base + 1] += right * gain_r;
                }
            }
        }
    }

    // Optional peak normalization.
    if let Some(target_db) = opts.normalize_peak_db {
        let peak = mix.iter().copied().fold(0.0f32, |a, s| a.max(s.abs()));
        if peak > 1e-9 {
            let target_linear = 10.0f32.powf(target_db / 20.0);
            let scale = target_linear / peak;
            for s in &mut mix {
                *s *= scale;
            }
        }
    }

    // Write WAV.
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create output directory: {}", parent.display()))?;
    }

    let (sample_format, bits) = match opts.bit_depth {
        BitDepth::Int16 => (hound::SampleFormat::Int, 16),
        BitDepth::Int24 => (hound::SampleFormat::Int, 24),
        BitDepth::Float32 => (hound::SampleFormat::Float, 32),
    };

    let spec = WavSpec {
        channels: opts.channels,
        sample_rate: opts.sample_rate,
        bits_per_sample: bits,
        sample_format,
    };

    let mut writer = WavWriter::create(output_path, spec)
        .with_context(|| format!("Failed to create output file: {}", output_path.display()))?;

    match opts.bit_depth {
        BitDepth::Float32 => {
            for &s in &mix {
                writer.write_sample(s)?;
            }
        }
        BitDepth::Int16 => {
            for &s in &mix {
                let int = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                writer.write_sample(int)?;
            }
        }
        BitDepth::Int24 => {
            let max = (1i32 << 23) as f32;
            for &s in &mix {
                let int = (s.clamp(-1.0, 1.0) * max) as i32;
                writer.write_sample(int)?;
            }
        }
    }

    writer.finalize().context("Failed to finalize WAV file")?;

    Ok(())
}

/// Export each track as a separate WAV file (stem export).
pub fn export_stems(
    project: &Project,
    output_dir: &Path,
    opts: &ExportOptions,
) -> Result<Vec<std::path::PathBuf>> {
    let mut paths = Vec::new();
    for (i, track) in project.tracks.iter().enumerate() {
        let file_name = format!(
            "{:02}_{}.wav",
            i + 1,
            sanitize_name(&track.name)
        );
        let out = output_dir.join(&file_name);

        // Build a temporary single-track project for export.
        let mut stem_proj = project.clone();
        stem_proj.tracks = vec![track.clone()];

        export_wav(&stem_proj, &out, opts)?;
        paths.push(out);
    }
    Ok(paths)
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// Load a WAV file into a flat `Vec<f32>` (interleaved channels, normalised to ±1).
pub(crate) fn load_wav_to_f32_inner(path: &std::path::Path) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("Failed to open WAV: {}", path.display()))?;
    let spec = reader.spec();

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|s| s.map_err(anyhow::Error::from))
            .collect::<Result<Vec<_>>>()?,
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max).map_err(anyhow::Error::from))
                .collect::<Result<Vec<_>>>()?
        }
    };

    Ok(samples)
}
