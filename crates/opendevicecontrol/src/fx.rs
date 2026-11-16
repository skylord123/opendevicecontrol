//! The newer effects, after ideas from WLED's sound-reactive effects, the
//! OpenRGB effects plugin and FastLED's examples (our own implementations).
//! Some remember the previous frame: [`FxState`] keeps that per device.

use crate::audio::{AudioFrame, BANDS};
use crate::config::{Effect, SoundStyle};
use crate::lighting::Pos;
use opendevicecontrol_core::Rgb;
use std::collections::VecDeque;

/// What an effect remembers from frame to frame, per device (and per zone
/// or fan with an effect of its own). Starts over when the effect changes.
#[derive(Default)]
pub struct FxState {
    effect: &'static str,
    last_t: Option<f32>,
    /// Gravity meter: the falling peak marker.
    peak: f32,
    /// Slowly following band levels, for spotting a band that jumps.
    slow_bands: [f32; BANDS],
    bubbles: Vec<Bubble>,
    /// Colours flowing outward (pitch stream, DJ light), newest first.
    trail: VecDeque<Rgb>,
    /// Spectrogram rows, newest first.
    rows: VecDeque<[f32; BANDS]>,
    since_push: f32,
    seed: u64,
}

struct Bubble {
    x: f32,
    y: f32,
    band: f32,
    amp: f32,
    radius: f32,
}

/// The layout facts the effects need.
pub struct Scene<'a> {
    pub pos: &'a [Pos],
    /// LEDs spread over two dimensions (a keyboard, the PC layout).
    pub bars: bool,
    /// The height things spread from.
    pub centre_y: f32,
}

/// Cheap deterministic noise in 0..1 for `(a, b)`.
pub fn hash01(a: u64, b: u64) -> f32 {
    let mut x = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

/// Smooth value noise in 0..1.
fn noise(x: f32, y: f32) -> f32 {
    let (xi, yi) = (x.floor(), y.floor());
    let (fx, fy) = (x - xi, y - yi);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let h = |i: f32, j: f32| hash01(i as i64 as u64, j as i64 as u64 ^ 0x51_7CC1);
    let a = h(xi, yi) + (h(xi + 1.0, yi) - h(xi, yi)) * s(fx);
    let b = h(xi, yi + 1.0) + (h(xi + 1.0, yi + 1.0) - h(xi, yi + 1.0)) * s(fx);
    a + (b - a) * s(fy)
}

fn brighter(a: Rgb, b: Rgb) -> Rgb {
    Rgb::new(a.r.max(b.r), a.g.max(b.g), a.b.max(b.b))
}

/// Black → red → orange → yellow → white, for heat 0..1.
fn fire(h: f32) -> Rgb {
    let h = h.clamp(0.0, 1.0) * 3.0;
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
    Rgb::new(c(h), c(h - 1.0), c(h - 2.0))
}

/// How far an LED is from where its item "starts": the top of a fan ring
/// running down both sides (0 top, 1 bottom), along a strip, or out from
/// the middle of a 2D layout.
fn outward(p: &Pos, s: &Scene) -> f32 {
    if p.ring {
        p.local.min(1.0 - p.local) * 2.0
    } else if s.bars {
        (((p.x - 0.5).powi(2) + (p.y - s.centre_y).powi(2)).sqrt() / 0.6).min(1.0)
    } else {
        p.local
    }
}

/// Height 0 (bottom) .. 1 (top) for flames: fan rings burn up both sides.
fn height(p: &Pos, s: &Scene) -> (f32, f32) {
    if p.ring {
        // (height, which side as a noise column)
        ((p.local - 0.5).abs() * 2.0, if p.local < 0.5 { 0.0 } else { 7.0 } + p.x * 3.0)
    } else if s.bars {
        (p.h, p.x * 6.0)
    } else {
        (p.local, 0.0)
    }
}

/// The dominant frequency as 0..1 across the bands (interpolated), and its
/// strength.
fn major_peak(a: &AudioFrame) -> (f32, f32) {
    let (i, &v) = a.bands.iter().enumerate().max_by(|x, y| x.1.total_cmp(y.1)).unwrap_or((0, &0.0));
    let l = if i > 0 { a.bands[i - 1] } else { v };
    let r = if i + 1 < BANDS { a.bands[i + 1] } else { v };
    let d = l - 2.0 * v + r;
    let off = if d.abs() > 1e-6 { (0.5 * (l - r) / d).clamp(-0.5, 0.5) } else { 0.0 };
    (((i as f32 + off) / (BANDS - 1) as f32).clamp(0.0, 1.0), v)
}

fn band_avg(a: &AudioFrame, from: usize, to: usize) -> f32 {
    a.bands[from..=to].iter().sum::<f32>() / (to - from + 1) as f32
}

/// A plain (not sound-reactive) effect from this module, or None.
pub fn render_light(e: &Effect, scene: &Scene, t: f32) -> Option<Vec<Rgb>> {
    let pos = scene.pos;
    Some(match e {
        Effect::Pacifica { speed } => {
            let t = t * speed.max(0.05);
            pos.iter()
                .map(|p| {
                    let x = if scene.bars { p.x } else { p.local } * 6.0;
                    let y = p.y * 6.0;
                    let layer = |k: f32, sx: f32, sp: f32| (x * sx + y * 0.4 * k + t * sp + (t * 0.07 * k).sin() * 2.0).sin() * 0.5 + 0.5;
                    let w = (layer(1.0, 1.0, 0.6) + layer(2.0, 1.7, -0.45) + layer(3.0, 0.6, 0.3)) / 3.0;
                    // Deep blue → teal → aqua, whitecaps where waves stack.
                    let deep = Rgb::new(0, 10, 60);
                    let sea = Rgb::new(0, 120, 140);
                    let mut c = deep.lerp(sea, w);
                    let cap = ((w - (0.72 + 0.08 * (t * 0.3).sin())) * 6.0).clamp(0.0, 1.0);
                    c = c.lerp(Rgb::new(170, 230, 230), cap);
                    c
                })
                .collect()
        }
        Effect::Twinkle { speed } => pos
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let rate = (0.4 + hash01(i as u64, 1)) * speed.max(0.05) * 0.6;
                let clock = t * rate + hash01(i as u64, 2) * 10.0;
                let cycle = clock.floor() as u64;
                if hash01(i as u64, cycle) > 0.45 {
                    return Rgb::BLACK;
                }
                let ph = clock.fract();
                let v = if ph < 0.25 { ph / 0.25 } else { (1.0 - ph) / 0.75 };
                Rgb::from_hsv(hash01(i as u64 ^ 0xF0F0, cycle) * 360.0, 0.55, 1.0).scale(v * v)
            })
            .collect(),
        Effect::Meteor { color, speed } => {
            let head = (t * speed.max(0.05) * 0.5).fract();
            pos.iter()
                .enumerate()
                .map(|(i, p)| {
                    let along = if scene.bars && !p.ring { p.x } else { p.local };
                    let behind = (head - along).rem_euclid(1.0);
                    if behind < 0.03 {
                        return color.0;
                    }
                    // A tail that fades and sparkles.
                    let fade = (1.0 - behind / 0.45).max(0.0);
                    let twinkle = if hash01(i as u64, (t * 18.0) as u64) < 0.25 { 0.4 } else { 1.0 };
                    color.0.scale(fade * fade * twinkle)
                })
                .collect()
        }
        _ => return None,
    })
}

/// A sound-reactive effect from this module, or None. `pick`: the style's
/// colour at 0..1 (rainbow, custom or by note).
pub fn render_sound(
    e: &Effect,
    st: &SoundStyle,
    scene: &Scene,
    a: &AudioFrame,
    state: &mut FxState,
    pick: &dyn Fn(f32) -> Rgb,
) -> Option<Vec<Rgb>> {
    let name = e.name();
    if state.effect != name {
        *state = FxState { effect: name, seed: state.seed.wrapping_add(1), ..FxState::default() };
    }
    let t = a.time;
    let dt = state.last_t.map(|l| (t - l).clamp(0.0, 0.25)).unwrap_or(0.0);
    state.last_t = Some(t);
    let g = st.sensitivity.clamp(0.25, 4.0);
    let gain = |v: f32| (v * g).clamp(0.0, 1.0);
    let pos = scene.pos;
    let max_y = pos.iter().map(|p| p.y).fold(0.0, f32::max);

    Some(match e {
        Effect::AudioFire(_) => {
            let bass = gain(band_avg(a, 0, 3) * 1.2);
            let flare = a.pulse(0.45);
            let fuel = gain(a.level * 2.0);
            pos.iter()
                .map(|p| {
                    let (h, col) = height(p, scene);
                    let n = noise(col * 1.5 + 13.0, h * 3.0 - t * (1.6 + bass * 2.0));
                    let heat = (n * 1.25 + bass * 0.5 + flare * 0.5 - h * (1.35 - bass * 0.6)).clamp(0.0, 1.0) * fuel;
                    if st.rainbow && !st.notes { fire(heat) } else { pick(1.0 - heat).scale(heat) }
                })
                .collect()
        }
        Effect::GravityMeter(_) => {
            let v = gain(a.level * 1.4);
            // The marker jumps up with the bar and falls back slowly.
            state.peak = (state.peak - dt * 0.5).max(v);
            pos.iter()
                .map(|p| {
                    // From each device's middle outward.
                    let d = (p.dev - 0.5).abs() * 2.0;
                    if state.peak > 0.03 && (d - state.peak).abs() < 0.05 {
                        return Rgb::new(255, 255, 255).scale(0.85);
                    }
                    if v < 0.01 || d >= v {
                        return Rgb::BLACK;
                    }
                    let shimmer = noise(d * 4.0 + p.local * 2.0, t * 0.8);
                    if st.rainbow && !st.notes { Rgb::from_hsv(shimmer * 360.0 + t * 20.0, 1.0, 1.0) } else { pick(shimmer) }
                })
                .collect()
        }
        Effect::FrequencyStar(_) => {
            let lift = 1.0 / (1.0 + a.level);
            pos.iter()
                .map(|p| {
                    let ang = if p.ring {
                        p.local
                    } else {
                        ((p.y - scene.centre_y).atan2(p.x - 0.5) / std::f32::consts::TAU + 0.25).rem_euclid(1.0)
                    };
                    // Mirrored, so both halves show low to high.
                    let b = 1.0 - (ang * 2.0 - 1.0).abs();
                    let v = gain(a.band_at(b)).powf(lift);
                    let c = if st.rainbow && !st.notes { Rgb::from_hsv(ang * 360.0 + t * 25.0, 1.0, 1.0) } else { pick(b) };
                    c.scale(v)
                })
                .collect()
        }
        Effect::SpectrumBubbles(_) => {
            // A band that jumps blows a bubble.
            for (i, (&now, slow)) in a.bands.iter().zip(state.slow_bands.iter_mut()).enumerate() {
                let jump = now - *slow;
                *slow += (now - *slow) * (1.0 - (-dt / 0.25).exp());
                let band = i as f32 / (BANDS - 1) as f32;
                let busy = state.bubbles.iter().any(|b| (b.band - band).abs() < 1e-3);
                if jump > 0.18 && now > 0.3 && !busy && state.bubbles.len() < 12 {
                    let r = |k: u64| hash01(state.seed.wrapping_add(k), (t * 1000.0) as u64 ^ i as u64);
                    let x = if scene.bars { r(1) } else { band };
                    state.bubbles.push(Bubble { x, y: r(2) * max_y, band, amp: gain(now).clamp(0.25, 1.0), radius: 0.0 });
                }
            }
            for b in &mut state.bubbles {
                b.radius += dt * 0.35 * (0.6 + b.amp);
            }
            state.bubbles.retain(|b| b.radius < 0.12 + b.amp * 0.35);
            pos.iter()
                .map(|p| {
                    let mut c = Rgb::BLACK;
                    for b in &state.bubbles {
                        let d = ((p.x - b.x).powi(2) + (p.y - b.y).powi(2)).sqrt();
                        let ring = (1.0 - (d - b.radius).abs() / (0.035 * (0.6 + b.amp))).max(0.0);
                        let life = (1.0 - b.radius / (0.12 + b.amp * 0.35)).max(0.0).sqrt();
                        if ring > 0.0 {
                            c = brighter(c, pick(b.band).scale(ring * ring * life));
                        }
                    }
                    c
                })
                .collect()
        }
        Effect::PitchStream(_) | Effect::DjLight(_) => {
            state.since_push += dt;
            if state.since_push >= 0.03 || state.trail.is_empty() {
                state.since_push = 0.0;
                let c = if matches!(e, Effect::DjLight(_)) {
                    // Bass red, mids green, highs blue.
                    let (lo, mid, hi) = (gain(band_avg(a, 0, 2)), gain(band_avg(a, 5, 8)), gain(band_avg(a, 11, 15) * 1.4));
                    Rgb::new((lo * lo * 255.0) as u8, (mid * mid * 255.0) as u8, (hi * hi * 255.0) as u8)
                } else {
                    let (peak, strength) = major_peak(a);
                    let v = gain(a.level * 1.5).min(gain(strength * 1.3));
                    let c = if st.rainbow && !st.notes { Rgb::from_hsv(peak * 280.0, 1.0, 1.0) } else { pick(peak) };
                    c.scale(v)
                };
                state.trail.push_front(c);
                state.trail.truncate(48);
            }
            let last = state.trail.len().saturating_sub(1);
            pos.iter()
                .map(|p| {
                    let d = if matches!(e, Effect::DjLight(_)) && !p.ring && !scene.bars { (p.local - 0.5).abs() * 2.0 } else { outward(p, scene) };
                    state.trail.get((d * last as f32).round() as usize).copied().unwrap_or(Rgb::BLACK)
                })
                .collect()
        }
        Effect::NoiseMove(_) => {
            const DOTS: usize = 8;
            pos.iter()
                .map(|p| {
                    let mut c = Rgb::BLACK;
                    for k in 0..DOTS {
                        let kf = k as f32 * 3.1;
                        let v = gain(a.band_at(k as f32 / (DOTS - 1) as f32) * 1.2);
                        if v < 0.02 {
                            continue;
                        }
                        let d = if scene.bars && !p.ring {
                            let (dx, dy) = (noise(t * 0.15, kf), noise(t * 0.15 + 50.0, kf) * max_y.max(0.05));
                            ((p.x - dx).powi(2) + (p.y - dy).powi(2)).sqrt()
                        } else {
                            let at = noise(t * 0.2, kf);
                            let d = (p.local - at).abs();
                            if p.ring { d.min(1.0 - d) } else { d }
                        };
                        let glow = (-(d / 0.09).powi(2)).exp() * v;
                        c = brighter(c, pick(k as f32 / DOTS as f32).scale(glow));
                    }
                    c
                })
                .collect()
        }
        Effect::AudioPlasma(_) => {
            let gate = gain(a.level * 1.3);
            pos.iter()
                .map(|p| {
                    let x = if scene.bars { p.x } else { p.local };
                    let b = ((x * 5.0 + t * 0.7).sin() + (p.y * 6.0 - t * 0.5).sin() + ((x + p.y) * 4.0 + t * 0.3).sin()) / 6.0 + 0.5;
                    if b > gate {
                        return Rgb::BLACK;
                    }
                    let c = if st.rainbow && !st.notes { Rgb::from_hsv((b + t * 0.05) * 360.0, 1.0, 1.0) } else { pick(b) };
                    c.scale(1.0 - b / gate.max(1e-3) * 0.6)
                })
                .collect()
        }
        Effect::Spectrogram(_) => {
            state.since_push += dt;
            if state.since_push >= 0.06 || state.rows.is_empty() {
                state.since_push = 0.0;
                state.rows.push_front(a.bands.map(gain));
                state.rows.truncate(32);
            }
            let last = state.rows.len().saturating_sub(1);
            pos.iter()
                .map(|p| {
                    let row = if scene.bars && !p.ring { ((1.0 - p.h) * last as f32).round() as usize } else { 0 };
                    let col = if scene.bars && !p.ring { p.x } else { p.local };
                    let bands = state.rows.get(row).copied().unwrap_or([0.0; BANDS]);
                    let v = bands[((col * (BANDS - 1) as f32).round() as usize).min(BANDS - 1)];
                    let c = if st.rainbow && !st.notes { Rgb::from_hsv(240.0 * (1.0 - v), 1.0, 1.0) } else { pick(v) };
                    c.scale(v)
                })
                .collect()
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(n: usize) -> Vec<Pos> {
        (0..n)
            .map(|i| {
                let x = i as f32 / (n - 1) as f32;
                Pos { x, y: 0.0, h: 0.5, local: x, dev: x, ring: false }
            })
            .collect()
    }

    #[test]
    fn noise_is_smooth_and_bounded() {
        for i in 0..200 {
            let x = i as f32 * 0.05;
            let (a, b) = (noise(x, 1.3), noise(x + 0.01, 1.3));
            assert!((0.0..=1.0).contains(&a));
            assert!((a - b).abs() < 0.05);
        }
    }

    #[test]
    fn gravity_peak_falls_back() {
        let pos = row(21);
        let scene = Scene { pos: &pos, bars: false, centre_y: 0.0 };
        let mut st = FxState::default();
        let e = Effect::GravityMeter(SoundStyle::default());
        let loud = AudioFrame { level: 0.6, time: 0.0, ..Default::default() };
        render_sound(&e, &SoundStyle::default(), &scene, &loud, &mut st, &|_| Rgb::WHITE);
        let high = st.peak;
        let quiet = AudioFrame { level: 0.0, time: 0.5, ..Default::default() };
        let f = render_sound(&e, &SoundStyle::default(), &scene, &quiet, &mut st, &|_| Rgb::WHITE).unwrap();
        assert!(st.peak < high && st.peak > 0.0);
        // Only the marker is lit, away from the middle.
        assert_eq!(f[10], Rgb::BLACK);
        assert!(f.iter().any(|&c| c != Rgb::BLACK));
    }

    #[test]
    fn a_jumping_band_blows_a_bubble() {
        let pos = row(30);
        let scene = Scene { pos: &pos, bars: false, centre_y: 0.0 };
        let mut st = FxState::default();
        let e = Effect::SpectrumBubbles(SoundStyle::default());
        let quiet = AudioFrame::default();
        render_sound(&e, &SoundStyle::default(), &scene, &quiet, &mut st, &|_| Rgb::WHITE);
        let mut bands = [0.0; BANDS];
        bands[3] = 0.9;
        let hit = AudioFrame { bands, time: 0.05, ..Default::default() };
        render_sound(&e, &SoundStyle::default(), &scene, &hit, &mut st, &|_| Rgb::WHITE);
        assert_eq!(st.bubbles.len(), 1);
        let later = AudioFrame { bands, time: 0.2, ..Default::default() };
        let f = render_sound(&e, &SoundStyle::default(), &scene, &later, &mut st, &|_| Rgb::WHITE).unwrap();
        assert!(f.iter().any(|&c| c != Rgb::BLACK));
    }

    #[test]
    fn pitch_stream_flows_outward() {
        let pos = row(10);
        let scene = Scene { pos: &pos, bars: false, centre_y: 0.0 };
        let mut st = FxState::default();
        let e = Effect::PitchStream(SoundStyle::default());
        let mut bands = [0.0; BANDS];
        bands[2] = 1.0;
        for k in 0..30 {
            let a = AudioFrame { bands, level: 0.8, time: k as f32 * 0.03, ..Default::default() };
            render_sound(&e, &SoundStyle::default(), &scene, &a, &mut st, &|_| Rgb::WHITE);
        }
        assert!(st.trail.len() > 20);
        let a = AudioFrame { time: 1.0, ..Default::default() };
        let f = render_sound(&e, &SoundStyle::default(), &scene, &a, &mut st, &|_| Rgb::WHITE).unwrap();
        // The newest (silent) colour at the start, older sound further out.
        assert_eq!(f[0], Rgb::BLACK);
        assert_ne!(f[5], Rgb::BLACK);
    }

    #[test]
    fn plain_effects_light_up() {
        let pos = row(16);
        let scene = Scene { pos: &pos, bars: false, centre_y: 0.0 };
        for e in [
            Effect::Pacifica { speed: 1.0 },
            Effect::Twinkle { speed: 1.0 },
            Effect::Meteor { color: crate::config::HexColor(Rgb::WHITE), speed: 1.0 },
        ] {
            let lit = (0..20).any(|k| render_light(&e, &scene, k as f32 * 0.37).unwrap().iter().any(|&c| c != Rgb::BLACK));
            assert!(lit, "{}", e.name());
        }
    }
}
