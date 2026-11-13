//! Software lighting effects. Every frame is computed here and sent to the
//! device, so effects work the same on any device with LED zones (and keep
//! running while iCUE-style "software lighting" would).

use crate::audio::AudioFrame;
use crate::config::{Effect, Lighting, SoundStyle};
use opendevicecontrol_core::{LedShape, LightZone, Rgb};

/// Whether frames change over time (static lighting needs no animation).
pub fn is_animated(l: &Lighting) -> bool {
    l.on && l.brightness > 0 && !matches!(l.effect, Effect::Static { .. })
}

/// Where an LED sits: `x` 0..1 left to right, `y` from the top in the same
/// units as `x` (so distances are round), `h` 0..1 height from the bottom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pos {
    pub x: f32,
    pub y: f32,
    pub h: f32,
}

/// Positions of `total` LEDs: from the device's layout when it has one
/// (second value true), else in a row.
pub fn positions(total: usize, layout: &[LedShape]) -> (Vec<Pos>, bool) {
    let row = |i: usize| Pos { x: if total > 1 { i as f32 / (total - 1) as f32 } else { 0.5 }, y: 0.0, h: 0.5 };
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
            *p = Pos { x: (x - x0) / w, y: (y - y0) / w, h: 1.0 - (y - y0) / hgt };
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

/// A sound-reactive frame. `bars`: the LEDs have a height (2D layout), so
/// levels can rise as bars.
fn render_sound(e: &Effect, st: &SoundStyle, pos: &[Pos], bars: bool, a: &AudioFrame) -> Vec<Rgb> {
    let g = st.sensitivity.clamp(0.25, 4.0);
    let gain = |v: f32| (v * g).clamp(0.0, 1.0);
    // A bar of `value` at height `h`: lit once the level is above the key,
    // with a soft edge (nothing lit in silence).
    let bar = |value: f32, h: f32| ((value - h) * 10.0).clamp(0.0, 1.0);
    let centre_y = pos.iter().map(|q| q.y).fold(0.0, f32::max) / 2.0;
    let radar_angle = (a.time * 180.0).rem_euclid(360.0);
    pos.iter()
        .enumerate()
        .map(|(i, p)| match e {
            Effect::Spectrum(_) => {
                let v = gain(a.band_at(p.x));
                if bars {
                    st.pick(if st.rainbow { p.x } else { p.h }).scale(bar(v, p.h))
                } else {
                    st.pick(p.x).scale(v)
                }
            }
            Effect::BeatPulse(_) => {
                let v = gain(a.pulse(0.35).max(a.level * 0.5));
                st.pick(beat_colour(st, a.beat_count)).scale(v)
            }
            Effect::Ripple(_) => {
                let d = ((p.x - 0.5).powi(2) + (p.y - centre_y).powi(2)).sqrt();
                let mut c = st.pick(0.0).scale(gain(a.level) * 0.15);
                for (k, &(age, strength)) in a.beats.iter().enumerate() {
                    let ring = (1.0 - (d - age * 0.9).abs() / 0.07).max(0.0) * (1.0 - age / 1.2).max(0.0);
                    if ring > 0.0 {
                        let n = a.beat_count.wrapping_sub(k as u64);
                        c = brighter(c, st.pick(beat_colour(st, n)).scale(gain(strength) * ring));
                    }
                }
                c
            }
            Effect::SoundStream(_) => {
                let hist = &a.history;
                let last = hist.len().saturating_sub(1);
                let v = gain(hist.get(((p.x * last as f32).round() as usize).min(last)).copied().unwrap_or(0.0));
                if bars {
                    st.pick(if st.rainbow { 0.6 - v * 0.6 } else { p.h }).scale(bar(v, p.h))
                } else {
                    st.pick(v).scale(v)
                }
            }
            Effect::Rain(_) => {
                let mut c = Rgb::BLACK;
                for (k, &(age, strength)) in a.beats.iter().enumerate() {
                    let n = a.beat_count.wrapping_sub(k as u64);
                    if hash01(i as u64, n) < 0.25 * gain(strength) {
                        let fade = (1.0 - age / 0.9).max(0.0);
                        c = brighter(c, st.pick(hash01(n, i as u64 + 7)).scale(fade));
                    }
                }
                c
            }
            Effect::LevelMeter(_) => {
                let v = gain(a.level * 1.4);
                let on = ((v - p.x) * 12.0).clamp(0.0, 1.0);
                // Rainbow: green → red like a VU meter.
                let c = if st.rainbow { Rgb::from_hsv(120.0 * (1.0 - p.x), 1.0, 1.0) } else { st.pick(p.x) };
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
                if on { st.pick(ang.rem_euclid(360.0) / 360.0).scale(fade) } else { Rgb::BLACK }
            }
            Effect::LevelColor(_) => {
                let v = gain(a.level * 1.4);
                let c = if st.rainbow {
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
            _ => Rgb::BLACK,
        })
        .collect()
}

/// Render one frame for all LEDs of `zones` (concatenated, zone order) at
/// time `t` seconds. `layout` places the LEDs when the device knows where
/// they are; `audio` feeds the sound-reactive effects.
pub fn render(l: &Lighting, zones: &[LightZone], layout: &[LedShape], t: f64, audio: Option<&AudioFrame>) -> Vec<Rgb> {
    let total: usize = zones.iter().map(|z| z.leds).sum();
    if !l.on || l.brightness == 0 {
        return vec![Rgb::BLACK; total];
    }
    let (pos, bars) = positions(total, layout);
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
            for p in &pos {
                let hue = t * 60.0 * speed.max(0.05) - p.x * 240.0;
                out.push(Rgb::from_hsv(hue, 1.0, 1.0));
            }
        }
        Effect::ColorWave { color, color2, speed } => {
            for p in &pos {
                let phase = ((t * speed.max(0.05) * 0.5 - p.x * 0.75) * std::f32::consts::TAU).sin() * 0.5 + 0.5;
                out.push(color.0.lerp(color2.0, phase));
            }
        }
        e => {
            let silence = AudioFrame::default();
            let st = e.sound_style().cloned().unwrap_or_default();
            out = render_sound(e, &st, &pos, bars, audio.unwrap_or(&silence));
        }
    }
    let b = l.brightness.min(100) as f32 / 100.0;
    out.into_iter().map(|c| c.scale(b)).collect()
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
        assert_eq!(p[0], Pos { x: 0.0, y: 0.0, h: 1.0 });
        assert_eq!(p[8], Pos { x: 1.0, y: 1.0, h: 0.0 });
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
    fn sound_effects_round_trip_in_config() {
        let l = sound(Effect::Ripple(SoundStyle { rainbow: false, sensitivity: 2.0, ..Default::default() }));
        let s = toml::to_string(&l).unwrap();
        let back: Lighting = toml::from_str(&s).unwrap();
        assert_eq!(back, l);
        assert!(s.contains("effect = \"ripple\""), "{s}");
    }
}
