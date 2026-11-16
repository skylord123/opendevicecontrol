//! The PC layout (the Lighting page's canvas): every fan, strip and keyboard
//! placed where it sits in the case, so effects can run across them by
//! position.
//!
//! A device contributes items:
//! * a device whose driver knows where its LEDs sit (a keyboard): one block;
//! * every copy of every fan or strip described on a zone;
//! * a zone with nothing described: one strip of all its LEDs.
//!
//! Hidden zones contribute nothing. Item keys, used to store where each item
//! was placed: `dev` (block), `dev/zone` (undescribed zone),
//! `dev/zone/<start>/<copy>` (piece).

use crate::config::{Config, Piece, PieceKind, Place};
use opendevicecontrol_core::{LedShape, LightZone};

/// The layout's size in layout units.
pub const WIDTH: f32 = 1000.0;
pub const HEIGHT: f32 = 600.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// LEDs evenly around a ring; LED 1 at the top before rotation.
    Fan,
    /// LEDs in a row, left to right before rotation.
    Strip,
    /// LEDs where the driver puts them (centres, 0..1 across the width).
    Block(Vec<(f32, f32)>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub key: String,
    pub device: String,
    /// Full name, for hovering.
    pub name: String,
    /// Short label drawn on the layout ("Side fans 2/3").
    pub title: String,
    /// The device's name for the zone ("Addressable RGB LED 1"), drawn
    /// under the title; empty for a whole device.
    pub channel: String,
    pub shape: Shape,
    /// The device's LED indices (zone order), in the item's own order.
    pub leds: Vec<usize>,
    /// Which copy of a mirrored piece (0 for most items).
    pub copy: usize,
    pub reverse: bool,
}

/// A device's items, named with the zone names the user gave.
pub fn items(cfg: &Config, device: &str, device_name: &str, zones: &[LightZone], shapes: &[LedShape]) -> Vec<Item> {
    if !shapes.is_empty() {
        let (mut x0, mut y0, mut x1) = (f32::MAX, f32::MAX, f32::MIN);
        let centre = |s: &LedShape| (s.rect[0] + s.rect[2] / 2.0, s.rect[1] + s.rect[3] / 2.0);
        for s in shapes {
            let (x, y) = centre(s);
            (x0, y0, x1) = (x0.min(x), y0.min(y), x1.max(x));
        }
        let w = (x1 - x0).max(1e-3);
        let mut by_led: Vec<&LedShape> = shapes.iter().collect();
        by_led.sort_by_key(|s| s.led);
        return vec![Item {
            key: device.to_string(),
            device: device.to_string(),
            name: device_name.to_string(),
            title: device_name.to_string(),
            channel: String::new(),
            shape: Shape::Block(by_led.iter().map(|s| (((centre(s).0 - x0) / w), (centre(s).1 - y0) / w)).collect()),
            leds: by_led.iter().map(|s| s.led).collect(),
            copy: 0,
            reverse: false,
        }];
    }
    let mut out = Vec::new();
    let mut first = 0;
    for z in zones {
        let zkey = format!("{device}/{}", z.id);
        let base = first;
        first += z.leds;
        if cfg.hidden_zones.contains(&zkey) {
            continue;
        }
        let zone_name = cfg.zone_names.get(&zkey).cloned().unwrap_or_else(|| z.name.clone());
        let pieces = cfg.pieces(device, &z.id);
        if pieces.is_empty() {
            out.push(Item {
                key: zkey,
                device: device.to_string(),
                name: format!("{device_name}: {zone_name}"),
                title: zone_name.clone(),
                channel: z.name.clone(),
                shape: if z.leds > 1 { Shape::Strip } else { Shape::Fan },
                leds: (base..base + z.leds).collect(),
                copy: 0,
                reverse: false,
            });
            continue;
        }
        for (k, p) in pieces.iter().enumerate() {
            let end = (p.start + p.leds).min(z.leds);
            let label = piece_label(&pieces, k);
            // A zone with one piece goes by the zone's name.
            let short = if p.name.trim().is_empty() && pieces.len() == 1 { zone_name.clone() } else { label.clone() };
            for copy in 0..p.copies.max(1) {
                let name = if p.copies > 1 { format!("{label} ({}/{})", copy + 1, p.copies) } else { label.clone() };
                let title = if p.copies > 1 { format!("{short} {}/{}", copy + 1, p.copies) } else { short.clone() };
                out.push(Item {
                    key: format!("{zkey}/{}/{copy}", p.start),
                    device: device.to_string(),
                    name: format!("{zone_name}: {name}"),
                    title,
                    channel: z.name.clone(),
                    shape: piece_shape(p),
                    leds: (base + p.start..base + end).collect(),
                    copy,
                    reverse: p.reverse,
                });
            }
        }
    }
    out
}

/// A piece's display name: its own, else "Fan 2", "Strip 1"...
pub fn piece_label(pieces: &[Piece], i: usize) -> String {
    let p = &pieces[i];
    if !p.name.trim().is_empty() {
        return p.name.trim().to_string();
    }
    let n = pieces[..=i].iter().filter(|q| q.kind == p.kind && q.name.trim().is_empty()).count();
    format!("{} {n}", p.kind.name())
}

fn piece_shape(p: &Piece) -> Shape {
    match p.kind {
        PieceKind::Fan => Shape::Fan,
        PieceKind::Strip | PieceKind::Other => Shape::Strip,
    }
}

/// Where each of a device's LEDs sits on the PC layout, for effects that run
/// across it: `None` until every item of the device has been placed (the
/// Lighting page places them). A LED shown by several copies (fans on a
/// splitter) takes the first copy's place.
pub fn canvas_positions(cfg: &Config, device: &str, zones: &[LightZone], shapes: &[LedShape]) -> Option<Vec<crate::lighting::Pos>> {
    let total: usize = zones.iter().map(|z| z.leds).sum();
    let items = items(cfg, device, "", zones, shapes);
    if items.is_empty() {
        return None;
    }
    let mut out: Vec<Option<crate::lighting::Pos>> = vec![None; total];
    for it in items.iter().filter(|it| it.copy == 0) {
        let place = cfg.layout_places.get(&it.key)?;
        let n = it.leds.len();
        for (j, &led) in it.leds.iter().enumerate() {
            let (x, y) = led_point(it, place, j);
            let local = match it.shape {
                Shape::Fan => j as f32 / n.max(1) as f32,
                _ if n > 1 => j as f32 / (n - 1) as f32,
                _ => 0.5,
            };
            if let Some(slot) = out.get_mut(led) {
                *slot = Some(crate::lighting::Pos {
                    x: x / WIDTH,
                    y: y / WIDTH,
                    h: 1.0 - (y / HEIGHT).clamp(0.0, 1.0),
                    local,
                    dev: if total > 1 { led as f32 / (total - 1) as f32 } else { 0.5 },
                    ring: it.shape == Shape::Fan,
                });
            }
        }
    }
    // LEDs on hidden zones aren't placed: park them off to the side.
    Some(
        out.into_iter()
            .enumerate()
            .map(|(i, p)| {
                let dev = if total > 1 { i as f32 / (total - 1) as f32 } else { 0.5 };
                p.unwrap_or(crate::lighting::Pos { x: 0.0, y: 0.0, h: 0.0, local: dev, dev, ring: false })
            })
            .collect(),
    )
}

/// The height effects spread from on the PC layout (its middle).
pub const CENTRE_Y: f32 = HEIGHT / WIDTH / 2.0;

/// A sensible size for an item that hasn't been placed.
pub fn default_size(item: &Item) -> f32 {
    match &item.shape {
        Shape::Fan => 110.0,
        Shape::Strip => (item.leds.len() as f32 * 9.0).clamp(60.0, 400.0),
        Shape::Block(_) => 420.0,
    }
}

/// Where every item sits: its stored place, or the next free spot in rows
/// for items not placed yet.
pub fn places(cfg: &Config, items: &[Item]) -> Vec<Place> {
    let (mut x, mut y, mut row) = (20.0f32, 20.0f32, 0.0f32);
    items
        .iter()
        .map(|it| {
            if let Some(p) = cfg.layout_places.get(&it.key) {
                return *p;
            }
            let size = default_size(it);
            let (w, h) = match &it.shape {
                Shape::Fan => (size, size),
                Shape::Strip => (size, 20.0),
                Shape::Block(pts) => (size, pts.iter().map(|p| p.1).fold(0.0, f32::max) * size + 20.0),
            };
            if x + w > WIDTH - 20.0 && x > 20.0 {
                (x, y, row) = (20.0, y + row + 50.0, 0.0); // room for the labels
            }
            let place = Place { x: x + w / 2.0, y: y + h / 2.0, size, rotation: 0.0 };
            x += w + 30.0;
            row = row.max(h);
            place
        })
        .collect()
}

/// Turn `(dx, dy)` by `deg` degrees clockwise (screen y points down).
pub fn rotate(dx: f32, dy: f32, deg: f32) -> (f32, f32) {
    let (s, c) = deg.to_radians().sin_cos();
    (dx * c - dy * s, dx * s + dy * c)
}

/// Where the item's `j`-th LED sits, in layout units.
pub fn led_point(item: &Item, place: &Place, j: usize) -> (f32, f32) {
    let n = item.leds.len().max(1);
    let (dx, dy) = match &item.shape {
        Shape::Fan => {
            let step = j as f32 / n as f32 * 360.0;
            let a = (if item.reverse { -step } else { step }).to_radians();
            let r = place.size * 0.38;
            (a.sin() * r, -a.cos() * r)
        }
        Shape::Strip => {
            let k = if item.reverse { n - 1 - j } else { j };
            let t = if n > 1 { k as f32 / (n - 1) as f32 - 0.5 } else { 0.0 };
            (t * place.size, 0.0)
        }
        Shape::Block(pts) => {
            let (px, py) = pts.get(j).copied().unwrap_or_default();
            let hgt = pts.iter().map(|p| p.1).fold(0.0, f32::max);
            ((px - 0.5) * place.size, (py - hgt / 2.0) * place.size)
        }
    };
    let (rx, ry) = rotate(dx, dy, place.rotation);
    (place.x + rx, place.y + ry)
}

/// Half the item's extent along its own axes (before rotation), for hit
/// testing and drawing.
pub fn half_extent(item: &Item, place: &Place) -> (f32, f32) {
    match &item.shape {
        Shape::Fan => (place.size / 2.0, place.size / 2.0),
        Shape::Strip => (place.size / 2.0 + 8.0, 10.0),
        Shape::Block(pts) => (place.size / 2.0 + 8.0, pts.iter().map(|p| p.1).fold(0.0, f32::max) * place.size / 2.0 + 8.0),
    }
}

/// Whether layout point `(x, y)` is on the item.
pub fn hit(item: &Item, place: &Place, x: f32, y: f32) -> bool {
    let (lx, ly) = rotate(x - place.x, y - place.y, -place.rotation);
    let (hx, hy) = half_extent(item, place);
    match item.shape {
        Shape::Fan => lx * lx + ly * ly <= hx * hx,
        _ => lx.abs() <= hx && ly.abs() <= hy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(pieces: Vec<Piece>) -> Config {
        let mut c = Config::default();
        c.zone_pieces.insert("mb/ch2".into(), pieces);
        c
    }

    fn zones() -> Vec<LightZone> {
        vec![
            LightZone { id: "ch0".into(), name: "RGB LED 1".into(), leds: 1 },
            LightZone { id: "ch2".into(), name: "ARGB 1".into(), leds: 20 },
        ]
    }

    #[test]
    fn pieces_and_copies_become_items() {
        let c = cfg_with(vec![
            Piece { kind: PieceKind::Fan, start: 0, leds: 8, copies: 3, ..Default::default() },
            Piece { kind: PieceKind::Strip, start: 8, leds: 12, ..Default::default() },
        ]);
        let it = items(&c, "mb", "Board", &zones(), &[]);
        // RGB LED 1 (undescribed), three copies of the fan, the strip.
        assert_eq!(it.len(), 5);
        assert_eq!(it[1].leds, (1..9).collect::<Vec<_>>());
        assert_eq!(it[3].copy, 2);
        assert_eq!(it[3].leds, it[1].leds);
        assert_eq!(it[4].leds.first(), Some(&9));
        assert_eq!(it[4].key, "mb/ch2/8/0");
    }

    #[test]
    fn hidden_zones_have_no_items() {
        let mut c = cfg_with(Vec::new());
        c.hidden_zones.insert("mb/ch0".into());
        let it = items(&c, "mb", "Board", &zones(), &[]);
        assert_eq!(it.len(), 1);
        assert_eq!(it[0].key, "mb/ch2");
    }

    #[test]
    fn fan_leds_go_round_from_the_top() {
        let item = Item {
            key: "k".into(),
            device: "d".into(),
            name: "f".into(),
            title: "f".into(),
            channel: String::new(),
            shape: Shape::Fan, leds: (0..4).collect(), copy: 0, reverse: false };
        let place = Place { x: 100.0, y: 100.0, size: 100.0, rotation: 0.0 };
        let p = |j| {
            let (x, y) = led_point(&item, &place, j);
            (x.round(), y.round())
        };
        assert_eq!(p(0), (100.0, 62.0)); // top
        assert_eq!(p(1), (138.0, 100.0)); // right: clockwise
        let turned = Place { rotation: 90.0, ..place };
        let (x, y) = led_point(&item, &turned, 0);
        assert_eq!((x.round(), y.round()), (138.0, 100.0));
        assert!(hit(&item, &place, 100.0, 140.0));
        assert!(!hit(&item, &place, 100.0, 160.0));
    }

    #[test]
    fn placed_devices_get_layout_positions() {
        let mut c = cfg_with(vec![Piece { kind: PieceKind::Fan, start: 0, leds: 4, copies: 2, ..Default::default() }]);
        let z = vec![LightZone { id: "ch2".into(), name: "ARGB 1".into(), leds: 4 }];
        assert!(canvas_positions(&c, "mb", &z, &[]).is_none(), "not placed yet");
        let it = items(&c, "mb", "", &z, &[]);
        for (i, p) in places(&c, &it).into_iter().enumerate() {
            c.layout_places.insert(it[i].key.clone(), p);
        }
        c.layout_places.insert("mb/ch2/0/0".into(), Place { x: 500.0, y: 300.0, size: 100.0, rotation: 0.0 });
        let pos = canvas_positions(&c, "mb", &z, &[]).unwrap();
        assert_eq!(pos.len(), 4);
        // LED 1 at the top of the first copy, the next clockwise.
        assert!((pos[0].x - 0.5).abs() < 1e-4 && (pos[0].y - 0.262).abs() < 1e-3, "{:?}", pos[0]);
        assert!(pos[1].x > 0.5);
        assert_eq!(pos[2].local, 0.5);
    }

    #[test]
    fn unplaced_items_are_laid_out_in_rows() {
        let c = cfg_with(vec![Piece { kind: PieceKind::Fan, start: 0, leds: 8, copies: 12, ..Default::default() }]);
        let it = items(&c, "mb", "Board", &zones(), &[]);
        let pl = places(&c, &it);
        assert!(pl.iter().all(|p| p.x > 0.0 && p.x < WIDTH));
        assert!(pl.last().unwrap().y > pl[1].y, "wrapped onto a second row");
    }
}
