//! The Lighting page: one effect for every device ticked there, while the
//! others keep the effect set on their own page.

use crate::config::{Effect, Lighting};
use crate::gui::{section, FerroApp, ACCENT, WARN};
use crate::service::{Command, DeviceView};
use eframe::egui::{self, Color32, RichText};
use opendevicecontrol_core::devices::SOFTWARE_LIGHTING;
use opendevicecontrol_core::{ControlValue, Rgb};
use std::collections::HashMap;
use std::time::Duration;

/// A device's live colours as one compact row of swatches.
fn mini_preview(ui: &mut egui::Ui, lights: &[Rgb]) {
    let n = lights.len().clamp(1, 60);
    let cell = if lights.len() > 30 { 5.0 } else { 9.0 };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(n as f32 * cell + 4.0, 12.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, Color32::from_gray(24));
    // Long rows are sampled, so the strip stays a sensible width.
    for k in 0..n {
        let c = lights.get(k * lights.len().max(1) / n).copied().unwrap_or_default();
        let min = rect.min + egui::vec2(2.0 + k as f32 * cell, 2.0);
        let fill = if c == Rgb::BLACK { Color32::from_gray(45) } else { Color32::from_rgb(c.r, c.g, c.b) };
        painter.rect_filled(egui::Rect::from_min_size(min, egui::vec2(cell - 1.0, 8.0)), 1.5, fill);
    }
}

impl FerroApp {
    /// A small picture of a device's lights: a keyboard as a keyboard,
    /// described zones as their fans and strips, the rest as swatches.
    fn device_preview(&self, ui: &mut egui::Ui, d: &DeviceView) {
        if !d.led_layout.is_empty() {
            crate::gui_peripheral::draw_layout_sized(ui, &d.led_layout, &d.lights, 340.0);
            return;
        }
        let cfg = self.h.config.read();
        let any_pieces = d.zones.iter().any(|z| !cfg.pieces(&d.info.id, &z.id).is_empty());
        if !any_pieces {
            drop(cfg);
            mini_preview(ui, &d.lights);
            return;
        }
        let zones: Vec<(Vec<crate::config::Piece>, std::ops::Range<usize>, bool)> = {
            let mut i = 0;
            d.zones
                .iter()
                .map(|z| {
                    let r = i..(i + z.leds).min(d.lights.len());
                    i += z.leds;
                    (cfg.pieces(&d.info.id, &z.id), r, cfg.hidden_zones.contains(&format!("{}/{}", d.info.id, z.id)))
                })
                .collect()
        };
        drop(cfg);
        ui.horizontal_wrapped(|ui| {
            for (pieces, r, hidden) in zones {
                if hidden {
                    continue;
                }
                let colors = d.lights.get(r).unwrap_or(&[]);
                if pieces.is_empty() {
                    mini_preview(ui, colors);
                } else {
                    crate::gui_layout::draw_pieces_sized(ui, &pieces, colors, 30.0);
                }
            }
        });
    }

    pub(crate) fn lighting_page(&mut self, ui: &mut egui::Ui) {
        let devices: Vec<DeviceView> = {
            let s = self.h.shared.read();
            let mut v: Vec<DeviceView> = s.devices.values().filter(|d| !d.zones.is_empty()).cloned().collect();
            v.sort_by_key(|d| d.info.display_name());
            v
        };
        ui.heading("Lighting");
        ui.label(
            RichText::new(
                "Tick the devices that should run one effect together. The others keep the effect set on their own page.",
            )
            .weak(),
        );
        ui.add_space(8.0);
        if devices.is_empty() {
            ui.label("No devices with lighting found.");
            return;
        }

        let synced: Vec<bool> = {
            let c = self.h.config.read();
            devices.iter().map(|d| c.synced(&d.info.id)).collect()
        };
        let mut open = None;
        section(ui, "Devices", |ui| {
            egui::Grid::new("sync_devices").num_columns(3).spacing([14.0, 8.0]).show(ui, |ui| {
                for (d, &on) in devices.iter().zip(&synced) {
                    let mut v = on;
                    let label = RichText::new(d.info.display_name()).strong();
                    if ui.checkbox(&mut v, label).on_hover_text("Run the shared effect below on this device").changed() {
                        self.set_synced(d, v);
                    }
                    self.device_preview(ui, d);
                    let status = if !d.online {
                        RichText::new("offline").weak()
                    } else if on {
                        RichText::new("synced").color(ACCENT)
                    } else {
                        RichText::new("own effect").weak()
                    };
                    ui.horizontal(|ui| {
                        ui.label(status);
                        if ui.small_button("Open page").clicked() {
                            open = Some(d.info.id.clone());
                        }
                    });
                    ui.end_row();
                }
            });
        });
        if let Some(id) = open {
            self.open_device_page(&id);
        }

        ui.add_space(6.0);
        section(ui, "Layout", |ui| {
            ui.label(
                RichText::new(
                    "Arrange your fans, strips and devices like they sit in your PC: drag to move, click to select, \
                     then turn or resize below. Effects on synced devices run across this layout.",
                )
                .small()
                .weak(),
            );
            ui.add_space(4.0);
            self.layout_canvas(ui, &devices);
        });

        ui.add_space(6.0);
        let mut l = self.h.config.read().lighting_sync.lighting.clone();
        let before = l.clone();
        section(ui, "Shared effect", |ui| {
            if !synced.iter().any(|&s| s) {
                ui.label(RichText::new("Tick a device above to use it.").small().weak());
            }
            let mut sound = l.effect.is_sound();
            let was = sound;
            ui.horizontal(|ui| {
                ui.selectable_value(&mut sound, false, RichText::new("OpenDeviceControl effects").strong());
                ui.selectable_value(&mut sound, true, RichText::new("Sound reactive").strong());
            });
            if sound != was {
                // Back to the last effect used in that family.
                let names = if sound { Effect::SOUND_NAMES } else { Effect::LIGHT_NAMES };
                let next = match l.other.take() {
                    Some(e) if e.is_sound() == sound => e,
                    _ => l.effect.switch_to(names[0]).unwrap_or_else(|| l.effect.clone()),
                };
                l.other = Some(std::mem::replace(&mut l.effect, next));
            }
            ui.add_space(4.0);
            if sound {
                let a = self.h.audio.frame();
                let msg = match &a.error {
                    Some(e) => RichText::new(format!("⚠ {e}")).color(WARN),
                    None => RichText::new("Follows what your computer is playing.").small().weak(),
                };
                ui.label(msg);
                ui.ctx().request_repaint_after(Duration::from_millis(50));
            }
            // Per-zone colours don't apply across devices: one colour.
            self.effect_editor(ui, &mut l, sound, &[], &HashMap::new());
        });
        if l != before {
            self.set_shared_lighting(ui, l);
        }
        // The swatches above move with the effect.
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }

    /// The PC layout: every device's items, drawn with their live colours.
    fn layout_canvas(&self, ui: &mut egui::Ui, devices: &[DeviceView]) {
        use crate::layout::{self, Shape};
        let (items, mut places) = {
            let cfg = self.h.config.read();
            let items: Vec<(layout::Item, &DeviceView)> = devices
                .iter()
                .flat_map(|d| layout::items(&cfg, &d.info.id, &d.info.model, &d.zones, &d.led_layout).into_iter().map(move |it| (it, d)))
                .collect();
            let only: Vec<layout::Item> = items.iter().map(|(it, _)| it.clone()).collect();
            let places = layout::places(&cfg, &only);
            (items, places)
        };
        // Items placed automatically get their spot stored, so effects and
        // the next visit see the same layout.
        let unplaced: Vec<(String, crate::config::Place)> = {
            let cfg = self.h.config.read();
            items
                .iter()
                .zip(&places)
                .filter(|((it, _), _)| !cfg.layout_places.contains_key(&it.key))
                .map(|((it, _), p)| (it.key.clone(), *p))
                .collect()
        };
        if !unplaced.is_empty() {
            self.h.update_config(|c| c.layout_places.extend(unplaced));
        }

        let width = ui.available_width().min(1100.0);
        let scale = width / layout::WIDTH;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, layout::HEIGHT * scale), egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 8.0, Color32::from_gray(18));
        painter.rect_stroke(rect, 8.0, egui::Stroke::new(1.0, Color32::from_gray(55)), egui::StrokeKind::Inside);
        let to_screen = |x: f32, y: f32| rect.min + egui::vec2(x * scale, y * scale);
        let to_layout = |p: egui::Pos2| ((p.x - rect.min.x) / scale, (p.y - rect.min.y) / scale);

        let sel_id = egui::Id::unique("layout_selected");
        let drag_id = egui::Id::unique("layout_drag");
        let band_id = egui::Id::unique("layout_band");
        let mut selected: Vec<String> = ui.data(|m| m.get_temp(sel_id)).unwrap_or_default();
        let adding = ui.input(|i| i.modifiers.shift || i.modifiers.command || i.modifiers.ctrl);

        // Click to select (Shift or Ctrl adds/removes), drag a selected item
        // to move the whole selection, drag on empty space to select with a
        // box.
        let under = |p: egui::Pos2, places: &[crate::config::Place]| {
            let (x, y) = to_layout(p);
            items.iter().zip(places).rposition(|((it, _), pl)| layout::hit(it, pl, x, y))
        };
        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                match under(p, &places) {
                    Some(i) => {
                        let key = &items[i].0.key;
                        if !selected.contains(key) {
                            if !adding {
                                selected.clear();
                            }
                            selected.push(key.clone());
                        }
                        // Where the pointer started, and every selected
                        // item's place then.
                        let start: Vec<(usize, f32, f32)> = items
                            .iter()
                            .enumerate()
                            .filter(|(_, (it, _))| selected.contains(&it.key))
                            .map(|(j, _)| (j, places[j].x, places[j].y))
                            .collect();
                        ui.data_mut(|m| m.insert_temp(drag_id, (to_layout(p), start)));
                    }
                    None => {
                        ui.data_mut(|m| m.insert_temp(band_id, p));
                    }
                }
            }
        }
        type Drag = ((f32, f32), Vec<(usize, f32, f32)>);
        let dragging: Option<Drag> = ui.data(|m| m.get_temp(drag_id));
        if let (Some(((x0, y0), start)), Some(p)) = (&dragging, resp.interact_pointer_pos()) {
            if resp.dragged() {
                let (x, y) = to_layout(p);
                let mut cfg = self.h.config.write();
                for &(j, ox, oy) in start {
                    if let Some(pl) = places.get_mut(j) {
                        pl.x = (ox + x - x0).clamp(0.0, layout::WIDTH);
                        pl.y = (oy + y - y0).clamp(0.0, layout::HEIGHT);
                        cfg.layout_places.insert(items[j].0.key.clone(), *pl);
                    }
                }
            }
        }
        let band: Option<egui::Pos2> = ui.data(|m| m.get_temp(band_id));
        let band_rect = band.zip(resp.interact_pointer_pos()).map(|(a, b)| egui::Rect::from_two_pos(a, b));
        if resp.drag_stopped() {
            if dragging.is_some() {
                ui.data_mut(|m| m.remove::<Drag>(drag_id));
                self.h.update_config(|_| {}); // save where they were dropped
            }
            if let Some(r) = band_rect {
                if !adding {
                    selected.clear();
                }
                for ((it, _), pl) in items.iter().zip(&places) {
                    if r.contains(to_screen(pl.x, pl.y)) && !selected.contains(&it.key) {
                        selected.push(it.key.clone());
                    }
                }
                ui.data_mut(|m| m.remove::<egui::Pos2>(band_id));
            }
        }
        if resp.clicked() {
            match resp.interact_pointer_pos().and_then(|p| under(p, &places)) {
                Some(i) => {
                    let key = items[i].0.key.clone();
                    if adding {
                        if let Some(k) = selected.iter().position(|s| *s == key) {
                            selected.remove(k);
                        } else {
                            selected.push(key);
                        }
                    } else {
                        selected = vec![key];
                    }
                }
                None if !adding => selected.clear(),
                None => {}
            }
        }

        // Draw.
        let mut hover_name = None;
        for ((it, d), pl) in items.iter().zip(&places) {
            let colour = |j: usize| it.leds.get(j).and_then(|&l| d.lights.get(l)).copied().unwrap_or_default();
            let fill = |c: Rgb| if c == Rgb::BLACK { Color32::from_gray(50) } else { Color32::from_rgb(c.r, c.g, c.b) };
            let n = it.leds.len();
            match &it.shape {
                Shape::Fan => {
                    let leds: Vec<Rgb> = (0..n).map(colour).collect();
                    crate::gui_layout::paint_fan(&painter, to_screen(pl.x, pl.y), pl.size * scale, &leds, it.reverse, pl.rotation);
                }
                Shape::Strip | Shape::Block(_) => {
                    let (hx, hy) = layout::half_extent(it, pl);
                    let corners: Vec<egui::Pos2> = [(-hx, -hy), (hx, -hy), (hx, hy), (-hx, hy)]
                        .iter()
                        .map(|&(dx, dy)| layout::rotate(dx, dy, pl.rotation))
                        .map(|(dx, dy)| to_screen(pl.x + dx, pl.y + dy))
                        .collect();
                    painter.add(egui::Shape::convex_polygon(corners, Color32::from_gray(26), egui::Stroke::new(1.0, Color32::from_gray(60))));
                    let dot = match &it.shape {
                        Shape::Block(_) => (pl.size / 40.0 * scale).clamp(1.5, 6.0),
                        _ => ((pl.size / n.max(1) as f32) * 0.35 * scale).clamp(1.5, 6.0),
                    };
                    for j in 0..n {
                        let (x, y) = layout::led_point(it, pl, j);
                        painter.circle_filled(to_screen(x, y), dot, fill(colour(j)));
                    }
                }
            }
            // Label under the item: its name, then the channel.
            {
                let (hx, hy) = layout::half_extent(it, pl);
                let (s, c) = pl.rotation.to_radians().sin_cos();
                // Half the height of the turned item, so the label clears it.
                let below = (hx * s).abs() + (hy * c).abs();
                let top = to_screen(pl.x, pl.y + below) + egui::vec2(0.0, 3.0);
                let size = (11.0 * scale * 1.6).clamp(9.0, 12.0);
                painter.text(top, egui::Align2::CENTER_TOP, &it.title, egui::FontId::proportional(size), Color32::from_gray(205));
                if !it.channel.is_empty() && it.channel != it.title {
                    painter.text(
                        top + egui::vec2(0.0, size + 2.0),
                        egui::Align2::CENTER_TOP,
                        &it.channel,
                        egui::FontId::proportional(size - 1.5),
                        Color32::from_gray(130),
                    );
                }
            }
            if selected.contains(&it.key) {
                let (hx, hy) = layout::half_extent(it, pl);
                let side = hx.max(hy) * 2.0 * scale + 8.0;
                let r = egui::Rect::from_center_size(to_screen(pl.x, pl.y), egui::vec2(side, side));
                painter.rect_stroke(r, 4.0, egui::Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
            }
            let hovered = resp.hover_pos().is_some_and(|p| {
                let (x, y) = to_layout(p);
                layout::hit(it, pl, x, y)
            });
            if hovered {
                hover_name = Some(it.name.clone());
            }
        }
        let banding = resp.dragged();
        if let Some(n) = hover_name.take() {
            resp.on_hover_text(n);
        }
        if let (Some(r), true) = (band_rect, banding) {
            painter.rect_filled(r, 2.0, ACCENT.gamma_multiply(0.12));
            painter.rect_stroke(r, 2.0, egui::Stroke::new(1.0, ACCENT), egui::StrokeKind::Inside);
        }
        selected.retain(|k| items.iter().any(|(it, _)| &it.key == k));
        ui.data_mut(|m| m.insert_temp(sel_id, selected.clone()));

        // The selection's settings: turning and resizing apply to each item
        // in place.
        ui.add_space(4.0);
        let sel: Vec<usize> = items.iter().enumerate().filter(|(_, (it, _))| selected.contains(&it.key)).map(|(i, _)| i).collect();
        ui.horizontal_wrapped(|ui| {
            if sel.is_empty() {
                ui.label(
                    RichText::new("Click an item to turn or resize it; drag a box or Shift-click to select several.").small().weak(),
                );
                if ui.small_button("Arrange everything automatically").clicked() {
                    let keys: Vec<String> = items.iter().map(|(it, _)| it.key.clone()).collect();
                    self.h.update_config(|c| c.layout_places.retain(|k, _| !keys.contains(k)));
                }
                return;
            }
            let title = if sel.len() == 1 { items[sel[0]].0.name.clone() } else { format!("{} items selected", sel.len()) };
            ui.label(RichText::new(title).strong());
            ui.separator();
            let mut turn = 0.0;
            ui.label("Turn");
            for (label, by) in [("⟲ 90°", -90.0), ("⟲ 15°", -15.0), ("⟳ 15°", 15.0), ("⟳ 90°", 90.0)] {
                if ui.small_button(label).clicked() {
                    turn = by;
                }
            }
            let mut changed: Vec<(usize, crate::config::Place)> = Vec::new();
            if sel.len() == 1 {
                let i = sel[0];
                let mut p = places[i];
                p.rotation = (p.rotation + turn).rem_euclid(360.0);
                ui.add(egui::DragValue::new(&mut p.rotation).range(0.0..=359.0).speed(1.0).suffix("°"));
                ui.separator();
                ui.label("Size");
                ui.add(egui::DragValue::new(&mut p.size).range(20.0..=900.0).speed(1.0));
                if p != places[i] {
                    changed.push((i, p));
                }
            } else {
                ui.separator();
                ui.label("Size");
                for (label, f) in [("−", 0.9), ("+", 1.1)] {
                    if ui.small_button(label).on_hover_text("Make every selected item smaller / larger").clicked() {
                        changed.extend(sel.iter().map(|&i| (i, crate::config::Place { size: (places[i].size * f).clamp(20.0, 900.0), ..places[i] })));
                    }
                }
                if turn != 0.0 {
                    changed = sel.iter().map(|&i| (i, crate::config::Place { rotation: (places[i].rotation + turn).rem_euclid(360.0), ..places[i] })).collect();
                }
            }
            ui.separator();
            let reset = ui.small_button("Back to automatic").on_hover_text("Let the layout place the selection again").clicked();
            if ui.small_button("Clear selection").clicked() {
                ui.data_mut(|m| m.remove::<Vec<String>>(sel_id));
            }
            let keys: Vec<String> = sel.iter().map(|&i| items[i].0.key.clone()).collect();
            if reset {
                self.h.update_config(|c| c.layout_places.retain(|k, _| !keys.contains(k)));
            } else if !changed.is_empty() {
                self.h.update_config(|c| {
                    for (i, p) in changed {
                        c.layout_places.insert(items[i].0.key.clone(), p);
                    }
                });
            }
        });
    }

    /// Start or stop syncing `d`. A device showing its built-in effect is
    /// switched to OpenDeviceControl lighting when it joins.
    pub(crate) fn set_synced(&self, d: &DeviceView, on: bool) {
        let id = d.info.id.clone();
        self.h.update_config(|c| {
            if on {
                // The first device to join brings its effect along.
                if c.lighting_sync.devices.is_empty() {
                    c.lighting_sync.lighting = c.lighting.get(&id).cloned().unwrap_or_default();
                }
                c.lighting_sync.devices.insert(id.clone());
            } else {
                c.lighting_sync.devices.remove(&id);
            }
        });
        let built_in = d.controls.iter().any(|c| c.id == SOFTWARE_LIGHTING && c.value == ControlValue::Bool(false));
        if on && built_in {
            let _ = self.h.cmd.send(Command::SetControl { device: id, id: SOFTWARE_LIGHTING.into(), value: ControlValue::Bool(true) });
        }
    }

    /// Apply the shared effect at once; write it to disk once the pointer
    /// is released (sliders and colour pickers change every frame).
    fn set_shared_lighting(&self, ui: &egui::Ui, l: Lighting) {
        if ui.input(|i| i.pointer.any_down()) {
            self.h.config.write().lighting_sync.lighting = l;
            let _ = self.h.cmd.send(Command::ConfigChanged);
        } else {
            self.h.update_config(|c| c.lighting_sync.lighting = l);
        }
    }
}
