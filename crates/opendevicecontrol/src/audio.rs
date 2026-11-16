//! What the computer is playing, for sound-reactive lighting.
//!
//! A background thread records the default output (WASAPI loopback on
//! Windows; the default sink's monitor through the PulseAudio API on Linux,
//! which PipeWire serves) and turns it into frequency bands, a loudness level
//! and beats. It runs only while some device shows a sound effect: readers
//! call [`Audio::frame`], and the thread stops a few seconds after the last
//! read.

use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Frequency bands, low to high (log-spaced, about 60 Hz–16 kHz).
pub const BANDS: usize = 16;
/// Level history kept for scrolling effects (newest first).
pub const HISTORY: usize = 64;
/// Beats remembered for ripple-style effects.
const BEATS: usize = 8;
const FFT: usize = 2048;
/// Samples between analyses (~11 ms at 48 kHz).
const HOP: usize = 512;
/// The capture thread stops when nobody read a frame for this long.
const IDLE_STOP: Duration = Duration::from_secs(5);

/// One analysis snapshot. Every value is 0..1.
#[derive(Debug, Clone)]
pub struct AudioFrame {
    pub bands: [f32; BANDS],
    /// Overall loudness.
    pub level: f32,
    /// Level history, newest first, one entry per ~30 ms.
    pub history: Vec<f32>,
    /// Seconds since each recent beat (newest first) and its strength.
    pub beats: Vec<(f32, f32)>,
    /// Beats seen so far (changes on every beat).
    pub beat_count: u64,
    /// Why there is no sound data, if capture failed.
    pub error: Option<String>,
    /// Seconds since capture started (drives sweeping effects).
    pub time: f32,
    /// Musical notes being heard, loudest first (see [`crate::notes`]).
    pub notes: Vec<crate::notes::Note>,
}

impl Default for AudioFrame {
    fn default() -> Self {
        Self {
            bands: [0.0; BANDS],
            level: 0.0,
            history: vec![0.0; HISTORY],
            beats: Vec::new(),
            beat_count: 0,
            error: None,
            time: 0.0,
            notes: Vec::new(),
        }
    }
}

impl AudioFrame {
    /// Strength of the most recent beat, fading out over `fade` seconds.
    pub fn pulse(&self, fade: f32) -> f32 {
        self.beats.first().map(|&(age, s)| s * (1.0 - age / fade.max(0.01)).max(0.0)).unwrap_or(0.0)
    }

    /// Band level at `x` in 0..1 (low to high), interpolated.
    pub fn band_at(&self, x: f32) -> f32 {
        let f = x.clamp(0.0, 1.0) * (BANDS - 1) as f32;
        let i = f.floor() as usize;
        let j = (i + 1).min(BANDS - 1);
        let t = f - i as f32;
        self.bands[i] * (1.0 - t) + self.bands[j] * t
    }
}

static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Shared handle; cheap to clone.
#[derive(Clone, Default)]
pub struct Audio(Arc<Inner>);

#[derive(Default)]
struct Inner {
    state: Mutex<State>,
    running: AtomicBool,
}

#[derive(Default)]
struct State {
    frame: AudioFrame,
    beat_times: VecDeque<(Instant, f32)>,
    /// Recent analyses (newest first), for [`Audio::frame_peak`].
    recent: VecDeque<(Instant, [f32; BANDS], f32)>,
    last_read: Option<Instant>,
}

/// How much analysis history [`Audio::frame_peak`] can look back over.
const RECENT: Duration = Duration::from_secs(1);

impl Audio {
    /// The latest analysis; starts capturing if it isn't running.
    pub fn frame(&self) -> AudioFrame {
        let mut s = self.0.state.lock();
        s.last_read = Some(Instant::now());
        let mut f = s.frame.clone();
        f.beats = s.beat_times.iter().map(|(t, st)| (t.elapsed().as_secs_f32(), *st)).collect();
        f.time = START.get_or_init(Instant::now).elapsed().as_secs_f32();
        drop(s);
        if !self.0.running.swap(true, Ordering::SeqCst) {
            let me = self.clone();
            if let Err(e) = std::thread::Builder::new().name("audio".into()).spawn(move || me.run()) {
                log::warn!("audio thread: {e}");
                // Let the next read try again.
                self.0.running.store(false, Ordering::SeqCst);
            }
        }
        f
    }

    /// Like [`Audio::frame`], with bands and level at their peak over the
    /// last `window`. For devices that take a frame only every so often:
    /// a peak between two frames still shows in the next one.
    pub fn frame_peak(&self, window: Duration) -> AudioFrame {
        let mut f = self.frame();
        apply_peak(&mut f, &self.0.state.lock().recent, window, Instant::now());
        f
    }

    fn idle(&self) -> bool {
        self.0.state.lock().last_read.is_none_or(|t| t.elapsed() > IDLE_STOP)
    }

    fn run(&self) {
        log::info!("sound-reactive lighting: capturing the default output");
        while !self.idle() {
            let mut an = Analyzer::default();
            let res = capture::run(&mut |samples, rate| {
                if let Some((frame, beat)) = an.push(samples, rate) {
                    let mut s = self.0.state.lock();
                    let count = s.frame.beat_count + beat.is_some() as u64;
                    let now = Instant::now();
                    s.recent.push_front((now, frame.bands, frame.level));
                    while s.recent.back().is_some_and(|(t, ..)| now.duration_since(*t) > RECENT) {
                        s.recent.pop_back();
                    }
                    s.frame = AudioFrame { beat_count: count, error: None, ..frame };
                    if let Some(strength) = beat {
                        s.beat_times.push_front((Instant::now(), strength));
                        s.beat_times.truncate(BEATS);
                    }
                }
                !self.idle()
            });
            match res {
                Ok(()) => {}
                Err(e) => {
                    log::warn!("sound capture: {e}");
                    self.0.state.lock().frame = AudioFrame { error: Some(e), ..AudioFrame::default() };
                    // Retry later (e.g. the output device changed or the
                    // sound server restarted).
                    let until = Instant::now() + Duration::from_secs(3);
                    while Instant::now() < until && !self.idle() {
                        std::thread::sleep(Duration::from_millis(200));
                    }
                }
            }
        }
        self.0.state.lock().frame = AudioFrame::default();
        self.0.running.store(false, Ordering::SeqCst);
        log::info!("sound-reactive lighting: capture stopped");
    }
}

/// Raise `f`'s bands and level to their peak over the analyses in `recent`
/// (newest first) from the last `window` before `now`.
fn apply_peak(f: &mut AudioFrame, recent: &VecDeque<(Instant, [f32; BANDS], f32)>, window: Duration, now: Instant) {
    for (_, bands, level) in recent.iter().take_while(|(t, ..)| now.saturating_duration_since(*t) <= window) {
        for (a, b) in f.bands.iter_mut().zip(bands) {
            *a = a.max(*b);
        }
        f.level = f.level.max(*level);
    }
}

/// Turns mono samples into bands, level and beats.
pub struct Analyzer {
    buf: Vec<f32>,
    /// Smoothed band values (before display decay).
    bands: [f32; BANDS],
    level: f32,
    /// Slowly decaying peak of the raw loudness, for automatic gain.
    agc: f32,
    /// Running average of bass energy, for beat detection.
    bass_avg: f32,
    since_beat: f32,
    history: VecDeque<f32>,
    since_history: f32,
    /// Created for the sample rate on the first samples.
    notes: Option<crate::notes::NoteFinder>,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self {
            buf: Vec::with_capacity(FFT * 2),
            bands: [0.0; BANDS],
            level: 0.0,
            agc: 1e-3,
            bass_avg: 0.0,
            since_beat: 1.0,
            history: VecDeque::from(vec![0.0; HISTORY]),
            since_history: 0.0,
            notes: None,
        }
    }
}

/// Edges of the log-spaced bands in Hz.
fn band_edges() -> [f32; BANDS + 1] {
    let (lo, hi) = (60.0f32, 16000.0f32);
    std::array::from_fn(|i| lo * (hi / lo).powf(i as f32 / BANDS as f32))
}

impl Analyzer {
    /// Feed mono samples; returns a new frame (and a beat strength when a
    /// beat started) each time a full window has been analysed.
    pub fn push(&mut self, samples: &[f32], rate: u32) -> Option<(AudioFrame, Option<f32>)> {
        self.buf.extend_from_slice(samples);
        if self.notes.as_ref().is_none_or(|n| n.rate() != rate.max(8000)) {
            self.notes = Some(crate::notes::NoteFinder::new(rate));
        }
        if let Some(n) = &mut self.notes {
            n.push(samples);
        }
        let mut out = None;
        while self.buf.len() >= FFT {
            let window: Vec<f32> = self.buf[..FFT].to_vec();
            let r = self.analyse(&window, rate);
            out = Some(match (out, r) {
                (Some((_, Some(b))), (f, None)) => (f, Some(b)), // keep a beat seen earlier in this batch
                (_, r) => r,
            });
            self.buf.drain(..HOP);
        }
        out
    }

    fn analyse(&mut self, window: &[f32], rate: u32) -> (AudioFrame, Option<f32>) {
        let dt = HOP as f32 / rate.max(1) as f32;
        // Hann window, FFT, magnitudes.
        let mut re: Vec<f32> = window
            .iter()
            .enumerate()
            .map(|(i, &s)| s * (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (FFT - 1) as f32).cos()))
            .collect();
        let mut im = vec![0.0f32; FFT];
        fft(&mut re, &mut im);
        let mag: Vec<f32> = (0..FFT / 2).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt() / FFT as f32).collect();
        let hz_per_bin = rate as f32 / FFT as f32;
        let edges = band_edges();
        let mut raw = [0.0f32; BANDS];
        for (b, r) in raw.iter_mut().enumerate() {
            let k0 = ((edges[b] / hz_per_bin) as usize).max(1);
            let k1 = ((edges[b + 1] / hz_per_bin) as usize).clamp(k0 + 1, FFT / 2);
            // Peak bin in the band; higher bands are boosted a little since
            // music has far less energy up there.
            let peak = mag[k0..k1].iter().copied().fold(0.0, f32::max);
            *r = peak * (1.0 + b as f32 * 0.15);
        }
        let loud = raw.iter().copied().fold(0.0, f32::max);
        // Automatic gain: follow peaks quickly, fall back over ~5 s.
        self.agc = (self.agc * (-dt / 5.0).exp()).max(loud).max(1e-4);
        let silent = loud < 2e-4;
        for (b, r) in raw.iter().enumerate() {
            let v = if silent { 0.0 } else { (r / self.agc).clamp(0.0, 1.0).sqrt() };
            // Fast attack, slower release.
            let k = if v > self.bands[b] { 0.6 } else { 1.0 - (-dt / 0.12).exp() };
            self.bands[b] += (v - self.bands[b]) * k;
        }
        let level_now = self.bands.iter().sum::<f32>() / BANDS as f32;
        let k = if level_now > self.level { 0.5 } else { 1.0 - (-dt / 0.2).exp() };
        self.level += (level_now - self.level) * k;

        // Beats: bass well above its recent average.
        let bass = (raw[0] + raw[1] + raw[2]) / self.agc;
        self.since_beat += dt;
        let mut beat = None;
        if !silent && bass > 0.25 && bass > self.bass_avg * 1.45 && self.since_beat > 0.18 {
            beat = Some((bass / (self.bass_avg * 2.0).max(0.3)).clamp(0.3, 1.0));
            self.since_beat = 0.0;
        }
        self.bass_avg += (bass - self.bass_avg) * (1.0 - (-dt / 0.6).exp());

        self.since_history += dt;
        if self.since_history >= 0.03 {
            self.since_history = 0.0;
            self.history.push_front(self.level);
            self.history.truncate(HISTORY);
        }
        let mut notes = self.notes.as_mut().map(|n| n.update(dt).to_vec()).unwrap_or_default();
        notes.sort_by(|a, b| b.amp.total_cmp(&a.amp));
        let frame = AudioFrame {
            notes,
            bands: self.bands,
            level: self.level.clamp(0.0, 1.0),
            history: self.history.iter().copied().collect(),
            ..AudioFrame::default()
        };
        (frame, beat)
    }
}

/// In-place radix-2 FFT (`re.len()` must be a power of two).
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -std::f32::consts::TAU / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                (cr, ci) = (cr * wr - ci * wi, cr * wi + ci * wr);
            }
        }
        len <<= 1;
    }
}

/// WASAPI loopback delivers no packets at all while nothing plays. This
/// turns such a gap into the silence it stands for, so the analysis (and
/// the lights) fade out instead of holding the last spectrum.
#[cfg_attr(not(windows), allow(dead_code))]
struct GapFiller {
    last: Instant,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl GapFiller {
    /// Gaps shorter than this are normal jitter between polls.
    const MIN_GAP: Duration = Duration::from_millis(30);
    /// At most this much silence is added at once (after a stall).
    const MAX_GAP: Duration = Duration::from_secs(1);

    fn new(now: Instant) -> Self {
        Self { last: now }
    }

    /// Call once per poll with the samples it produced (mono, `rate` Hz).
    fn fill(&mut self, mono: &mut Vec<f32>, rate: u32, now: Instant) {
        if !mono.is_empty() {
            self.last = now;
            return;
        }
        let gap = now.saturating_duration_since(self.last);
        if gap >= Self::MIN_GAP {
            let n = (gap.min(Self::MAX_GAP).as_secs_f64() * rate as f64) as usize;
            mono.resize(n, 0.0);
            self.last = now;
        }
    }
}

/// Average interleaved channels into mono.
fn to_mono(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    out.extend(interleaved.chunks_exact(channels.max(1)).map(|c| c.iter().sum::<f32>() / c.len() as f32));
}

#[cfg(windows)]
mod capture {
    use std::collections::VecDeque;
    use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

    const RATE: u32 = 48000;
    const CHANNELS: usize = 2;

    /// Record the default output until `sink` returns false.
    pub fn run(sink: &mut dyn FnMut(&[f32], u32) -> bool) -> Result<(), String> {
        let e = |e: wasapi::WasapiError| e.to_string();
        let _ = wasapi::initialize_mta();
        let device = DeviceEnumerator::new().map_err(e)?.get_default_device(Direction::Render).map_err(e)?;
        let mut client = device.get_iaudioclient().map_err(e)?;
        let format = WaveFormat::new(32, 32, SampleType::Float, RATE as usize, CHANNELS, None);
        // Polling: loopback events don't fire while nothing is playing.
        let mode = StreamMode::PollingShared { autoconvert: true, buffer_duration_hns: 1_000_000 };
        client.initialize_client(&format, Direction::Capture, mode, None).map_err(e)?;
        let capture = client.get_audiocaptureclient().map_err(e)?;
        client.start_stream().map_err(e)?;
        let mut bytes = VecDeque::new();
        let (mut floats, mut mono) = (Vec::new(), Vec::new());
        let mut gaps = super::GapFiller::new(std::time::Instant::now());
        loop {
            std::thread::sleep(std::time::Duration::from_millis(10));
            while capture.get_next_packet_size().map_err(e)?.unwrap_or(0) > 0 {
                capture.read_from_device_to_deque(&mut bytes).map_err(e)?;
            }
            let whole = bytes.len() / (4 * CHANNELS) * (4 * CHANNELS);
            floats.clear();
            let raw: Vec<u8> = bytes.drain(..whole).collect();
            floats.extend(raw.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)));
            super::to_mono(&floats, CHANNELS, &mut mono);
            gaps.fill(&mut mono, RATE, std::time::Instant::now());
            if !sink(&mono, RATE) {
                let _ = client.stop_stream();
                return Ok(());
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod capture {
    use libpulse_binding::def::BufferAttr;
    use libpulse_binding::sample::{Format, Spec};
    use libpulse_binding::stream::Direction;
    use libpulse_simple_binding::Simple;

    const RATE: u32 = 48000;
    const CHANNELS: u8 = 2;

    /// Record the default output's monitor until `sink` returns false.
    pub fn run(sink: &mut dyn FnMut(&[f32], u32) -> bool) -> Result<(), String> {
        let spec = Spec { format: Format::F32le, channels: CHANNELS, rate: RATE };
        // Small fragments (~10 ms) so lighting reacts quickly.
        let frag = (RATE as usize * CHANNELS as usize * 4 / 100) as u32;
        let attr = BufferAttr { maxlength: u32::MAX, tlength: u32::MAX, prebuf: u32::MAX, minreq: u32::MAX, fragsize: frag };
        let s = Simple::new(
            None,
            "OpenDeviceControl",
            Direction::Record,
            Some("@DEFAULT_MONITOR@"),
            "sound-reactive lighting",
            &spec,
            None,
            Some(&attr),
        )
        .map_err(|e| format!("cannot record the default output ({e}); is PipeWire or PulseAudio running?"))?;
        let mut buf = vec![0u8; frag as usize];
        let (mut floats, mut mono) = (Vec::new(), Vec::new());
        loop {
            s.read(&mut buf).map_err(|e| format!("reading sound: {e}"))?;
            floats.clear();
            floats.extend(buf.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)));
            super::to_mono(&floats, CHANNELS as usize, &mut mono);
            if !sink(&mono, RATE) {
                return Ok(());
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod capture {
    pub fn run(_sink: &mut dyn FnMut(&[f32], u32) -> bool) -> Result<(), String> {
        Err("sound capture is not supported on this system".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_holds_what_happened_between_frames() {
        let now = Instant::now();
        let at = |ms: u64| now.checked_sub(Duration::from_millis(ms)).unwrap_or(now);
        let mut loud = [0.0; BANDS];
        loud[0] = 0.9;
        let recent: VecDeque<_> =
            [(at(5), [0.1; BANDS], 0.1), (at(60), loud, 0.8), (at(300), [1.0; BANDS], 1.0)].into_iter().collect();
        let mut f = AudioFrame { bands: [0.1; BANDS], level: 0.1, ..AudioFrame::default() };
        apply_peak(&mut f, &recent, Duration::from_millis(100), now);
        // The beat 60 ms ago shows; the one 300 ms ago is outside the window.
        assert_eq!(f.bands[0], 0.9);
        assert_eq!(f.bands[1], 0.1);
        assert_eq!(f.level, 0.8);
    }

    fn sine(hz: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / 48000.0).sin()).collect()
    }

    #[test]
    fn fft_finds_a_tone() {
        let mut re: Vec<f32> = (0..64).map(|i| (std::f32::consts::TAU * 5.0 * i as f32 / 64.0).cos()).collect();
        let mut im = vec![0.0; 64];
        fft(&mut re, &mut im);
        let mag: Vec<f32> = (0..32).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt()).collect();
        let peak = mag.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        assert_eq!(peak, 5);
    }

    #[test]
    fn tones_land_in_their_bands() {
        let edges = band_edges();
        for hz in [80.0, 1000.0, 8000.0] {
            let mut a = Analyzer::default();
            let (f, _) = a.push(&sine(hz, 0.5, 48000), 48000).unwrap();
            let want = edges.windows(2).position(|w| hz >= w[0] && hz < w[1]).unwrap();
            let got = f.bands.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
            assert!(got.abs_diff(want) <= 1, "{hz} Hz: band {got}, expected {want}");
            assert!(f.level > 0.0);
        }
    }

    #[test]
    fn silence_is_dark() {
        let mut a = Analyzer::default();
        let (f, beat) = a.push(&vec![0.0; 48000], 48000).unwrap();
        assert!(f.bands.iter().all(|&b| b == 0.0));
        assert_eq!(f.level, 0.0);
        assert!(beat.is_none());
    }

    #[test]
    fn kick_drums_are_beats() {
        // Quiet hum with a loud 60 Hz burst every 0.5 s.
        let mut a = Analyzer::default();
        let mut beats = 0;
        let mut x = sine(1000.0, 0.02, 48000 * 4);
        for k in 0..8 {
            let start = k * 24000 + 4800;
            for (i, s) in sine(60.0, 0.8, 4800).into_iter().enumerate() {
                x[start + i] += s;
            }
        }
        for chunk in x.chunks(480) {
            if let Some((_, Some(_))) = a.push(chunk, 48000) {
                beats += 1;
            }
        }
        assert!((6..=9).contains(&beats), "{beats} beats");
    }

    #[test]
    fn no_packets_fade_to_black() {
        let mut a = Analyzer::default();
        let (f, _) = a.push(&sine(1000.0, 0.5, 48000), 48000).unwrap();
        assert!(f.level > 0.0);
        assert!(f.bands.iter().copied().fold(0.0, f32::max) > 0.5, "{:?}", f.bands);
        // Then the loopback goes quiet: polls every 10 ms with no packets.
        let t0 = Instant::now();
        let mut gaps = GapFiller::new(t0);
        let mut last = f;
        let mut mono = Vec::new();
        for i in 1..=200 {
            mono.clear();
            gaps.fill(&mut mono, 48000, t0 + Duration::from_millis(10 * i));
            if let Some((f, _)) = a.push(&mono, 48000) {
                last = f;
            }
        }
        assert!(last.level < 0.01, "level {}", last.level);
        assert!(last.bands.iter().all(|&b| b < 0.01), "{:?}", last.bands);

        // Real samples reset the gap; short gaps add nothing.
        let mut gaps = GapFiller::new(t0);
        let mut m = vec![0.5; 10];
        gaps.fill(&mut m, 48000, t0 + Duration::from_secs(5));
        assert_eq!(m.len(), 10);
        m.clear();
        gaps.fill(&mut m, 48000, t0 + Duration::from_secs(5) + Duration::from_millis(10));
        assert!(m.is_empty());
        // A long stall adds at most a second.
        gaps.fill(&mut m, 48000, t0 + Duration::from_secs(60));
        assert_eq!(m.len(), 48000);
    }

    #[test]
    fn frame_helpers() {
        let mut f = AudioFrame::default();
        f.bands[0] = 1.0;
        assert_eq!(f.band_at(0.0), 1.0);
        assert!(f.band_at(1.0 / 30.0) < 1.0);
        f.beats = vec![(0.1, 1.0)];
        assert!((f.pulse(0.5) - 0.8).abs() < 1e-5);
        assert_eq!(f.pulse(0.05), 0.0);
    }
}
