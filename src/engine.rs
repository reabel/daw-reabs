#![cfg(feature = "audio")]

use anyhow::{Context, Result};
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, Host, SampleFormat, Stream, StreamConfig,
};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::{export::load_wav_to_f32_inner, project::Project, transport::TransportHandle};

/// Commands the UI thread sends to the audio engine.
#[derive(Debug)]
pub enum EngineCommand {
    /// Replace the project being rendered (e.g. after a track is added).
    LoadProject(Box<Project>),
    Shutdown,
}

/// Real-time audio engine backed by cpal.
pub struct AudioEngine {
    _stream: Stream,
    pub cmd_tx: Sender<EngineCommand>,
}

impl AudioEngine {
    /// Open the default output device and start the audio stream.
    pub fn start(
        transport: TransportHandle,
        project: Arc<Mutex<Project>>,
    ) -> Result<Self> {
        let host: Host = cpal::default_host();
        let device: Device = host
            .default_output_device()
            .context("No output device available")?;

        let default_cfg = device
            .default_output_config()
            .context("Failed to get default output config")?;

        let sample_rate = default_cfg.sample_rate().0;
        let channels = default_cfg.channels() as usize;

        let config = StreamConfig {
            channels: default_cfg.channels(),
            sample_rate: default_cfg.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        };

        let (cmd_tx, cmd_rx) = bounded::<EngineCommand>(64);

        let stream = match default_cfg.sample_format() {
            SampleFormat::F32 => build_stream::<f32>(
                &device,
                &config,
                transport,
                Arc::clone(&project),
                cmd_rx,
                sample_rate,
                channels,
            )?,
            SampleFormat::I16 => build_stream::<i16>(
                &device,
                &config,
                transport,
                Arc::clone(&project),
                cmd_rx,
                sample_rate,
                channels,
            )?,
            SampleFormat::U16 => build_stream::<u16>(
                &device,
                &config,
                transport,
                Arc::clone(&project),
                cmd_rx,
                sample_rate,
                channels,
            )?,
            fmt => anyhow::bail!("Unsupported sample format: {:?}", fmt),
        };

        stream.play().context("Failed to start audio stream")?;

        Ok(Self {
            _stream: stream,
            cmd_tx,
        })
    }
}

/// Trait alias so we can write the generic callback once.
trait CpalSample: cpal::Sample + cpal::FromSample<f32> {}
impl CpalSample for f32 {}
impl CpalSample for i16 {}
impl CpalSample for u16 {}

fn build_stream<S: CpalSample + Send + 'static>(
    device: &Device,
    config: &StreamConfig,
    transport: TransportHandle,
    project: Arc<Mutex<Project>>,
    cmd_rx: Receiver<EngineCommand>,
    _sample_rate: u32,
    channels: usize,
) -> Result<Stream> {
    // Per-clip sample caches (clip_idx -> interleaved f32 samples).
    // We pre-load clips once; a real DAW would stream from disk.
    let clip_cache: Arc<Mutex<Vec<Vec<f32>>>> = Arc::new(Mutex::new(Vec::new()));

    // Pre-load audio data for each clip in the project.
    {
        let proj = project.lock().unwrap();
        let mut cache = clip_cache.lock().unwrap();
        for track in &proj.tracks {
            for clip in &track.clips {
                let samples = load_wav_to_f32(&clip.source_path).unwrap_or_default();
                cache.push(samples);
            }
        }
    }

    let err_fn = |err| eprintln!("[AudioEngine] stream error: {err}");

    let mut global_clip_idx = 0usize;
    let clip_cache_cb = Arc::clone(&clip_cache);
    let project_cb = Arc::clone(&project);

    let stream = device.build_output_stream(
        config,
        move |output: &mut [S], _info: &cpal::OutputCallbackInfo| {
            // Drain any pending commands (non-blocking).
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    EngineCommand::LoadProject(new_proj) => {
                        // Reload clip cache.
                        let mut cache = clip_cache_cb.lock().unwrap();
                        cache.clear();
                        global_clip_idx = 0;
                        for track in &new_proj.tracks {
                            for clip in &track.clips {
                                let s = load_wav_to_f32(&clip.source_path).unwrap_or_default();
                                cache.push(s);
                            }
                        }
                        *project_cb.lock().unwrap() = *new_proj;
                    }
                    EngineCommand::Shutdown => {}
                }
            }

            let frames = output.len() / channels;
            let pos = transport.advance(frames as u64);

            if !transport.is_playing() {
                for s in output.iter_mut() {
                    *s = S::from_sample(0.0f32);
                }
                return;
            }

            // Mix all active clips into a stereo f32 buffer, then write to output.
            let mut mix = vec![0.0f32; output.len()];

            let proj = project_cb.lock().unwrap();
            let cache = clip_cache_cb.lock().unwrap();

            let mut clip_offset = 0usize;
            for track in &proj.tracks {
                if track.muted {
                    clip_offset += track.clips.len();
                    continue;
                }
                let (gain_l, gain_r) = track.stereo_gains();

                for clip in &track.clips {
                    if clip_offset >= cache.len() {
                        break;
                    }
                    let samples = &cache[clip_offset];
                    clip_offset += 1;

                    if clip.muted {
                        continue;
                    }

                    let clip_end = clip.timeline_end();
                    if pos >= clip_end || pos + frames as u64 <= clip.timeline_start {
                        continue;
                    }

                    // Source file channels (assume mono=1, stereo=2).
                    let src_ch = {
                        if let Ok(r) = hound::WavReader::open(&clip.source_path) {
                            r.spec().channels as usize
                        } else {
                            1
                        }
                    };

                    for frame in 0..frames {
                        let tl_pos = pos + frame as u64;
                        if tl_pos < clip.timeline_start || tl_pos >= clip_end {
                            continue;
                        }
                        let clip_pos = tl_pos - clip.timeline_start;
                        let src_pos = clip.source_offset + clip_pos;
                        let env = clip.envelope_at(clip_pos);

                        // Read source samples (interleaved).
                        let left = samples
                            .get((src_pos as usize) * src_ch)
                            .copied()
                            .unwrap_or(0.0)
                            * env;
                        let right = if src_ch > 1 {
                            samples
                                .get((src_pos as usize) * src_ch + 1)
                                .copied()
                                .unwrap_or(0.0)
                                * env
                        } else {
                            left
                        };

                        let out_base = frame * channels;
                        mix[out_base] += left * gain_l;
                        if channels > 1 {
                            mix[out_base + 1] += right * gain_r;
                        }
                    }
                }
            }

            // Write mixed f32 buffer into the typed output slice.
            for (o, m) in output.iter_mut().zip(mix.iter()) {
                *o = S::from_sample(*m);
            }
        },
        err_fn,
        None,
    )?;

    Ok(stream)
}

/// Load a WAV file into a flat `Vec<f32>` (interleaved channels, normalised to ±1).
/// Delegates to `export::load_wav_to_f32_inner`.
pub fn load_wav_to_f32(path: &std::path::Path) -> Result<Vec<f32>> {
    load_wav_to_f32_inner(path)
}
