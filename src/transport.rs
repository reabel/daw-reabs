use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

/// Transport playback state shared between the UI thread and the audio thread.
///
/// All fields are atomics so the audio callback can read them without locking.
#[derive(Debug)]
pub struct Transport {
    /// Current playhead position in samples.
    position: Arc<AtomicU64>,
    /// Whether playback is active.
    playing: Arc<AtomicBool>,
    /// Whether recording is active.
    recording: Arc<AtomicBool>,
    /// Whether the transport should loop.
    looping: Arc<AtomicBool>,
    /// Loop start in samples.
    loop_start: Arc<AtomicU64>,
    /// Loop end in samples.
    loop_end: Arc<AtomicU64>,
    /// Metronome enabled.
    metronome: Arc<AtomicBool>,
}

/// A cheap handle to `Transport` that the audio thread owns.
#[derive(Clone, Debug)]
pub struct TransportHandle {
    position: Arc<AtomicU64>,
    playing: Arc<AtomicBool>,
    recording: Arc<AtomicBool>,
    looping: Arc<AtomicBool>,
    loop_start: Arc<AtomicU64>,
    loop_end: Arc<AtomicU64>,
    metronome: Arc<AtomicBool>,
}

impl Transport {
    pub fn new() -> (Self, TransportHandle) {
        let position = Arc::new(AtomicU64::new(0));
        let playing = Arc::new(AtomicBool::new(false));
        let recording = Arc::new(AtomicBool::new(false));
        let looping = Arc::new(AtomicBool::new(false));
        let loop_start = Arc::new(AtomicU64::new(0));
        let loop_end = Arc::new(AtomicU64::new(0));
        let metronome = Arc::new(AtomicBool::new(false));

        let handle = TransportHandle {
            position: Arc::clone(&position),
            playing: Arc::clone(&playing),
            recording: Arc::clone(&recording),
            looping: Arc::clone(&looping),
            loop_start: Arc::clone(&loop_start),
            loop_end: Arc::clone(&loop_end),
            metronome: Arc::clone(&metronome),
        };

        let transport = Self {
            position,
            playing,
            recording,
            looping,
            loop_start,
            loop_end,
            metronome,
        };

        (transport, handle)
    }

    // --- Control API (called from UI / non-RT thread) ---

    pub fn play(&self) {
        self.playing.store(true, Ordering::Release);
    }

    pub fn stop(&self) {
        self.playing.store(false, Ordering::Release);
        self.recording.store(false, Ordering::Release);
    }

    pub fn record(&self) {
        self.recording.store(true, Ordering::Release);
        self.playing.store(true, Ordering::Release);
    }

    pub fn rewind(&self) {
        self.position.store(0, Ordering::Release);
    }

    pub fn seek(&self, sample: u64) {
        self.position.store(sample, Ordering::Release);
    }

    pub fn set_loop(&self, start: u64, end: u64, enabled: bool) {
        self.loop_start.store(start, Ordering::Release);
        self.loop_end.store(end, Ordering::Release);
        self.looping.store(enabled, Ordering::Release);
    }

    pub fn set_metronome(&self, enabled: bool) {
        self.metronome.store(enabled, Ordering::Release);
    }

    // --- State query ---

    pub fn position(&self) -> u64 {
        self.position.load(Ordering::Acquire)
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Acquire)
    }

    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Acquire)
    }

    pub fn is_looping(&self) -> bool {
        self.looping.load(Ordering::Acquire)
    }

    pub fn metronome_enabled(&self) -> bool {
        self.metronome.load(Ordering::Acquire)
    }
}

impl Default for Transport {
    fn default() -> Self {
        Transport::new().0
    }
}

impl TransportHandle {
    /// Called from the audio thread each block to advance the playhead and
    /// handle looping. Returns the position *before* advancing.
    #[inline]
    pub fn advance(&self, frames: u64) -> u64 {
        if !self.playing.load(Ordering::Acquire) {
            return self.position.load(Ordering::Acquire);
        }

        let pos = self.position.fetch_add(frames, Ordering::AcqRel);

        // Handle loop wrap-around.
        if self.looping.load(Ordering::Acquire) {
            let end = self.loop_end.load(Ordering::Acquire);
            let start = self.loop_start.load(Ordering::Acquire);
            if end > start && pos + frames >= end {
                self.position.store(start + (pos + frames - end), Ordering::Release);
            }
        }

        pos
    }

    #[inline]
    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Acquire)
    }

    #[inline]
    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Acquire)
    }

    #[inline]
    pub fn position(&self) -> u64 {
        self.position.load(Ordering::Acquire)
    }

    #[inline]
    pub fn metronome_enabled(&self) -> bool {
        self.metronome.load(Ordering::Acquire)
    }
}
