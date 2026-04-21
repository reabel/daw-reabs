use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[cfg(feature = "audio")]
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

mod export;
mod project;
mod track;
mod transport;
mod ui;

#[cfg(feature = "audio")]
mod engine;

use export::{BitDepth, ExportOptions};
use project::Project;
use track::{AudioClip, Track};
#[cfg(feature = "audio")]
use transport::Transport;

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "daw-reabs", version, about = "A Rust-based DAW (MVP)")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Open the interactive TUI for a project.
    Tui {
        /// Path to the `.dawproj` file to open.
        #[arg(short, long)]
        project: PathBuf,
    },

    /// Create a new blank project file.
    New {
        /// Human-readable project name.
        #[arg(short, long)]
        name: String,
        /// Output file path (e.g. my_session.dawproj).
        #[arg(short, long)]
        output: PathBuf,
        /// Sample rate in Hz.
        #[arg(long, default_value_t = 44100)]
        sample_rate: u32,
        /// Tempo in BPM.
        #[arg(long, default_value_t = 120.0)]
        bpm: f64,
    },

    /// Add an audio track (with a WAV clip) to an existing project.
    AddTrack {
        /// Path to the `.dawproj` file.
        #[arg(short, long)]
        project: PathBuf,
        /// Track display name.
        #[arg(short, long)]
        name: String,
        /// Path to the WAV source file.
        #[arg(short = 'f', long)]
        file: PathBuf,
        /// Timeline start position in samples.
        #[arg(long, default_value_t = 0)]
        start: u64,
        /// Clip gain (linear, 1.0 = unity).
        #[arg(long, default_value_t = 1.0)]
        gain: f32,
    },

    /// Play a project through the default audio device.
    Play {
        /// Path to the `.dawproj` file.
        #[arg(short, long)]
        project: PathBuf,
        /// Loop playback.
        #[arg(long)]
        r#loop: bool,
        /// Playback duration in seconds (0 = play to end).
        #[arg(long, default_value_t = 0.0)]
        duration: f64,
    },

    /// Offline bounce/export of a project to a WAV file.
    Export {
        /// Path to the `.dawproj` file.
        #[arg(short, long)]
        project: PathBuf,
        /// Output WAV file path.
        #[arg(short, long)]
        output: PathBuf,
        /// Sample rate for the output file.
        #[arg(long, default_value_t = 44100)]
        sample_rate: u32,
        /// Bit depth: 16, 24, or 32 (float).
        #[arg(long, default_value_t = 24)]
        bit_depth: u8,
        /// Normalize to peak dBFS (e.g. -1.0). Omit to skip.
        #[arg(long)]
        normalize: Option<f32>,
        /// Export individual stems into a directory instead.
        #[arg(long)]
        stems: bool,
    },

    /// Print project information.
    Info {
        /// Path to the `.dawproj` file.
        #[arg(short, long)]
        project: PathBuf,
    },
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::New {
            name,
            output,
            sample_rate,
            bpm,
        } => {
            let mut project = Project::new(name, sample_rate);
            project.bpm = bpm;
            let saved = project.save(Some(&output))?;
            println!("Created project: {}", saved.display());
        }

        Command::AddTrack {
            project: proj_path,
            name,
            file,
            start,
            gain,
        } => {
            let mut project = Project::open(&proj_path)?;

            let mut clip = AudioClip::new(&name, file.clone(), start);
            clip.gain = gain;

            let mut track = Track::audio(&name);
            track.add_clip(clip);
            let idx = project.add_track(track);

            project.save(Option::<PathBuf>::None)?;
            println!(
                "Added track #{} '{}' with clip '{}' at sample {}",
                idx,
                name,
                file.display(),
                start
            );
        }

        Command::Play {
            project: proj_path,
            r#loop,
            duration,
        } => {
            let project = Project::open(&proj_path)?;
            let total_len = project.length_samples();
            let sample_rate = project.sample_rate;
            println!(
                "Playing '{}' ({:.2}s, {}Hz, {} tracks)",
                project.name,
                project.length_seconds(),
                sample_rate,
                project.tracks.len()
            );

            #[cfg(not(feature = "audio"))]
            {
                eprintln!(
                    "Real-time audio is not enabled in this build.\n\
                     Rebuild with `cargo build --features audio` after installing ALSA dev headers:\n\
                     \n  sudo apt install pkg-config libasound2-dev\n"
                );
                return Ok(());
            }

            #[cfg(feature = "audio")]
            {
                let shared_proj = Arc::new(Mutex::new(project));
                let (transport, handle) = Transport::new();

                if r#loop && total_len > 0 {
                    transport.set_loop(0, total_len, true);
                }

                let _engine = engine::AudioEngine::start(handle, Arc::clone(&shared_proj))?;

                transport.play();

                let play_dur = if duration > 0.0 {
                    Duration::from_secs_f64(duration)
                } else if total_len > 0 {
                    Duration::from_secs_f64(total_len as f64 / sample_rate as f64 + 0.5)
                } else {
                    Duration::from_secs(5)
                };

                std::thread::sleep(play_dur);
                transport.stop();
                println!("Done.");
            }
        }

        Command::Export {
            project: proj_path,
            output,
            sample_rate,
            bit_depth,
            normalize,
            stems,
        } => {
            let project = Project::open(&proj_path)?;

            let bd = match bit_depth {
                16 => BitDepth::Int16,
                24 => BitDepth::Int24,
                32 => BitDepth::Float32,
                other => anyhow::bail!("Unsupported bit depth: {}", other),
            };

            let opts = ExportOptions {
                sample_rate,
                bit_depth: bd,
                channels: 2,
                range: None,
                normalize_peak_db: normalize,
            };

            if stems {
                let paths = export::export_stems(&project, &output, &opts)?;
                println!("Exported {} stems to {}", paths.len(), output.display());
                for p in paths {
                    println!("  {}", p.display());
                }
            } else {
                export::export_wav(&project, &output, &opts)?;
                println!("Exported: {}", output.display());
            }
        }

        Command::Info { project: proj_path } => {
            let project = Project::open(&proj_path)?;
            println!("Project : {}", project.name);
            println!("BPM     : {}", project.bpm);
            println!(
                "Time sig: {}/{}",
                project.time_sig_numerator, project.time_sig_denominator
            );
            println!("Rate    : {} Hz", project.sample_rate);
            println!("Length  : {:.3}s ({} samples)", project.length_seconds(), project.length_samples());
            println!("Tracks  : {}", project.tracks.len());
            for (i, t) in project.tracks.iter().enumerate() {
                println!(
                    "  [{i}] '{}' | vol={:.2} pan={:.2} muted={} solo={} clips={}",
                    t.name, t.volume, t.pan, t.muted, t.soloed,
                    t.clips.len()
                );
                for (j, c) in t.clips.iter().enumerate() {
                    println!(
                        "       clip[{j}] '{}' start={} len={:?} gain={:.2} muted={}",
                        c.name, c.timeline_start, c.length, c.gain, c.muted
                    );
                }
            }
        }

        Command::Tui { project: proj_path } => {
            let project = Project::open(&proj_path)?;
            ui::run(project)?;
        }
    }

    Ok(())
}
