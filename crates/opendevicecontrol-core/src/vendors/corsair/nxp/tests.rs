//! Packet encodings checked against the byte layouts in ckb-next and OpenRGB.

use super::*;
use std::collections::HashSet;

fn k55() -> &'static NxpModel {
    model_for_pid(0x1b3d).unwrap()
}

fn planes() -> ([u8; LED_SLOTS], [u8; LED_SLOTS], [u8; LED_SLOTS]) {
    let mut r = [0u8; LED_SLOTS];
    let mut g = [0u8; LED_SLOTS];
    let mut b = [0u8; LED_SLOTS];
    for i in 0..LED_SLOTS {
        r[i] = i as u8;
        g[i] = 0x80 | i as u8;
        b[i] = 0xff - i as u8;
    }
    (r, g, b)
}

// ---------------------------------------------------------------- K55

#[test]
fn k55_key_table_unchanged() {
    // Hardware-verified routing: G1–G6, MR, brightness, lock → host only.
    let t = software_key_table(k55());
    let special: HashSet<usize> = [120, 121, 122, 123, 124, 125, 132, 71, 96].into();
    for (i, mode) in t.iter().enumerate() {
        assert_eq!(*mode, if special.contains(&i) { ROUTE_HOST } else { ROUTE_HID }, "slot {i}");
    }
    let p = key_table_packets(&t);
    assert_eq!(p.len(), 6);
    assert_eq!(&p[0][..8], &[0x07, 0x40, 30, 0x00, 0x00, 0x80, 0x01, 0x80]);
    assert_eq!(&p[5][..4], &[0x07, 0x40, 2, 0x00]);
    assert_eq!(&p[5][4..], &[150, 0x80, 151, 0x80]);
}

#[test]
fn k55_hardware_key_table() {
    let t = hardware_key_table(k55());
    assert_eq!(t[132], ACT_MR);
    assert_eq!(t[71], ACT_LIGHT);
    assert_eq!(t[96], ACT_LOCK);
    assert_eq!(t[120], ROUTE_HID);
}

#[test]
fn k55_zones_and_packet() {
    let (zones, map) = build_zones(k55(), KbLayout::Ansi);
    let ids: Vec<_> = zones.iter().map(|z| (z.id.as_str(), z.leds)).collect();
    assert_eq!(ids, [("left", 1), ("middle", 1), ("right", 1)]);
    assert_eq!(map, [0, 1, 2]);
    let p = three_zone_packet(&[Rgb::new(1, 2, 3), Rgb::new(4, 5, 6), Rgb::new(7, 8, 9)]);
    assert_eq!(p, [0x07, 0x25, 0x00, 0x00, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
}

#[test]
fn key_locks() {
    assert_eq!(key_lock_flags(false, true, true, true), 0xa0);
    assert_eq!(key_lock_flags(true, true, true, false), 0xa7);
    assert_eq!(key_lock_flags(true, false, false, true), 0xa9);
}

// ---------------------------------------------------------------- per-key

#[test]
fn per_key_24bit_layout() {
    let (r, g, b) = planes();
    let p = per_key_24_packets(&r, &g, &b);
    assert_eq!(p.len(), 12);
    // Red: 7F 01 3C 00 r[0..60], 7F 02 3C 00 r[60..120], 7F 03 30 00 r[120..168]
    assert_eq!(&p[0][..4], &[0x7f, 0x01, 0x3c, 0x00]);
    assert_eq!(&p[0][4..], &r[..60]);
    assert_eq!(&p[1][..4], &[0x7f, 0x02, 0x3c, 0x00]);
    assert_eq!(&p[1][4..], &r[60..120]);
    assert_eq!(&p[2][..4], &[0x7f, 0x03, 0x30, 0x00]);
    assert_eq!(&p[2][4..], &r[120..168]);
    assert_eq!(p[3], [0x07, 0x28, 0x01, 0x03, 0x01]);
    assert_eq!(&p[4][4..], &g[..60]);
    assert_eq!(p[7], [0x07, 0x28, 0x02, 0x03, 0x01]);
    assert_eq!(&p[10][4..], &b[120..168]);
    assert_eq!(p[11], [0x07, 0x28, 0x03, 0x03, 0x02]);
    assert!(p.iter().all(|x| x.len() <= PACKET));
}

#[test]
fn per_key_9bit_layout() {
    let mut r = [0u8; LED_SLOTS];
    let mut g = [0u8; LED_SLOTS];
    let b = [0xffu8; LED_SLOTS];
    r[1] = 0xff; // LED 1: high nibble of byte 0
    g[0] = 0x40; // 0x40 >> 5 = 2 → 7 - 2 = 5, low nibble of green byte 0
    let p = per_key_9_packets(&r, &g, &b);
    assert_eq!(p.len(), 5);
    let lens: Vec<u8> = p[..4].iter().map(|x| x[2]).collect();
    assert_eq!(lens, [60, 60, 60, 36]);
    for (i, x) in p[..4].iter().enumerate() {
        assert_eq!(&x[..2], &[0x7f, i as u8 + 1]);
        assert_eq!(x[3], 0);
        assert_eq!(x.len(), 4 + x[2] as usize);
    }
    let data: Vec<u8> = p[..4].iter().flat_map(|x| x[4..].to_vec()).collect();
    assert_eq!(data.len(), 216);
    assert_eq!(data[0], 0x07); // red: LED0 off (7), LED1 full (0)
    assert_eq!(data[1], 0x77);
    assert_eq!(data[72], 0x75); // green plane starts at byte 72
    assert!(data[144..].iter().all(|&x| x == 0x00)); // blue all full
    assert_eq!(p[4], [0x07, 0x27, 0x00, 0x00, 0xd8]);
}

#[test]
fn per_key_layouts_are_sane() {
    for m in MODELS {
        let Lighting::PerKey { leds, .. } = m.lighting else { continue };
        for layout in [KbLayout::Ansi, KbLayout::Iso, KbLayout::Abnt, KbLayout::Jis] {
            let v: Vec<_> = layout_leds(leds, layout).collect();
            let unique: HashSet<u8> = v.iter().map(|l| l.led).collect();
            assert_eq!(unique.len(), v.len(), "{} {layout:?}: duplicate LED", m.name);
            assert!(v.iter().all(|l| (l.led as usize) < LED_SLOTS), "{}", m.name);
            assert!(v.windows(2).all(|w| w[0].x <= w[1].x), "{}: not left to right", m.name);
            assert!(v.len() > 80, "{} {layout:?}: {} LEDs", m.name, v.len());
            let (zones, map) = build_zones(m, layout);
            assert_eq!(zones.iter().map(|z| z.leds).sum::<usize>(), map.len());
        }
        for k in m.keys {
            assert!((k.bit as usize) < KEY_SLOTS);
        }
    }
    // ISO keys only in ISO, ANSI enter position only in ANSI.
    let k70 = model_for_pid(0x1b13).unwrap();
    let Lighting::PerKey { leds, .. } = k70.lighting else { unreachable!() };
    let names = |l| layout_leds(leds, l).map(|k| k.name).collect::<Vec<_>>();
    assert!(names(KbLayout::Iso).contains(&"# (ISO)"));
    assert!(!names(KbLayout::Ansi).contains(&"# (ISO)"));
    assert!(names(KbLayout::Ansi).contains(&"\\"));
    assert!(!names(KbLayout::Iso).contains(&"\\"));
    // 9-bit models only use the first 144 LEDs.
    for m in MODELS {
        if let Lighting::PerKey { leds, depth: ColorDepth::Nine } = m.lighting {
            assert!(leds.iter().all(|l| (l.led as usize) < LED_SLOTS_9), "{}", m.name);
        }
    }
}

#[test]
fn k95_platinum_light_bar_zone() {
    let m = model_for_pid(0x1b2d).unwrap();
    let (zones, map) = build_zones(m, KbLayout::Ansi);
    assert_eq!(zones[1].id, "light_bar");
    assert_eq!(zones[1].leds, 19);
    assert!(map[zones[0].leds..].iter().all(|&l| (0x90..=0xa2).contains(&l)));
}

#[test]
fn layout_byte() {
    assert_eq!(KbLayout::from_info(0), KbLayout::Ansi);
    assert_eq!(KbLayout::from_info(1), KbLayout::Iso);
    assert_eq!(KbLayout::from_info(3), KbLayout::Jis);
    assert_eq!(KbLayout::from_info(4), KbLayout::Ansi);
}

// ---------------------------------------------------------------- mice etc.

#[test]
fn mouse_colour_packet() {
    // M65 PRO, as OpenLinkHub sends it: 07 22 03 01 01 RGB 02 RGB 03 RGB
    let p = mouse_packet(&[(1, Rgb::new(1, 2, 3)), (3, Rgb::new(4, 5, 6)), (2, Rgb::new(7, 8, 9))]);
    assert_eq!(p, [0x07, 0x22, 0x03, 0x01, 1, 1, 2, 3, 3, 4, 5, 6, 2, 7, 8, 9]);
}

#[test]
fn mouse_zone_ids() {
    for m in MODELS.iter().filter(|m| matches!(m.lighting, Lighting::Mouse)) {
        let ids: Vec<u8> = m.zones.iter().flat_map(|z| z.leds.iter().copied()).collect();
        assert!(ids.iter().all(|i| (1..=6).contains(i)), "{}", m.name);
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len(), "{}", m.name);
        assert!(m.max_dpi >= 6000);
    }
}

#[test]
fn pad_packets() {
    let mm800 = model_for_pid(0x1b3b).unwrap();
    let Lighting::Pad { count, control } = mm800.lighting else { panic!() };
    assert_eq!((count, control), (15, 0x04));
    let p = pad_packet(&[Rgb::new(1, 2, 3); 15]);
    assert_eq!(&p[..4], &[0x07, 0x22, 0x0f, 0x00]);
    assert_eq!(p.len(), 4 + 45);
    assert_eq!(&p[4..7], &[1, 2, 3]);
    // Every device zone is covered exactly once.
    for m in MODELS {
        if let Lighting::Pad { count, .. } = m.lighting {
            let mut all: Vec<u8> = m.zones.iter().flat_map(|z| z.leds.iter().copied()).collect();
            all.sort();
            assert_eq!(all, (0..count).collect::<Vec<_>>(), "{}", m.name);
        }
    }
    // ST100: logo is device zone 4 (OpenRGB remap), 9 zones, control byte 3.
    let st100 = model_for_pid(0x0a34).unwrap();
    assert!(matches!(st100.lighting, Lighting::Pad { count: 9, control: 0x03 }));
    assert_eq!(st100.zones[1].leds, &[4]);
}

#[test]
fn dark_core_packet_layout() {
    let p = dark_core_packet(2, Rgb::new(10, 20, 30));
    assert_eq!(p, [0x07, 0xaa, 0, 0, 0x04, 0x07, 0, 0, 100, 10, 20, 30, 0, 0, 0, 3]);
}

#[test]
fn dpi_packets() {
    assert_eq!(dpi_stage_packet(1, 800, None), [0x07, 0x13, 0xd1, 0x00, 0x00, 0x20, 0x03, 0x20, 0x03]);
    assert_eq!(
        dpi_stage_packet(0, 400, Some(Rgb::new(255, 0, 0))),
        [0x07, 0x13, 0xd0, 0x00, 0x00, 0x90, 0x01, 0x90, 0x01, 255, 0, 0]
    );
}

#[test]
fn model_table_is_consistent() {
    let pids: HashSet<u16> = MODELS.iter().map(|m| m.pid).collect();
    assert_eq!(pids.len(), MODELS.len(), "duplicate PID");
    for m in MODELS {
        assert!(!m.name.is_empty() && !m.slug.is_empty());
        let ids: HashSet<&str> = m.keys.iter().map(|k| k.id).collect();
        assert_eq!(ids.len(), m.keys.len(), "{}: duplicate key id", m.name);
        assert_eq!(m.max_dpi > 0, m.kind == DeviceKind::Mouse, "{}", m.name);
    }
}
