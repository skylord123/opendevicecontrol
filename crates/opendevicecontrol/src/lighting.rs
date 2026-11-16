//! Software lighting effects. Every frame is computed here and sent to the
//! device, so effects work the same on any device with LED zones (and keep
//! running while iCUE-style "software lighting" would).

use crate::audio::AudioFrame;
use crate::config::{Config, Effect, Lighting, SoundStyle};
use crate::fx::{FxState, Scene};
use crate::notes::{note_color, Note};
use opendevicecontrol_core::{LedShape, LightZone, Rgb};

/// Whether frames change over time (static lighting needs no animation).
pub fn is_animated(l: &Lighting) -> bool {
    l.on && l.brightness > 0 && !matches!(l.effect, Effect::Static { .. })
}

/// Where an LED sits: `x` 0..1 left to right, `y` from the top in the same
/// units as `x` (so distances are round), `h` 0..1 height from the bottom.
/// `local`: 0..1 along its own fan ring or strip (effects that fill each
/// item); `dev`: 0..1 along its device's LEDs (effects kept per device).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pos {
    pub x: f32,
    pub y: f32,
    pub h: f32,
    pub local: f32,
    pub dev: f32,
    /// On a fan ring (`local` goes round it, 0 at LED 1).
    pub ring: bool,
}

/// Positions of `total` LEDs: from the device's layout when it has one
/// (second value true), else in a row.
pub fn positions(total: usize, layout: &[LedShape]) -> (Vec<Pos>, bool) {
    let row = |i: usize| {
        let x = if total > 1 { i as f32 / (total - 1) as f32 } else { 0.5 };
        Pos { x, y: 0.0, h: 0.5, local: x, dev: x, ring: false }
    };
    let mut out: Vec<Pos> = (0..total).map(row).collect();
    if layout.is_empty() {
        return (out, false);
    }
    let centre = |s: &LedShape| (s.rect[0] + s.rect[2] / 2.0, s.rect[1] + s.rect[3] / 2.0);
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for s in layout {
        let (x, y) = centre(s);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    let (w, hgt) = ((x1 - x0).max(1e-3), (y1 - y0).max(1e-3));
    for s in layout {
        if let Some(p) = out.get_mut(s.led) {
            let (x, y) = centre(s);
            let px = (x - x0) / w;
            *p = Pos { x: px, y: (y - y0) / w, h: 1.0 - (y - y0) / hgt, local: px, dev: px, ring: false };
        }
    }
    (out, true)
}

/// Cheap deterministic noise in 0..1 for `(a, b)`.
fn hash01(a: u64, b: u64) -> f32 {
    let mut x = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

fn brighter(a: Rgb, b: Rgb) -> Rgb {
    Rgb::new(a.r.max(b.r), a.g.max(b.g), a.b.max(b.b))
}

/// Palette position for the `n`-th beat.
fn beat_colour(st: &SoundStyle, n: u64) -> f32 {
    if st.rainbow { (n % 12) as f32 / 12.0 } else { (n % 2) as f32 }
}

/// The notes' shares of a run of LEDs: (start, end, note) along 0..1, in
/// note-id order so shares stay put as loudness changes. `power` shapes
/// the shares (higher favours loud notes); a share below `floor` of the
/// total is dropped, like ColorChord's minimum splotch size.
fn note_shares(notes: &[Note], power: f32, floor: f32) -> Vec<(f32, f32, Note)> {
    let mut v: Vec<Note> = notes.iter().copied().filter(|n| n.size > 0.0).collect();
    v.sort_by_key(|n| n.id);
    let w: Vec<f32> = v.iter().map(|n| n.size.powf(power)).collect();
    let total: f32 = w.iter().sum();
    if total <= 1e-6 {
        return Vec::new();
    }
    let kept: Vec<(f32, Note)> = w.iter().zip(&v).map(|(w, n)| (w - floor * total, *n)).filter(|(w, _)| *w > 0.0).collect();
    let sum: f32 = kept.iter().map(|k| k.0).sum();
    let mut at = 0.0;
    kept.into_iter()
        .map(|(w, n)| {
            let start = at;
            at += w / sum.max(1e-6);
            (start, at, n)
        })
        .collect()
}

/// A note's colour at its current brightness.
fn note_light(n: &Note, gain: impl Fn(f32) -> f32) -> Rgb {
    note_color(n.pos).scale(gain(n.amp * 1.6))
}

/// A sound-reactive frame. `bars`: the LEDs have a height (2D layout), so
/// levels can rise as bars.
fn render_sound(e: &Effect, st: &SoundStyle, scene: &Scene, centre: Option<f32>, a: &AudioFrame, fx: &mut FxState) -> Vec<Rgb> {
    let (pos, bars) = (scene.pos, scene.bars);
    let g = st.sensitivity.clamp(0.25, 4.0);
    let gain = |v: f32| (v * g).clamp(0.0, 1.0);
    // Note effects: shared shapes, worked out once per frame.
    let shares = match e {
        Effect::Notes(_) => note_shares(&a.notes, 1.5, 0.1),
        Effect::NoteCells(_) => note_shares(&a.notes, 1.9, 0.0),
        _ => Vec::new(),
    };
    let occupied = (a.notes.iter().map(|n| n.size).sum::<f32>() * 1.5 * g).min(1.0);
    // Colours by note: shares of the palette's 0..1, by pitch.
    let by_pitch = {
        let mut v: Vec<Note> = a.notes.iter().copied().filter(|n| n.size > 0.02).collect();
        v.sort_by(|x, y| x.pos.total_cmp(&y.pos));
        note_shares(&v, 1.0, 0.0)
    };
    let pick = |t: f32| -> Rgb {
        if st.notes {
            if let Some((_, _, n)) = by_pitch.iter().find(|(s, e, _)| t >= *s && t <= *e).or(by_pitch.last()) {
                return note_color(n.pos);
            }
        }
        st.pick(t)
    };
    // A bar of `value` at height `h`: lit once the level is above the key,
    // with a soft edge (nothing lit in silence).
    let bar = |value: f32, h: f32| ((value - h) * 10.0).clamp(0.0, 1.0);
    let centre_y = centre.unwrap_or_else(|| pos.iter().map(|q| q.y).fold(0.0, f32::max) / 2.0);
    let radar_angle = (a.time * 180.0).rem_euclid(360.0);
    if let Some(f) = crate::fx::render_sound(e, st, scene, a, fx, &pick) {
        return f;
    }
    pos.iter()
        .enumerate()
        .map(|(i, p)| match e {
            Effect::Spectrum(_) => {
                let v = gain(a.band_at(p.x));
                if bars {
                    pick(if st.rainbow { p.x } else { p.h }).scale(bar(v, p.h))
                } else {
                    pick(p.x).scale(v)
                }
            }
            Effect::BeatPulse(_) => {
                let v = gain(a.pulse(0.35).max(a.level * 0.5));
                pick(beat_colour(st, a.beat_count)).scale(v)
            }
            Effect::Ripple(_) => {
                let d = ((p.x - 0.5).powi(2) + (p.y - centre_y).powi(2)).sqrt();
                let mut c = pick(0.0).scale(gain(a.level) * 0.15);
                for (k, &(age, strength)) in a.beats.iter().enumerate() {
                    let ring = (1.0 - (d - age * 0.9).abs() / 0.07).max(0.0) * (1.0 - age / 1.2).max(0.0);
                    if ring > 0.0 {
                        let n = a.beat_count.wrapping_sub(k as u64);
                        c = brighter(c, pick(beat_colour(st, n)).scale(gain(strength) * ring));
                    }
                }
                c
            }
            Effect::SoundStream(_) => {
                let hist = &a.history;
                let last = hist.len().saturating_sub(1);
                let v = gain(hist.get(((p.x * last as f32).round() as usize).min(last)).copied().unwrap_or(0.0));
                if bars {
                    pick(if st.rainbow { 0.6 - v * 0.6 } else { p.h }).scale(bar(v, p.h))
                } else {
                    pick(v).scale(v)
                }
            }
            Effect::Rain(_) => {
                let mut c = Rgb::BLACK;
                for (k, &(age, strength)) in a.beats.iter().enumerate() {
                    let n = a.beat_count.wrapping_sub(k as u64);
                    // Louder beats splash bigger drops (runs of LEDs).
                    let drop = 1 + (gain(strength) * 3.0) as usize;
                    let i = i / drop;
                    if hash01(i as u64, n) < 0.25 * gain(strength) {
                        let fade = (1.0 - age / 0.9).max(0.0);
                        c = brighter(c, pick(hash01(n, i as u64 + 7)).scale(fade));
                    }
                }
                c
            }
            Effect::LevelMeter(_) => {
                let v = gain(a.level * 1.4);
                // Each device meters by itself, even across the PC layout.
                let on = ((v - p.dev) * 12.0).clamp(0.0, 1.0);
                // Rainbow: green → red like a VU meter.
                let c = if st.rainbow && !st.notes { Rgb::from_hsv(120.0 * (1.0 - p.dev), 1.0, 1.0) } else { pick(p.dev) };
                c.scale(on)
            }
            Effect::Radar(_) => {
                // One turn every 2 s; the beam reaches as far as the sound is loud.
                let (dx, dy) = (p.x - 0.5, p.y - centre_y);
                let ang = dy.atan2(dx).to_degrees();
                let behind = (radar_angle - ang).rem_euclid(360.0);
                let reach = gain(a.level * 1.4) * 0.75;
                let on = (dx * dx + dy * dy).sqrt() < reach;
                let fade = (1.0 - behind / 90.0).max(0.0);
                if on { pick(ang.rem_euclid(360.0) / 360.0).scale(fade) } else { Rgb::BLACK }
            }
            Effect::LevelColor(_) => {
                let v = gain(a.level * 1.4);
                let c = if let (true, Some(n)) = (st.notes, a.notes.first()) {
                    note_color(n.pos)
                } else if st.rainbow {
                    // Like the official Aula driver: blue when quiet → red when loud.
                    const STEPS: [Rgb; 7] = [
                        Rgb::new(0, 0, 255),
                        Rgb::new(0, 255, 255),
                        Rgb::new(0, 255, 0),
                        Rgb::new(255, 255, 0),
                        Rgb::new(255, 128, 0),
                        Rgb::new(255, 0, 255),
                        Rgb::new(255, 0, 0),
                    ];
                    STEPS[((v * 7.0) as usize).min(6)]
                } else {
                    st.color.0.lerp(st.color2.0, v)
                };
                c.scale(v)
            }
            // Every fan ring and strip shows all the notes.
            Effect::Notes(_) => match shares.iter().find(|(s, e, _)| p.local >= *s && p.local <= *e).or(shares.last()) {
                Some((_, _, n)) => note_light(n, gain),
                None => Rgb::BLACK,
            },
            Effect::NoteCells(_) => {
                // Each LED has a fixed rank; the notes own ranks below how
                // full the lights are, so LEDs keep their note.
                let r = hash01(i as u64, 0xC0_10C4);
                if r >= occupied {
                    return Rgb::BLACK;
                }
                let t = r / occupied.max(1e-6);
                match shares.iter().find(|(s, e, _)| t >= *s && t < *e) {
                    Some((_, _, n)) => note_light(n, gain),
                    None => Rgb::BLACK,
                }
            }
            Effect::NoteFields(_) => {
                // Notes sit around an ellipse at their pitch; each LED goes
                // to the note that pulls hardest: loud and near.
                let (cx, cy) = (0.5, centre_y.max(0.05));
                let mut best: Option<(f32, &Note)> = None;
                for n in a.notes.iter().filter(|n| n.size > 0.03) {
                    let ang = n.pos * std::f32::consts::TAU;
                    let (sx, sy) = (cx + ang.sin() * 0.45, cy - ang.cos() * cy * 0.9);
                    let d = ((p.x - sx).powi(2) + (p.y - sy).powi(2)).sqrt() + 0.05;
                    let pull = n.size.powf(2.5) / d.powf(1.33);
                    if best.is_none_or(|(b, _)| pull > b) {
                        best = Some((pull, n));
                    }
                }
                best.map(|(_, n)| note_light(n, gain)).unwrap_or(Rgb::BLACK)
            }
            Effect::MainNote(_) => a.notes.first().map(|n| note_light(n, gain)).unwrap_or(Rgb::BLACK),
            _ => Rgb::BLACK,
        })
        .collect()
}

/// Render one frame for all LEDs of `zones` (concatenated, zone order) at
/// time `t` seconds. `layout` places the LEDs when the device knows where
/// they are; `audio` feeds the sound-reactive effects.
#[cfg(test)]
pub fn render(l: &Lighting, zones: &[LightZone], layout: &[LedShape], t: f64, audio: Option<&AudioFrame>) -> Vec<Rgb> {
    render_with(l, zones, layout, t, audio, &mut FxState::default())
}

/// [`render`] for a device, keeping what effects remember in `fx`.
pub fn render_with(l: &Lighting, zones: &[LightZone], layout: &[LedShape], t: f64, audio: Option<&AudioFrame>, fx: &mut FxState) -> Vec<Rgb> {
    let total: usize = zones.iter().map(|z| z.leds).sum();
    let (pos, bars) = positions(total, layout);
    render_at(l, zones, &Placed { pos: &pos, bars, centre: None }, t, audio, fx)
}

/// LEDs and where they sit (see [`render_at`]).
pub struct Placed<'a> {
    pub pos: &'a [Pos],
    /// Spread over two dimensions.
    pub bars: bool,
    /// The height effects spread from (default: the middle of these LEDs).
    pub centre: Option<f32>,
}

/// [`render_with`] with the LEDs placed by `at` (one per LED, zone order),
/// e.g. where they sit on the PC layout.
pub fn render_at(l: &Lighting, zones: &[LightZone], at: &Placed, t: f64, audio: Option<&AudioFrame>, fx: &mut FxState) -> Vec<Rgb> {
    let (pos, bars, centre) = (at.pos, at.bars, at.centre);
    let total: usize = zones.iter().map(|z| z.leds).sum();
    if !l.on || l.brightness == 0 {
        return vec![Rgb::BLACK; total];
    }
    let t = t as f32;
    let mut out = Vec::with_capacity(total);
    match &l.effect {
        Effect::Static { colors } => {
            // One colour per zone; fewer colours than zones repeat the last.
            for (zi, z) in zones.iter().enumerate() {
                let c = colors.get(zi).or(colors.last()).map(|c| c.0).unwrap_or(Rgb::WHITE);
                out.extend(std::iter::repeat_n(c, z.leds));
            }
        }
        Effect::Breathing { color, speed } => {
            // Smooth sine fade, 4 s period at speed 1.
            let phase = (t * speed.max(0.05) * std::f32::consts::TAU / 4.0).sin() * 0.5 + 0.5;
            out = vec![color.0.scale(0.05 + 0.95 * phase); total];
        }
        Effect::ColorCycle { speed } => {
            let hue = t * 36.0 * speed.max(0.05);
            out = vec![Rgb::from_hsv(hue, 1.0, 1.0); total];
        }
        Effect::RainbowWave { speed } => {
            for p in pos {
                let hue = t * 60.0 * speed.max(0.05) - p.x * 240.0;
                out.push(Rgb::from_hsv(hue, 1.0, 1.0));
            }
        }
        Effect::ColorWave { color, color2, speed } => {
            for p in pos {
                let phase = ((t * speed.max(0.05) * 0.5 - p.x * 0.75) * std::f32::consts::TAU).sin() * 0.5 + 0.5;
                out.push(color.0.lerp(color2.0, phase));
            }
        }
        e => {
            let centre_y = centre.unwrap_or_else(|| pos.iter().map(|q| q.y).fold(0.0, f32::max) / 2.0);
            let scene = Scene { pos, bars, centre_y };
            if let Some(f) = crate::fx::render_light(e, &scene, t) {
                out = f;
            } else {
                let silence = AudioFrame::default();
                let st = e.sound_style().cloned().unwrap_or_default();
                out = render_sound(e, &st, &scene, centre, audio.unwrap_or(&silence), fx);
            }
        }
    }
    let b = l.brightness.min(100) as f32 / 100.0;
    out.into_iter().map(|c| c.scale(b)).collect()
}

/// The zones and pieces of `device` that have their own effect: (the LEDs
/// they cover, in zone order, and the effect). Pieces come after zones, so
/// a fan's own effect wins over its zone's.
pub fn overrides(cfg: &Config, device: &str, zones: &[LightZone]) -> Vec<(std::ops::Range<usize>, Lighting)> {
    let mut zone_ov = Vec::new();
    let mut piece_ov = Vec::new();
    let mut first = 0;
    for z in zones {
        let base = first;
        first += z.leds;
        let key = format!("{device}/{}", z.id);
        if let Some(l) = cfg.zone_lighting.get(&key) {
            zone_ov.push((base..base + z.leds, l.clone()));
        }
        for p in cfg.pieces(device, &z.id) {
            if let Some(l) = cfg.zone_lighting.get(&format!("{key}/{}", p.start)) {
                let end = (p.start + p.leds).min(z.leds);
                if p.start < end {
                    piece_ov.push((base + p.start..base + end, l.clone()));
                }
            }
        }
    }
    zone_ov.extend(piece_ov);
    zone_ov
}

/// Paint each override's effect over its LEDs in `frame`; `fx` keeps what
/// each one's effect remembers.
pub fn apply_overrides(frame: &mut [Rgb], overrides: &[(std::ops::Range<usize>, Lighting)], t: f64, audio: Option<&AudioFrame>, fx: &mut Vec<FxState>) {
    fx.resize_with(overrides.len(), FxState::default);
    for ((r, l), state) in overrides.iter().zip(fx.iter_mut()) {
        let r = r.start.min(frame.len())..r.end.min(frame.len());
        let zone = LightZone { id: String::new(), name: String::new(), leds: r.len() };
        let sub = render_with(l, &[zone], &[], t, audio, state);
        frame[r].copy_from_slice(&sub);
    }
}

/// Brightness key: 100 → 66 → 33 → off → 100 (like the keyboard does itself).
pub fn next_brightness(current: u8) -> u8 {
    match current {
        67..=u8::MAX => 66,
        34..=66 => 33,
        1..=33 => 0,
        0 => 100,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HexColor;

    fn zones() -> Vec<LightZone> {
        ["a", "b", "c"].iter().map(|z| LightZone { id: (*z).into(), name: (*z).into(), leds: 1 }).collect()
    }

    #[test]
    fn static_colors_per_zone_and_brightness() {
        let l = Lighting {
            on: true,
            brightness: 50,
            effect: Effect::Static { colors: vec![HexColor(Rgb::new(200, 0, 0)), HexColor(Rgb::new(0, 100, 0))] },
            other: None,
        };
        let f = render(&l, &zones(), &[], 0.0, None);
        assert_eq!(f, vec![Rgb::new(100, 0, 0), Rgb::new(0, 50, 0), Rgb::new(0, 50, 0)]);
    }

    #[test]
    fn off_is_black() {
        let l = Lighting { on: false, ..Default::default() };
        assert!(render(&l, &zones(), &[], 1.0, None).iter().all(|c| *c == Rgb::BLACK));
    }

    #[test]
    fn rainbow_varies_across_leds() {
        let f = render(&Lighting::default(), &zones(), &[], 0.0, None);
        assert_ne!(f[0], f[2]);
    }

    #[test]
    fn brightness_cycle() {
        assert_eq!(next_brightness(100), 66);
        assert_eq!(next_brightness(66), 33);
        assert_eq!(next_brightness(33), 0);
        assert_eq!(next_brightness(0), 100);
    }

    fn sound(effect: Effect) -> Lighting {
        Lighting { effect, ..Default::default() }
    }

    /// A 3×3 grid: LED i at column i % 3, row i / 3.
    fn grid() -> (Vec<LightZone>, Vec<LedShape>) {
        let z = vec![LightZone { id: "keys".into(), name: "Keys".into(), leds: 9 }];
        let l = (0..9)
            .map(|i| LedShape {
                led: i,
                name: format!("k{i}"),
                label: String::new(),
                rect: [(i % 3) as f32 * 10.0, (i / 3) as f32 * 10.0, 9.0, 9.0],
            })
            .collect();
        (z, l)
    }

    #[test]
    fn layout_positions() {
        let (_, l) = grid();
        let (p, bars) = positions(9, &l);
        assert!(bars);
        assert_eq!((p[0].x, p[0].y, p[0].h), (0.0, 0.0, 1.0));
        assert_eq!((p[8].x, p[8].y, p[8].h), (1.0, 1.0, 0.0));
        let (p, bars) = positions(3, &[]);
        assert!(!bars);
        assert_eq!(p[2].x, 1.0);
    }

    #[test]
    fn silence_is_dark_for_sound_effects() {
        let (z, l) = grid();
        for name in Effect::SOUND_NAMES {
            let e = Lighting::default().effect.switch_to(name).unwrap();
            assert!(e.is_sound());
            assert!(render(&sound(e), &z, &l, 1.0, None).iter().all(|c| *c == Rgb::BLACK), "{name}");
        }
    }

    #[test]
    fn spectrum_bars_rise_from_the_bottom() {
        let (z, l) = grid();
        let a = AudioFrame { bands: [0.5; crate::audio::BANDS], ..Default::default() };
        let f = render(&sound(Effect::Spectrum(SoundStyle::default())), &z, &l, 0.0, Some(&a));
        assert!(f[6..9].iter().all(|c| *c != Rgb::BLACK));
        assert!(f[0..3].iter().all(|c| *c == Rgb::BLACK));
    }

    #[test]
    fn beat_pulse_follows_the_beat() {
        let z = zones();
        let mut a = AudioFrame { beats: vec![(0.0, 1.0)], beat_count: 1, ..Default::default() };
        let lit = render(&sound(Effect::BeatPulse(SoundStyle::default())), &z, &[], 0.0, Some(&a));
        assert!(lit.iter().all(|c| *c != Rgb::BLACK));
        a.beats = vec![(1.0, 1.0)];
        let gone = render(&sound(Effect::BeatPulse(SoundStyle::default())), &z, &[], 0.0, Some(&a));
        assert!(gone.iter().all(|c| *c == Rgb::BLACK));
    }

    #[test]
    fn level_meter_fills_from_the_left() {
        let a = AudioFrame { level: 0.35, ..Default::default() };
        let z = vec![LightZone { id: "z".into(), name: "z".into(), leds: 5 }];
        let f = render(&sound(Effect::LevelMeter(SoundStyle::default())), &z, &[], 0.0, Some(&a));
        assert_ne!(f[0], Rgb::BLACK);
        assert_eq!(f[4], Rgb::BLACK);
    }

    #[test]
    fn note_effects_use_the_notes_colours() {
        use crate::notes::Note;
        let notes = vec![
            Note { id: 1, pos: 0.0, amp: 1.0, size: 0.8 },      // A: yellow
            Note { id: 2, pos: 8.0 / 12.0, amp: 1.0, size: 0.4 }, // F: blue
        ];
        let a = AudioFrame { notes, ..Default::default() };
        let z = vec![LightZone { id: "z".into(), name: "z".into(), leds: 20 }];
        let f = render(&sound(Effect::Notes(SoundStyle::default())), &z, &[], 0.0, Some(&a));
        let (yellow, blue) = (Rgb::new(255, 255, 0), Rgb::new(0, 0, 255));
        let y = f.iter().filter(|&&c| c == yellow).count();
        let b = f.iter().filter(|&&c| c == blue).count();
        assert!(y > b && b > 0 && y + b == 20, "{y} yellow, {b} blue");
        let main = render(&sound(Effect::MainNote(SoundStyle::default())), &z, &[], 0.0, Some(&a));
        assert!(main.iter().all(|&c| c == yellow));
        let cells = render(&sound(Effect::NoteCells(SoundStyle::default())), &z, &[], 0.0, Some(&a));
        assert!(cells.contains(&yellow) && cells.contains(&blue));
        // Note colours on an ordinary effect.
        let st = SoundStyle { notes: true, ..Default::default() };
        let lm = render(&sound(Effect::LevelColor(st)), &z, &[], 0.0, Some(&AudioFrame { level: 1.0, ..a.clone() }));
        assert!(lm.iter().all(|&c| c == yellow || c == blue), "{:?}", lm[0]);
    }

    #[test]
    fn zones_and_fans_can_have_their_own_effect() {
        use crate::config::{Piece, PieceKind};
        let zones = vec![
            LightZone { id: "a".into(), name: "A".into(), leds: 2 },
            LightZone { id: "b".into(), name: "B".into(), leds: 6 },
        ];
        let mut cfg = crate::config::Config::default();
        cfg.zone_pieces.insert("d/b".into(), vec![Piece { kind: PieceKind::Fan, start: 2, leds: 3, ..Default::default() }]);
        let solid = |c: Rgb| Lighting { effect: Effect::Static { colors: vec![HexColor(c)] }, ..Default::default() };
        cfg.zone_lighting.insert("d/b".into(), solid(Rgb::new(0, 0, 255)));
        cfg.zone_lighting.insert("d/b/2".into(), solid(Rgb::new(255, 0, 0)));
        let ov = overrides(&cfg, "d", &zones);
        assert_eq!(ov.iter().map(|o| o.0.clone()).collect::<Vec<_>>(), vec![2..8, 4..7]);
        let mut f = vec![Rgb::WHITE; 8];
        apply_overrides(&mut f, &ov, 0.0, None, &mut Vec::new());
        let (b, r) = (Rgb::new(0, 0, 255), Rgb::new(255, 0, 0));
        assert_eq!(f, vec![Rgb::WHITE, Rgb::WHITE, b, b, r, r, r, b]);
    }

    #[test]
    fn sound_effects_round_trip_in_config() {
        let l = sound(Effect::Ripple(SoundStyle { rainbow: false, sensitivity: 2.0, ..Default::default() }));
        let s = toml::to_string(&l).unwrap();
        let back: Lighting = toml::from_str(&s).unwrap();
        assert_eq!(back, l);
        assert!(s.contains("effect = \"ripple\""), "{s}");
    }
}
