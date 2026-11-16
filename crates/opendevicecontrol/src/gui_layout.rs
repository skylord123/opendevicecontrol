//! What is plugged into each lighting zone: fans (rings of LEDs) and strips,
//! each a run of the zone's LEDs. Drawn in the device's preview, and edited
//! on its lighting tab.
//!
//! LED numbers are shown from 1, as people count them; stored from 0.

use crate::config::{Effect, Lighting, Piece, PieceKind};
use crate::gui::{FerroApp, WARN};
use eframe::egui::{self, Color32, RichText};
use opendevicecontrol_core::{LightZone, Rgb};

fn to_c32(c: Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

fn led_fill(c: Rgb) -> Color32 {
    if c == Rgb::BLACK { Color32::from_gray(45) } else { to_c32(c) }
}

pub use crate::layout::piece_label;

fn copies_help(kind: PieceKind) -> &'static str {
    match kind {
        PieceKind::Fan => "How many fans show these same LEDs: fans on a splitter all light alike",
        _ => "How many of these show the same LEDs",
    }
}

/// "LEDs 9–16" (or "LED 9").
fn led_range(p: &Piece) -> String {
    if p.leds <= 1 { format!("LED {}", p.start + 1) } else { format!("LEDs {}–{}", p.start + 1, p.start + p.leds) }
}

/// The first LED not used by any piece, and how many free LEDs follow it.
fn first_gap(pieces: &[Piece], zone_leds: usize) -> (usize, usize) {
    let used = |i: usize| pieces.iter().any(|p| i >= p.start && i < p.start + p.leds);
    let start = (0..zone_leds).find(|&i| !used(i)).unwrap_or(zone_leds);
    let len = (start..zone_leds).take_while(|&i| !used(i)).count();
    (start, len)
}

/// Whether a piece reaches past the zone or onto another piece.
fn overlaps(pieces: &[Piece], i: usize) -> bool {
    let p = &pieces[i];
    pieces.iter().enumerate().any(|(j, q)| j != i && p.start < q.start + q.leds && q.start < p.start + p.leds)
}

/// Draw one zone's LEDs as its pieces: every copy of a fan as a ring, a
/// strip as a bar; LEDs no piece covers as plain rows.
pub fn draw_pieces(ui: &mut egui::Ui, pieces: &[Piece], colors: &[Rgb]) {
    draw_pieces_sized(ui, pieces, colors, 56.0);
}

/// [`draw_pieces`] with fans `fan` points across.
pub fn draw_pieces_sized(ui: &mut egui::Ui, pieces: &[Piece], colors: &[Rgb], fan: f32) {
    let led = |i: usize| colors.get(i).copied().unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(10.0, 6.0);
        let mut covered = vec![false; colors.len()];
        for (k, p) in pieces.iter().enumerate() {
            let leds: Vec<Rgb> = (p.start..p.start + p.leds).map(led).collect();
            for i in p.start..(p.start + p.leds).min(covered.len()) {
                covered[i] = true;
            }
            let label = format!("{} ({})", piece_label(pieces, k), led_range(p));
            for copy in 0..p.copies.max(1) {
                let hover = if p.copies > 1 { format!("{label}, copy {} of {}", copy + 1, p.copies) } else { label.clone() };
                match p.kind {
                    PieceKind::Fan => draw_fan(ui, &leds, p.reverse, 0.0, fan, &hover),
                    PieceKind::Strip | PieceKind::Other => draw_strip(ui, &leds, p.reverse, &hover),
                }
            }
        }
        // Runs of LEDs no piece describes.
        let mut i = 0;
        while i < covered.len() {
            if covered[i] {
                i += 1;
                continue;
            }
            let start = i;
            while i < covered.len() && !covered[i] {
                i += 1;
            }
            let rest: Vec<Rgb> = (start..i).map(led).collect();
            let hover = format!("LEDs {}–{}: not described yet", start + 1, i);
            draw_strip(ui, &rest, false, &hover);
        }
    });
}

/// A fan: LED 1 at `rotation` degrees clockwise from the top, the rest
/// evenly around (clockwise unless `reverse`).
pub fn draw_fan(ui: &mut egui::Ui, leds: &[Rgb], reverse: bool, rotation: f32, size: f32, hover: &str) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    paint_fan(ui.painter(), rect.center(), size, leds, reverse, rotation);
    resp.on_hover_text(hover);
}

pub fn paint_fan(painter: &egui::Painter, centre: egui::Pos2, size: f32, leds: &[Rgb], reverse: bool, rotation: f32) {
    let r = size * 0.38;
    painter.circle_filled(centre, size * 0.47, Color32::from_gray(24));
    painter.circle_stroke(centre, size * 0.47, egui::Stroke::new(1.0, Color32::from_gray(60)));
    painter.circle_stroke(centre, size * 0.16, egui::Stroke::new(1.0, Color32::from_gray(55)));
    let n = leds.len().max(1);
    let dot = (std::f32::consts::PI * r / n as f32 * 0.8).clamp(2.0, size * 0.09);
    for (j, &c) in leds.iter().enumerate() {
        let step = j as f32 / n as f32 * 360.0;
        let deg = rotation + if reverse { -step } else { step };
        let a = deg.to_radians();
        let p = centre + egui::vec2(a.sin(), -a.cos()) * r;
        painter.circle_filled(p, dot, led_fill(c));
    }
}

fn draw_strip(ui: &mut egui::Ui, leds: &[Rgb], reverse: bool, hover: &str) {
    let n = leds.len().max(1);
    let cell = if n > 24 { 6.0 } else { 10.0 };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(n as f32 * cell + 6.0, cell + 8.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, Color32::from_gray(24));
    for (j, &c) in leds.iter().enumerate() {
        let k = if reverse { n - 1 - j } else { j };
        let min = rect.min + egui::vec2(3.0 + k as f32 * cell, 4.0);
        painter.rect_filled(egui::Rect::from_min_size(min, egui::vec2(cell - 1.5, cell)), 2.0, led_fill(c));
    }
    resp.on_hover_text(hover);
}

/// The "add a fan / strip" dialog, while open.
#[derive(Clone)]
struct Adding {
    /// `"<device id>/<zone id>"`.
    key: String,
    piece: Piece,
    focus: bool,
}

fn adding_id() -> egui::Id {
    egui::Id::unique("add_piece_dialog")
}

/// Effects / Sound reactive tabs for `l`; switching brings back the last
/// effect used in the other family.
pub(crate) fn family_tabs(ui: &mut egui::Ui, l: &mut Lighting) -> bool {
    let was = l.effect.is_sound();
    let mut sound = was;
    ui.horizontal(|ui| {
        ui.selectable_value(&mut sound, false, RichText::new("OpenDeviceControl effects").strong());
        ui.selectable_value(&mut sound, true, RichText::new("Sound reactive").strong());
    });
    if sound != was {
        let names = if sound { Effect::SOUND_NAMES } else { Effect::LIGHT_NAMES };
        let next = match l.other.take() {
            Some(e) if e.is_sound() == sound => e,
            _ => l.effect.switch_to(names[0]).unwrap_or_else(|| l.effect.clone()),
        };
        l.other = Some(std::mem::replace(&mut l.effect, next));
    }
    sound
}

impl FerroApp {
    /// A box per channel, under the device's effect: the channel can run
    /// an effect of its own, and its fans and strips can each run their
    /// own. `zones`: the visible zones with the names to show.
    pub(crate) fn channel_effects(&self, ui: &mut egui::Ui, dev: &str, zones: &[(LightZone, String)]) {
        ui.add_space(8.0);
        ui.label(RichText::new("Channels").strong());
        ui.label(
            RichText::new("Each channel follows the effect above unless it has its own; its fans and strips can each have one too.")
                .small()
                .weak(),
        );
        ui.add_space(4.0);
        let cols = if ui.available_width() > 720.0 { 2 } else { 1 };
        for row in zones.chunks(cols) {
            ui.columns(cols, |columns| {
                for (col, (z, name)) in columns.iter_mut().zip(row) {
                    crate::gui::section(col, "", |ui| self.channel_box(ui, dev, z, name));
                }
            });
            ui.add_space(4.0);
        }
    }

    fn channel_box(&self, ui: &mut egui::Ui, dev: &str, z: &LightZone, name: &str) {
        let key = format!("{dev}/{}", z.id);
        ui.push_id(("channel", &key), |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(name).strong());
                if name != z.name {
                    ui.label(RichText::new(&z.name).small().weak()).on_hover_text("The device's name for this channel");
                }
            });
            let (own, pieces, split) = {
                let cfg = self.h.config.read();
                let pieces = cfg.pieces(dev, &z.id);
                let split = pieces.iter().any(|p| cfg.zone_lighting.contains_key(&format!("{key}/{}", p.start)));
                (cfg.zone_lighting.contains_key(&key), pieces, split)
            };

            let mut on = own;
            if ui.checkbox(&mut on, "Own effect").on_hover_text("Run an effect of its own instead of the device's").changed() {
                let start = self.h.config.read().lighting(dev);
                self.h.update_config(|c| {
                    if on {
                        c.zone_lighting.insert(key.clone(), start);
                    } else {
                        c.zone_lighting.remove(&key);
                    }
                });
            }
            if on {
                ui.indent("own", |ui| self.own_effect_editor(ui, &key));
            }

            if pieces.is_empty() {
                return;
            }
            let mut sep = split;
            let what = if pieces.iter().all(|p| p.kind == PieceKind::Fan) { "Separate fans" } else { "Separate fans and strips" };
            if ui.checkbox(&mut sep, what).on_hover_text("Give each fan or strip on this channel an effect of its own").changed() {
                let base = {
                    let cfg = self.h.config.read();
                    cfg.zone_lighting.get(&key).cloned().unwrap_or_else(|| cfg.lighting(dev))
                };
                self.h.update_config(|c| {
                    for p in &pieces {
                        let k = format!("{key}/{}", p.start);
                        if sep {
                            c.zone_lighting.entry(k).or_insert_with(|| base.clone());
                        } else {
                            c.zone_lighting.remove(&k);
                        }
                    }
                });
            }
            if !sep {
                return;
            }
            for (k, p) in pieces.iter().enumerate() {
                let pkey = format!("{key}/{}", p.start);
                egui::Frame::group(ui.style()).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.push_id(("piece_effect", &pkey), |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(piece_label(&pieces, k)).strong());
                            ui.label(RichText::new(led_range(p)).small().weak());
                        });
                        self.own_effect_editor(ui, &pkey);
                    });
                });
            }
        });
    }

    /// The effect stored for `key` in `zone_lighting` (a channel or a
    /// piece), edited in place; nothing while there is none.
    fn own_effect_editor(&self, ui: &mut egui::Ui, key: &str) {
        let Some(mut l) = self.h.config.read().zone_lighting.get(key).cloned() else { return };
        let before = l.clone();
        let sound = family_tabs(ui, &mut l);
        if sound {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
        }
        self.effect_editor(ui, &mut l, sound, &[], &std::collections::HashMap::new());
        if l == before {
            return;
        }
        if ui.input(|i| i.pointer.any_down()) {
            self.h.config.write().zone_lighting.insert(key.to_string(), l);
            let _ = self.h.cmd.send(crate::service::Command::ConfigChanged);
        } else {
            self.h.update_config(|c| {
                c.zone_lighting.insert(key.to_string(), l);
            });
        }
    }

    /// The "What's plugged in" editor for one zone: its pieces by start
    /// LED, and buttons to add more.
    pub(crate) fn pieces_editor(&self, ui: &mut egui::Ui, dev: &str, z: &LightZone, zone_name: &str) {
        let key = format!("{dev}/{}", z.id);
        let mut pieces = self.h.config.read().pieces(dev, &z.id);
        let before = pieces.clone();
        let described: usize = (0..z.leds).filter(|&i| pieces.iter().any(|p| i >= p.start && i < p.start + p.leds)).count();

        ui.horizontal(|ui| {
            ui.label(RichText::new(zone_name).strong());
            let text = if described == z.leds {
                format!("all {} LEDs described", z.leds)
            } else {
                format!("{described} of {} LEDs described", z.leds)
            };
            ui.label(RichText::new(text).small().weak());
        });

        let mut remove = None;
        for i in 0..pieces.len() {
            let label = piece_label(&pieces, i);
            let bad = overlaps(&pieces, i) || pieces[i].start + pieces[i].leds > z.leds;
            let p = &mut pieces[i];
            ui.push_id(("piece", &key, i), |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&label).strong()).on_hover_text(p.kind.name());
                    ui.label(RichText::new(led_range(p)).weak());
                    if bad {
                        ui.label(RichText::new("⚠").color(WARN))
                            .on_hover_text("Overlaps another piece, or runs past the zone's last LED");
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    let mut first = p.start + 1;
                    ui.label("Start");
                    if ui.add(egui::DragValue::new(&mut first).range(1..=z.leds.max(1))).changed() {
                        p.start = first - 1;
                    }
                    ui.add(egui::DragValue::new(&mut p.leds).range(1..=z.leds.max(1)).suffix(" LEDs"));
                    ui.label(if p.kind == PieceKind::Fan { "Fans" } else { "Copies" });
                    ui.add(egui::DragValue::new(&mut p.copies).range(1..=12))
                        .on_hover_text(copies_help(p.kind));
                    let dir = if p.kind == PieceKind::Fan { "Counter-clockwise" } else { "Reversed" };
                    ui.checkbox(&mut p.reverse, dir).on_hover_text("The LEDs run the other way");
                    ui.add(egui::TextEdit::singleline(&mut p.name).hint_text("Name").desired_width(120.0));
                    if ui.small_button("🗑").on_hover_text("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            });
        }
        if let Some(i) = remove {
            pieces.remove(i);
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new("Add:").weak());
            for kind in PieceKind::ALL {
                if ui.button(kind.name()).clicked() {
                    let (start, free) = first_gap(&pieces, z.leds);
                    let leds = match kind {
                        PieceKind::Fan => 8.min(free.max(1)),
                        _ => free.max(1),
                    };
                    let st = Adding { key: key.clone(), piece: Piece { kind, start: start.min(z.leds.saturating_sub(1)), leds, ..Default::default() }, focus: true };
                    ui.data_mut(|m| m.insert_temp(adding_id(), st));
                }
            }
        });

        if let Some(new) = self.add_dialog(ui.ctx(), &key, z.leds, &pieces) {
            pieces.push(new);
        }

        if pieces != before {
            pieces.sort_by_key(|p| p.start);
            self.h.update_config(|c| {
                if pieces.is_empty() {
                    c.zone_pieces.remove(&key);
                } else {
                    c.zone_pieces.insert(key.clone(), pieces.clone());
                }
            });
        }
    }

    /// The add dialog for zone `key`, if it is open: the new piece once
    /// added.
    fn add_dialog(&self, ctx: &egui::Context, key: &str, zone_leds: usize, pieces: &[Piece]) -> Option<Piece> {
        let mut st = ctx.data(|m| m.get_temp::<Adding>(adding_id())).filter(|s| s.key == key)?;
        // None: still open; Some(None): cancelled; Some(Some(p)): added.
        let mut done: Option<Option<Piece>> = None;
        let modal = egui::Modal::new(adding_id().with("modal")).show(ctx, |ui| {
            ui.set_width(330.0);
            let p = &mut st.piece;
            ui.heading(format!("Add a {}", p.kind.name().to_lowercase()));
            ui.add_space(6.0);
            egui::Grid::new("add_piece").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("LEDs");
                let r = ui.add(egui::DragValue::new(&mut p.leds).range(1..=zone_leds.max(1)));
                if std::mem::take(&mut st.focus) {
                    r.request_focus();
                }
                ui.end_row();
                ui.label("Starts at LED");
                let mut first = p.start + 1;
                if ui.add(egui::DragValue::new(&mut first).range(1..=zone_leds.max(1))).changed() {
                    p.start = first - 1;
                }
                ui.end_row();
                ui.label(if p.kind == PieceKind::Fan { "How many fans" } else { "Copies" });
                ui.add(egui::DragValue::new(&mut p.copies).range(1..=12)).on_hover_text(copies_help(p.kind));
                ui.end_row();
                if p.kind == PieceKind::Fan {
                    ui.label("");
                    ui.label(RichText::new("Fans on a splitter all show these same LEDs.").small().weak());
                    ui.end_row();
                }
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut p.name).hint_text("Optional, e.g. Top fan").desired_width(180.0));
                ui.end_row();
            });
            ui.add_space(4.0);
            let end = p.start + p.leds;
            let clash = pieces.iter().any(|q| p.start < q.start + q.leds && q.start < end);
            let note = if end > zone_leds {
                Some(format!("The zone has {zone_leds} LEDs; this would end at LED {end}."))
            } else if clash {
                Some("This overlaps a piece already described.".to_string())
            } else {
                None
            };
            match note {
                Some(n) => ui.label(RichText::new(format!("⚠ {n}")).small().color(WARN)),
                None => ui.label(RichText::new(format!("Uses {}.", led_range(p))).small().weak()),
            };
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Add").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = Some(Some(p.clone()));
                }
                if ui.button("Cancel").clicked() {
                    done = Some(None);
                }
            });
        });
        if modal.should_close() && done.is_none() {
            done = Some(None);
        }
        match done {
            None => {
                ctx.data_mut(|m| m.insert_temp(adding_id(), st));
                None
            }
            Some(r) => {
                ctx.data_mut(|m| m.remove::<Adding>(adding_id()));
                r
            }
        }
    }
}
