//! Small vector icons for the sensor tree, painted with egui shapes so
//! they are crisp at any scale and match the theme (no image assets).

use eframe::egui::{pos2, vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};
use opendevicecontrol_core::{DeviceKind, HardwareKind, SensorKind};

/// Colour of a sensor group, used for its icon, sparklines and graphs.
pub fn category_color(kind: SensorKind) -> Color32 {
    match kind {
        SensorKind::Temperature => Color32::from_rgb(244, 120, 80),
        SensorKind::Voltage => Color32::from_rgb(240, 200, 70),
        SensorKind::Power => Color32::from_rgb(190, 130, 250),
        SensorKind::Current => Color32::from_rgb(245, 150, 200),
        SensorKind::Energy => Color32::from_rgb(240, 180, 40),
        SensorKind::FanSpeed => Color32::from_rgb(70, 195, 235),
        SensorKind::Duty => Color32::from_rgb(90, 160, 245),
        SensorKind::Flow => Color32::from_rgb(80, 140, 250),
        SensorKind::Frequency => Color32::from_rgb(130, 175, 255),
        SensorKind::Load | SensorKind::Efficiency => Color32::from_rgb(120, 210, 110),
        SensorKind::Battery => Color32::from_rgb(150, 220, 90),
        SensorKind::DataSize => Color32::from_rgb(175, 180, 200),
        SensorKind::DataRate => Color32::from_rgb(100, 205, 200),
        SensorKind::Duration => Color32::from_rgb(185, 185, 185),
        SensorKind::Charging | SensorKind::State => Color32::from_rgb(34, 190, 178),
    }
}

pub fn hardware_color(kind: HardwareKind) -> Color32 {
    match kind {
        HardwareKind::Computer => Color32::from_rgb(205, 210, 220),
        HardwareKind::Motherboard => Color32::from_rgb(210, 150, 95),
        HardwareKind::Chip => Color32::from_rgb(185, 185, 195),
        HardwareKind::Cpu => Color32::from_rgb(95, 165, 245),
        HardwareKind::Memory => Color32::from_rgb(175, 125, 245),
        HardwareKind::Gpu => Color32::from_rgb(120, 210, 110),
        HardwareKind::Storage => Color32::from_rgb(240, 185, 50),
        HardwareKind::Network => Color32::from_rgb(34, 190, 178),
        HardwareKind::Battery => Color32::from_rgb(140, 210, 90),
        HardwareKind::PowerSupply => Color32::from_rgb(240, 180, 40),
        HardwareKind::Cooler => Color32::from_rgb(70, 195, 235),
        HardwareKind::Other => Color32::from_rgb(170, 170, 180),
    }
}

/// The hardware kind whose icon stands for a whole device.
pub fn device_hardware(kind: DeviceKind) -> HardwareKind {
    match kind {
        DeviceKind::PowerSupply => HardwareKind::PowerSupply,
        DeviceKind::Cooler | DeviceKind::FanHub => HardwareKind::Cooler,
        DeviceKind::Memory => HardwareKind::Memory,
        DeviceKind::GraphicsCard => HardwareKind::Gpu,
        DeviceKind::Computer => HardwareKind::Computer,
        _ => HardwareKind::Other,
    }
}

fn s(w: f32, c: Color32) -> Stroke {
    Stroke::new(w, c)
}

/// Rect `r` mapped to a 16×16 design grid.
struct Grid(Rect);

impl Grid {
    fn p(&self, x: f32, y: f32) -> Pos2 {
        let k = self.0.width().min(self.0.height()) / 16.0;
        let o = self.0.center() - vec2(8.0 * k, 8.0 * k);
        pos2(o.x + x * k, o.y + y * k)
    }
    fn k(&self) -> f32 {
        self.0.width().min(self.0.height()) / 16.0
    }
    fn r(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_two_pos(self.p(x0, y0), self.p(x1, y1))
    }
}

fn bolt(p: &Painter, g: &Grid, c: Color32) {
    p.add(Shape::convex_polygon(vec![g.p(9.5, 1.5), g.p(4.0, 9.0), g.p(8.0, 9.0)], c, Stroke::NONE));
    p.add(Shape::convex_polygon(vec![g.p(8.0, 7.0), g.p(12.0, 7.0), g.p(6.5, 14.5)], c, Stroke::NONE));
}

fn fan(p: &Painter, g: &Grid, c: Color32, w: f32) {
    let ctr = g.p(8.0, 8.0);
    p.circle_stroke(ctr, 6.5 * g.k(), s(w, c));
    for i in 0..3 {
        let a = i as f32 * std::f32::consts::TAU / 3.0 + 0.4;
        let tip = ctr + vec2(a.cos(), a.sin()) * 5.0 * g.k();
        let side = ctr + vec2((a + 0.9).cos(), (a + 0.9).sin()) * 3.6 * g.k();
        p.add(Shape::convex_polygon(vec![ctr, tip, side], c, Stroke::NONE));
    }
    p.circle_filled(ctr, 1.3 * g.k(), c);
}

fn chip(p: &Painter, g: &Grid, c: Color32, w: f32, all_sides: bool) {
    let body = if all_sides { g.r(3.5, 3.5, 12.5, 12.5) } else { g.r(4.5, 2.5, 11.5, 13.5) };
    p.rect_stroke(body, 1.5 * g.k(), s(w, c), eframe::egui::StrokeKind::Middle);
    for i in 0..3 {
        let t = 5.5 + i as f32 * 2.5;
        if all_sides {
            p.line_segment([g.p(t, 1.0), g.p(t, 3.5)], s(w, c));
            p.line_segment([g.p(t, 12.5), g.p(t, 15.0)], s(w, c));
            p.line_segment([g.p(1.0, t), g.p(3.5, t)], s(w, c));
            p.line_segment([g.p(12.5, t), g.p(15.0, t)], s(w, c));
        } else {
            let t = 4.5 + i as f32 * 3.5;
            p.line_segment([g.p(2.0, t), g.p(4.5, t)], s(w, c));
            p.line_segment([g.p(11.5, t), g.p(14.0, t)], s(w, c));
        }
    }
    if all_sides {
        p.rect_filled(g.r(6.0, 6.0, 10.0, 10.0), 1.0 * g.k(), c);
    }
}

/// Icon of a piece of hardware.
pub fn hardware(p: &Painter, r: Rect, kind: HardwareKind, c: Color32) {
    let g = Grid(r);
    let w = (1.3 * g.k()).max(1.0);
    let sm = eframe::egui::StrokeKind::Middle;
    match kind {
        HardwareKind::Computer => {
            p.rect_stroke(g.r(1.5, 2.5, 14.5, 11.0), 1.5 * g.k(), s(w, c), sm);
            p.line_segment([g.p(8.0, 11.0), g.p(8.0, 13.5)], s(w, c));
            p.line_segment([g.p(4.5, 14.0), g.p(11.5, 14.0)], s(w, c));
        }
        HardwareKind::Motherboard => {
            p.rect_stroke(g.r(1.5, 1.5, 14.5, 14.5), 1.5 * g.k(), s(w, c), sm);
            p.rect_filled(g.r(4.0, 4.0, 8.5, 8.5), 0.5 * g.k(), c);
            for y in [10.5, 12.5] {
                p.line_segment([g.p(4.0, y), g.p(12.5, y)], s(w, c));
            }
            p.rect_stroke(g.r(10.0, 4.0, 12.5, 8.5), 0.0, s(w * 0.8, c), sm);
        }
        HardwareKind::Chip | HardwareKind::Other => chip(p, &g, c, w, false),
        HardwareKind::Cpu => chip(p, &g, c, w, true),
        HardwareKind::Memory => {
            p.rect_stroke(g.r(1.0, 4.0, 15.0, 11.5), 1.0 * g.k(), s(w, c), sm);
            for i in 0..4 {
                let x = 2.5 + i as f32 * 3.2;
                p.rect_filled(g.r(x, 5.5, x + 2.2, 8.5), 0.4 * g.k(), c);
            }
            for i in 0..6 {
                let x = 2.5 + i as f32 * 2.2;
                p.line_segment([g.p(x, 11.5), g.p(x, 13.5)], s(w * 0.8, c));
            }
        }
        HardwareKind::Gpu => {
            p.rect_stroke(g.r(1.0, 3.5, 15.0, 12.0), 1.5 * g.k(), s(w, c), sm);
            p.circle_stroke(g.p(6.0, 7.75), 2.6 * g.k(), s(w, c));
            p.circle_filled(g.p(6.0, 7.75), 0.9 * g.k(), c);
            p.rect_filled(g.r(10.0, 5.5, 13.0, 10.0), 0.5 * g.k(), c);
            p.line_segment([g.p(3.0, 12.0), g.p(3.0, 14.5)], s(w, c));
            p.line_segment([g.p(5.0, 12.0), g.p(10.0, 12.0)], s(w * 1.6, c));
        }
        HardwareKind::Storage => {
            p.rect_stroke(g.r(2.0, 2.0, 14.0, 14.0), 2.0 * g.k(), s(w, c), sm);
            p.circle_stroke(g.p(8.0, 7.0), 3.2 * g.k(), s(w, c));
            p.circle_filled(g.p(8.0, 7.0), 0.9 * g.k(), c);
            p.line_segment([g.p(4.5, 12.0), g.p(9.0, 12.0)], s(w, c));
            p.circle_filled(g.p(11.5, 12.0), 0.9 * g.k(), c);
        }
        HardwareKind::Network => {
            for (i, h) in [4.0f32, 7.5, 11.0].iter().enumerate() {
                let x = 2.5 + i as f32 * 4.0;
                p.rect_filled(g.r(x, 14.0 - h, x + 2.8, 14.0), 0.6 * g.k(), c);
            }
            p.line_segment([g.p(1.5, 14.5), g.p(14.5, 14.5)], s(w * 0.8, c));
        }
        HardwareKind::Battery => battery(p, &g, c, w, 0.7),
        HardwareKind::PowerSupply => {
            p.rect_stroke(g.r(1.5, 2.5, 14.5, 13.5), 1.5 * g.k(), s(w, c), sm);
            bolt(p, &Grid(g.r(3.5, 3.5, 12.5, 12.5)), c);
        }
        HardwareKind::Cooler => fan(p, &g, c, w),
    }
}

fn battery(p: &Painter, g: &Grid, c: Color32, w: f32, level: f32) {
    let sm = eframe::egui::StrokeKind::Middle;
    p.rect_stroke(g.r(1.5, 4.5, 13.0, 11.5), 1.5 * g.k(), s(w, c), sm);
    p.rect_filled(g.r(13.0, 6.5, 14.8, 9.5), 0.5 * g.k(), c);
    p.rect_filled(g.r(3.0, 6.0, 3.0 + 8.5 * level.clamp(0.0, 1.0), 10.0), 0.5 * g.k(), c);
}

/// Icon of a device on the device row of the overview.
pub fn device(p: &Painter, r: Rect, kind: DeviceKind, c: Color32) {
    let g = Grid(r);
    let w = (1.3 * g.k()).max(1.0);
    let sm = eframe::egui::StrokeKind::Middle;
    match kind {
        DeviceKind::Keyboard => {
            p.rect_stroke(g.r(0.8, 4.0, 15.2, 12.5), 1.5 * g.k(), s(w, c), sm);
            for row in 0..2 {
                for col in 0..5 {
                    p.rect_filled(g.r(2.6 + col as f32 * 2.4, 5.8 + row as f32 * 2.2, 3.8 + col as f32 * 2.4, 6.9 + row as f32 * 2.2), 0.2, c);
                }
            }
            p.line_segment([g.p(4.5, 10.8), g.p(11.5, 10.8)], s(w, c));
        }
        DeviceKind::Mouse => {
            p.rect_stroke(g.r(4.0, 1.5, 12.0, 14.5), 4.0 * g.k(), s(w, c), sm);
            p.line_segment([g.p(8.0, 2.0), g.p(8.0, 6.5)], s(w, c));
            p.line_segment([g.p(4.2, 6.5), g.p(11.8, 6.5)], s(w, c));
        }
        DeviceKind::Mousepad => {
            p.rect_stroke(g.r(1.0, 4.5, 15.0, 12.5), 1.5 * g.k(), s(w, c), sm);
            p.line_segment([g.p(1.5, 10.5), g.p(14.5, 10.5)], s(w * 0.8, c));
        }
        DeviceKind::Headset => {
            let pts: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let a = std::f32::consts::PI * (1.0 + i as f32 / 16.0);
                    g.p(8.0 + 5.5 * a.cos(), 9.0 + 6.5 * a.sin())
                })
                .collect();
            p.add(Shape::line(pts, s(w, c)));
            p.rect_filled(g.r(1.5, 8.5, 4.5, 14.0), 1.0 * g.k(), c);
            p.rect_filled(g.r(11.5, 8.5, 14.5, 14.0), 1.0 * g.k(), c);
        }
        other => hardware(p, r, device_hardware(other), c),
    }
}

/// Icon of a sensor group.
pub fn category(p: &Painter, r: Rect, kind: SensorKind, c: Color32) {
    let g = Grid(r);
    let w = (1.4 * g.k()).max(1.0);
    let sm = eframe::egui::StrokeKind::Middle;
    match kind {
        SensorKind::Temperature => {
            p.line_segment([g.p(8.0, 2.5), g.p(8.0, 10.0)], s(3.4 * g.k(), c));
            p.circle_filled(g.p(8.0, 11.8), 2.9 * g.k(), c);
            for y in [4.5, 6.5, 8.5] {
                p.line_segment([g.p(10.5, y), g.p(12.5, y)], s(w * 0.8, c));
            }
        }
        SensorKind::Voltage => bolt(p, &g, c),
        SensorKind::Power => {
            let pts: Vec<Pos2> = (0..=20)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2 + 0.75 + i as f32 / 20.0 * (std::f32::consts::TAU - 1.5);
                    g.p(8.0 + 5.5 * a.cos(), 8.5 + 5.5 * a.sin())
                })
                .collect();
            p.add(Shape::line(pts, s(w * 1.2, c)));
            p.line_segment([g.p(8.0, 1.5), g.p(8.0, 8.0)], s(w * 1.2, c));
        }
        SensorKind::Current => {
            let pts: Vec<Pos2> = (0..=24)
                .map(|i| {
                    let x = 1.5 + i as f32 / 24.0 * 13.0;
                    g.p(x, 8.0 - 4.0 * (i as f32 / 24.0 * std::f32::consts::TAU).sin())
                })
                .collect();
            p.add(Shape::line(pts, s(w * 1.2, c)));
        }
        SensorKind::Energy => {
            p.circle_stroke(g.p(8.0, 8.0), 6.8 * g.k(), s(w, c));
            bolt(p, &Grid(g.r(3.0, 3.0, 13.0, 13.0)), c);
        }
        SensorKind::FanSpeed => fan(p, &g, c, w),
        SensorKind::Duty => {
            for (y, x) in [(4.5, 10.0), (8.0, 5.0), (11.5, 8.5)] {
                p.line_segment([g.p(1.5, y), g.p(14.5, y)], s(w, c));
                p.circle_filled(g.p(x, y), 2.0 * g.k(), c);
            }
        }
        SensorKind::Flow => {
            p.add(Shape::convex_polygon(vec![g.p(8.0, 1.5), g.p(12.3, 9.0), g.p(3.7, 9.0)], c, Stroke::NONE));
            p.circle_filled(g.p(8.0, 10.0), 4.4 * g.k(), c);
        }
        SensorKind::Frequency => {
            p.circle_stroke(g.p(8.0, 8.0), 6.3 * g.k(), s(w, c));
            p.line_segment([g.p(8.0, 8.0), g.p(8.0, 4.0)], s(w * 1.2, c));
            p.line_segment([g.p(8.0, 8.0), g.p(11.0, 9.5)], s(w * 1.2, c));
        }
        SensorKind::Load | SensorKind::Efficiency => {
            let pts: Vec<Pos2> = (0..=20)
                .map(|i| {
                    let a = std::f32::consts::PI * (1.0 + i as f32 / 20.0);
                    g.p(8.0 + 6.3 * a.cos(), 11.5 + 6.3 * a.sin())
                })
                .collect();
            p.add(Shape::line(pts, s(w * 1.2, c)));
            p.line_segment([g.p(8.0, 11.5), g.p(11.5, 6.5)], s(w * 1.3, c));
            p.circle_filled(g.p(8.0, 11.5), 1.5 * g.k(), c);
        }
        SensorKind::Battery => battery(p, &g, c, w, 0.6),
        SensorKind::DataSize => {
            for y in [3.0, 7.0, 11.0] {
                p.rect_stroke(g.r(2.5, y, 13.5, y + 3.0), 1.5 * g.k(), s(w, c), sm);
            }
        }
        SensorKind::DataRate => {
            p.line_segment([g.p(5.0, 14.0), g.p(5.0, 3.0)], s(w * 1.2, c));
            p.add(Shape::convex_polygon(vec![g.p(5.0, 1.5), g.p(8.0, 5.0), g.p(2.0, 5.0)], c, Stroke::NONE));
            p.line_segment([g.p(11.0, 2.0), g.p(11.0, 13.0)], s(w * 1.2, c));
            p.add(Shape::convex_polygon(vec![g.p(11.0, 14.5), g.p(8.0, 11.0), g.p(14.0, 11.0)], c, Stroke::NONE));
        }
        SensorKind::Duration => {
            p.add(Shape::convex_polygon(vec![g.p(3.5, 2.0), g.p(12.5, 2.0), g.p(8.0, 8.0)], c, Stroke::NONE));
            p.add(Shape::convex_polygon(vec![g.p(8.0, 8.0), g.p(12.5, 14.0), g.p(3.5, 14.0)], Color32::TRANSPARENT, s(w, c)));
        }
        SensorKind::Charging | SensorKind::State => {
            p.rect_stroke(g.r(1.5, 4.5, 14.5, 11.5), 3.5 * g.k(), s(w, c), sm);
            p.circle_filled(g.p(11.0, 8.0), 2.3 * g.k(), c);
        }
    }
}

/// Home Assistant marker: a house, filled when the sensor is sent, an
/// outline when not, half-filled for groups where only some are.
pub fn house(p: &Painter, r: Rect, state: Option<bool>, on: Color32, off: Color32) {
    let g = Grid(r);
    let w = (1.3 * g.k()).max(1.0);
    let roof = vec![g.p(8.0, 1.5), g.p(15.0, 8.0), g.p(1.0, 8.0)];
    let body = g.r(3.0, 7.5, 13.0, 14.5);
    match state {
        Some(true) => {
            p.add(Shape::convex_polygon(roof, on, Stroke::NONE));
            p.rect_filled(body, 0.5, on);
            p.rect_filled(g.r(6.5, 10.0, 9.5, 14.5), 0.5, Color32::from_black_alpha(140));
        }
        Some(false) => {
            p.add(Shape::closed_line(vec![g.p(8.0, 1.5), g.p(15.0, 8.0), g.p(13.0, 8.0), g.p(13.0, 14.5), g.p(3.0, 14.5), g.p(3.0, 8.0), g.p(1.0, 8.0)], s(w, off)));
        }
        None => {
            p.add(Shape::closed_line(vec![g.p(8.0, 1.5), g.p(15.0, 8.0), g.p(13.0, 8.0), g.p(13.0, 14.5), g.p(3.0, 14.5), g.p(3.0, 8.0), g.p(1.0, 8.0)], s(w, on)));
            p.rect_filled(g.r(3.0, 10.5, 13.0, 14.5), 0.5, on);
        }
    }
}

/// Expand / collapse arrow.
pub fn chevron(p: &Painter, r: Rect, open: bool, c: Color32) {
    let g = Grid(r);
    let pts = if open { vec![g.p(4.0, 6.0), g.p(12.0, 6.0), g.p(8.0, 11.0)] } else { vec![g.p(6.0, 4.0), g.p(11.0, 8.0), g.p(6.0, 12.0)] };
    p.add(Shape::convex_polygon(pts, c, Stroke::NONE));
}
