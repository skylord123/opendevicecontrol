//! Musical notes in what the computer is playing, after ColorChord by
//! Charles Lohr (<https://github.com/cnlohr/colorchord>, MIT/CC0): a
//! reimplementation of its method, not a port of its code.
//!
//! 1. A bank of slowly decaying sliding DFTs, 24 bins per octave over 5
//!    octaves from 55 Hz (A1). Each octave runs on the audio decimated to
//!    a rate that suits it, so the bank stays cheap.
//! 2. Every frame the bins are folded into one octave (an E is an E in any
//!    octave), blurred a little, and the peaks found with sub-bin accuracy.
//! 3. Peaks are tracked from frame to frame as notes with stable ids, a
//!    fast amplitude (brightness) and a slow one (how much room it gets),
//!    so notes don't flicker in and out.

use opendevicecontrol_core::Rgb;

/// Bins per octave.
pub const BINS: usize = 24;
const OCTAVES: usize = 5;
/// The lowest frequency analysed (A1). Note position 0 is an A.
const BASE_HZ: f32 = 55.0;
/// Most notes tracked at once.
pub const MAX_NOTES: usize = 12;

/// A note being heard.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Note {
    /// Stays the same while the note lasts.
    pub id: u32,
    /// Pitch class, 0..1 around the octave (0 = A, 1/12 = A#, ...).
    pub pos: f32,
    /// Fast-following loudness, 0..1: brightness.
    pub amp: f32,
    /// Slow-following loudness, 0..1: how much of the LEDs the note gets.
    pub size: f32,
}

/// The colour of pitch class `pos` (0..1 around the octave, 0 = A).
pub fn note_color(pos: f32) -> Rgb {
    let n = pos.rem_euclid(1.0) * 12.0;
    let hue = if n < 4.0 {
        60.0 - 15.0 * n // yellow -> red
    } else if n < 8.0 {
        360.0 - 30.0 * (n - 4.0) // red -> magenta -> blue
    } else {
        240.0 - 45.0 * (n - 8.0) // blue -> cyan -> green -> yellow
    };
    Rgb::from_hsv(hue, 1.0, 1.0)
}

/// One sliding DFT bin: a phasor turning at the bin's frequency and a
/// decaying complex accumulator.
#[derive(Clone, Copy)]
struct Bin {
    rot: (f32, f32),
    phasor: (f32, f32),
    acc: (f32, f32),
    /// Accumulator decay per (decimated) sample.
    keep: f32,
}

/// One octave: its decimation state and its bins.
struct Octave {
    /// Input samples averaged per decimated sample.
    factor: usize,
    sum: f32,
    count: usize,
    bins: Vec<Bin>,
    /// Samples since the phasors were last renormalised.
    since_norm: u32,
}

/// Finds and tracks notes in a stream of mono samples.
pub struct NoteFinder {
    rate: u32,
    octaves: Vec<Octave>,
    /// Smoothed bin magnitudes, all octaves (low to high).
    smooth: Vec<f32>,
    /// Slowly decaying peak of the folded spectrum (automatic gain).
    agc: f32,
    notes: Vec<Note>,
    next_id: u32,
}

impl NoteFinder {
    pub fn new(rate: u32) -> Self {
        let rate = rate.max(8000);
        let octaves = (0..OCTAVES)
            .map(|o| {
                // Run each octave at about 8x its top frequency.
                let top = BASE_HZ * 2f32.powi(o as i32 + 1);
                let mut factor = 1;
                while rate as f32 / (factor * 2) as f32 >= top * 8.0 {
                    factor *= 2;
                }
                let r = rate as f32 / factor as f32;
                let bins = (0..BINS)
                    .map(|i| {
                        let hz = BASE_HZ * 2f32.powf(o as f32 + i as f32 / BINS as f32);
                        let w = std::f32::consts::TAU * hz / r;
                        // About 12 cycles of memory (tighter than a
                        // semitone), but at least 30 ms and at most 220 ms.
                        let tau = (12.0 / hz).clamp(0.03, 0.22);
                        Bin { rot: (w.cos(), w.sin()), phasor: (1.0, 0.0), acc: (0.0, 0.0), keep: (-1.0 / (tau * r)).exp() }
                    })
                    .collect();
                Octave { factor, sum: 0.0, count: 0, bins, since_norm: 0 }
            })
            .collect();
        Self { rate, octaves, smooth: vec![0.0; BINS * OCTAVES], agc: 1e-3, notes: Vec::new(), next_id: 1 }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Feed mono samples (-1..1).
    pub fn push(&mut self, samples: &[f32]) {
        for o in &mut self.octaves {
            for &x in samples {
                o.sum += x;
                o.count += 1;
                if o.count < o.factor {
                    continue;
                }
                let s = o.sum / o.count as f32;
                (o.sum, o.count) = (0.0, 0);
                for b in &mut o.bins {
                    let (pr, pi) = b.phasor;
                    let (rr, ri) = b.rot;
                    b.phasor = (pr * rr - pi * ri, pr * ri + pi * rr);
                    let a = 1.0 - b.keep;
                    b.acc.0 = b.acc.0 * b.keep + s * pr * a;
                    b.acc.1 = b.acc.1 * b.keep + s * pi * a;
                }
                o.since_norm += 1;
                if o.since_norm >= 1024 {
                    // Keep rounding from shrinking or growing the phasors.
                    o.since_norm = 0;
                    for b in &mut o.bins {
                        let m = (b.phasor.0 * b.phasor.0 + b.phasor.1 * b.phasor.1).sqrt().max(1e-6);
                        b.phasor = (b.phasor.0 / m, b.phasor.1 / m);
                    }
                }
            }
        }
    }

    /// Update the notes from what has been heard; call once per frame,
    /// `dt` seconds apart.
    pub fn update(&mut self, dt: f32) -> &[Note] {
        // Magnitudes, smoothed a little.
        let k = 1.0 - (-dt / 0.03).exp();
        for (o, oct) in self.octaves.iter().enumerate() {
            for (i, b) in oct.bins.iter().enumerate() {
                let m = 2.0 * (b.acc.0 * b.acc.0 + b.acc.1 * b.acc.1).sqrt();
                let s = &mut self.smooth[o * BINS + i];
                *s += (m - *s) * k;
            }
        }
        // Fade the lowest and highest octave in and out, so a note doesn't
        // jump when it crosses the analysed range; fold into one octave.
        let n = self.smooth.len();
        let mut folded = [0.0f32; BINS];
        for (j, &v) in self.smooth.iter().enumerate() {
            let edge = ((j + 1) as f32 / BINS as f32).min((n - j) as f32 / BINS as f32).min(1.0);
            folded[j % BINS] += v * edge;
        }
        // Blur around the circle: two passes of [1/4 1/2 1/4].
        for _ in 0..2 {
            let prev = folded;
            for i in 0..BINS {
                folded[i] = prev[(i + BINS - 1) % BINS] * 0.25 + prev[i] * 0.5 + prev[(i + 1) % BINS] * 0.25;
            }
        }
        // Automatic gain: follow peaks quickly, fall back over ~6 s.
        let loud = folded.iter().copied().fold(0.0, f32::max);
        self.agc = (self.agc * (-dt / 6.0).exp()).max(loud).max(2e-4);
        let silent = loud < 3e-4;
        let norm: Vec<f32> = folded.iter().map(|v| if silent { 0.0 } else { v / self.agc }).collect();

        // Peaks, with the position refined between bins.
        let mut peaks: Vec<(f32, f32)> = Vec::new();
        for i in 0..BINS {
            let (p, c, q) = (norm[(i + BINS - 1) % BINS], norm[i], norm[(i + 1) % BINS]);
            if c < 0.12 || c < p || c < q || (c == p && c == q) {
                continue;
            }
            let d = p - 2.0 * c + q;
            let off = if d.abs() > 1e-9 { (0.5 * (p - q) / d).clamp(-0.5, 0.5) } else { 0.0 };
            peaks.push((((i as f32 + off) / BINS as f32).rem_euclid(1.0), c));
        }
        peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
        self.track(&peaks, dt);
        &self.notes
    }

    /// Match peaks to notes: move, strengthen, spawn, merge and fade.
    fn track(&mut self, peaks: &[(f32, f32)], dt: f32) {
        // Rates tuned for ColorChord's ~60 frames a second, scaled to ours.
        let per = |rate60: f32| 1.0 - (1.0 - rate60).powf(dt * 60.0);
        let (pos_k, amp_k, size_k) = (per(0.35), per(0.35), per(0.2));
        let jump = 2.0 / BINS as f32;
        let mut taken = vec![false; peaks.len()];
        for note in &mut self.notes {
            let best = peaks
                .iter()
                .enumerate()
                .filter(|(j, _)| !taken[*j])
                .map(|(j, &(pos, a))| (j, circ_dist(note.pos, pos), pos, a))
                .filter(|&(_, d, ..)| d < jump)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            match best {
                Some((j, _, pos, a)) => {
                    taken[j] = true;
                    note.pos = circ_lerp(note.pos, pos, pos_k);
                    note.amp += (a - note.amp) * amp_k;
                }
                None => note.amp *= 1.0 - amp_k,
            }
        }
        for (j, &(pos, a)) in peaks.iter().enumerate() {
            if !taken[j] && a > 0.2 && self.notes.len() < MAX_NOTES {
                self.notes.push(Note { id: self.next_id, pos, amp: a * amp_k, size: 0.0 });
                self.next_id = self.next_id.wrapping_add(1).max(1);
            }
        }
        // Merge notes that met.
        let combine = 0.6 / BINS as f32;
        let mut i = 0;
        while i < self.notes.len() {
            let mut j = i + 1;
            while j < self.notes.len() {
                if circ_dist(self.notes[i].pos, self.notes[j].pos) < combine {
                    let (a, b) = (self.notes[i], self.notes[j]);
                    let (keep, gone) = if a.amp >= b.amp { (i, j) } else { (j, i) };
                    let w = b.amp / (a.amp + b.amp).max(1e-6);
                    self.notes[keep].pos = circ_lerp(a.pos, b.pos, w);
                    self.notes[keep].amp = (a.amp + b.amp).min(1.0);
                    self.notes[keep].size = a.size.max(b.size);
                    self.notes.remove(gone);
                    if gone < i {
                        i -= 1;
                    }
                    continue;
                }
                j += 1;
            }
            i += 1;
        }
        for note in &mut self.notes {
            note.amp = note.amp.clamp(0.0, 1.0);
            note.size += (note.amp - note.size) * size_k;
        }
        self.notes.retain(|n| n.size > 0.02 || n.amp > 0.05);
    }
}

/// Distance between two positions on the 0..1 circle.
fn circ_dist(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(1.0);
    d.min(1.0 - d)
}

/// `a` moved toward `b` by `t`, the short way round the circle.
fn circ_lerp(a: f32, b: f32, t: f32) -> f32 {
    let mut d = (b - a).rem_euclid(1.0);
    if d > 0.5 {
        d -= 1.0;
    }
    (a + d * t).rem_euclid(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: &[f32], secs: f32, rate: u32) -> Vec<f32> {
        let n = (secs * rate as f32) as usize;
        (0..n)
            .map(|i| hz.iter().map(|h| 0.3 * (std::f32::consts::TAU * h * i as f32 / rate as f32).sin()).sum())
            .collect()
    }

    /// Run `samples` through a finder in ~10 ms frames; the notes at the end.
    fn listen(samples: &[f32]) -> Vec<Note> {
        let mut f = NoteFinder::new(48000);
        for chunk in samples.chunks(512) {
            f.push(chunk);
            f.update(512.0 / 48000.0);
        }
        let mut notes = f.notes.clone();
        notes.sort_by(|a, b| b.amp.total_cmp(&a.amp));
        notes
    }

    fn semitones(pos: f32) -> f32 {
        pos * 12.0
    }

    #[test]
    fn an_a_is_found_in_any_octave() {
        for hz in [110.0, 440.0, 880.0] {
            let notes = listen(&tone(&[hz], 1.0, 48000));
            let top = notes.first().unwrap_or_else(|| panic!("{hz} Hz: no note"));
            let off = circ_dist(top.pos, 0.0) * 12.0;
            assert!(off < 0.3, "{hz} Hz: {} semitones from A", off);
            assert!(top.amp > 0.5 && top.size > 0.3, "{top:?}");
        }
    }

    #[test]
    fn a_chord_gives_its_notes() {
        // C major: C4, E4, G4.
        let notes = listen(&tone(&[261.63, 329.63, 392.0], 1.5, 48000));
        let mut found: Vec<f32> = notes.iter().filter(|n| n.amp > 0.2).map(|n| semitones(n.pos).round().rem_euclid(12.0)).collect();
        found.sort_by(f32::total_cmp);
        // From A: C = 3, E = 7, G = 10.
        assert_eq!(found, vec![3.0, 7.0, 10.0], "{notes:?}");
    }

    #[test]
    fn notes_keep_their_id_and_fade_in_silence() {
        let mut f = NoteFinder::new(48000);
        let mut ids = Vec::new();
        for chunk in tone(&[440.0], 1.0, 48000).chunks(512) {
            f.push(chunk);
            if let Some(n) = f.update(512.0 / 48000.0).first() {
                ids.push(n.id);
            }
        }
        ids.dedup();
        assert_eq!(ids.len(), 1, "one note, one id: {ids:?}");
        for chunk in vec![0.0; 48000].chunks(512) {
            f.push(chunk);
            f.update(512.0 / 48000.0);
        }
        assert!(f.notes.is_empty(), "{:?}", f.notes);
    }

    #[test]
    fn colours_follow_the_wheel() {
        assert_eq!(note_color(0.0), Rgb::new(255, 255, 0)); // A: yellow
        assert_eq!(note_color(4.0 / 12.0), Rgb::new(255, 0, 0)); // C#: red
        assert_eq!(note_color(8.0 / 12.0), Rgb::new(0, 0, 255)); // F: blue
        assert_eq!(note_color(1.0), note_color(0.0));
    }
}
