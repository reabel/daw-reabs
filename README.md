# daw-reabs

A Rust-based digital audio workstation (DAW) with a terminal UI, offline export, and optional real-time audio playback.

## Design Documentation

- [DAW Feature Design Document](docs/daw-feature-design.md)

---

## Building

**Prerequisites:** [Rust toolchain](https://rustup.rs)

```sh
cargo build
```

To enable real-time audio playback (Linux requires ALSA dev headers):

```sh
sudo apt install pkg-config libasound2-dev
cargo build --features audio
```

---

## CLI Usage

### Create a new project

```sh
./daw-reabs new -n "My Session" -o session.dawproj --bpm 140 --sample-rate 44100
```

### Add an audio track

```sh
./daw-reabs add-track -p session.dawproj -n "Drums" -f drums.wav --start 0 --gain 1.0
```

### Open the interactive TUI

```sh
./daw-reabs tui --project session.dawproj
```

### Export to WAV

```sh
# Stereo mixdown
./daw-reabs export -p session.dawproj -o mix.wav --bit-depth 24 --normalize -1.0

# Stem export (one file per track)
./daw-reabs export -p session.dawproj -o stems/ --stems
```

### Print project info

```sh
./daw-reabs info -p session.dawproj
```

---

## TUI Key Bindings

| Key | Action |
|-----|--------|
| `Space` | Play / Stop |
| `r` | Record |
| `Home` | Rewind to start |
| `↑` / `↓` or `k` / `j` | Select track up / down |
| `m` | Mute selected track |
| `s` | Solo selected track |
| `a` | Arm / disarm track |
| `+` / `-` | Volume up / down |
| `[` / `]` | Pan left / right |
| `←` / `→` or `h` / `l` | Scroll timeline |
| `i` / `o` | Zoom in / out |
| `?` | Help overlay |
| `q` | Quit |

---

## Project Structure

| File | Description |
|------|-------------|
| `src/project.rs` | Create, open, and save `.dawproj` (JSON) project files |
| `src/track.rs` | `Track` and `AudioClip` model (volume, pan, mute, solo, fades, split) |
| `src/transport.rs` | Lock-free transport (play/stop/record/seek/loop) safe for audio threads |
| `src/engine.rs` | Real-time audio engine via `cpal` (`--features audio`) |
| `src/export.rs` | Offline stereo bounce and stem export (16/24-bit int, 32-bit float WAV) |
| `src/ui.rs` | `ratatui` terminal UI — transport bar, track list, timeline |
| `src/main.rs` | `clap` CLI entry point |

---

## TUI Example

```bash
┌─ My Session ──────────────────────────────────────────────────────────────────┐
│  ■ STOP   00:00.00 / 00:32.10  ████████████░░░░░░░░░░  140.0 BPM  4/4       │
└───────────────────────────────────────────────────────────────────────────────┘
┌ Tracks ──────────────┐┌ Timeline (i/o zoom  ◀▶ scroll) ──────────────────────┐
│ Track  Vol  Pan  Flg ││ 00:00    00:08    00:16    00:24                      │
│ Drums  100%   C   ●  ││ ╔═Drums══════╗                                       │
│ Bass    80%  R12     ││ ╔════Bass╗                                            │
└──────────────────────┘└───────────────────────────────────────────────────────┘
 Track: Drums  Vol: 100%  Pan: 0.00  Zoom: 1s/col   [?] Help  [q] Quit ...
```

## Implementation Phases

1. **MVP** ✅ — transport, audio tracks, recording model, save/load, stereo export, TUI
2. **Composition** — MIDI tracks, piano roll, virtual instruments, tempo map
3. **Mixing** — busses/sends, automation, plugin delay compensation, metering
4. **Advanced** — comping, advanced routing, external sync, accessibility
