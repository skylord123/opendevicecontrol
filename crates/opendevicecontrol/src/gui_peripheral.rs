//! Page for keyboards, mice and other peripherals: lighting, programmable
//! keys and on/off settings. Built from the generic device model, so new
//! peripherals get a working page without UI code.

use crate::config::{Effect, HexColor, KeyAction, Lighting};
use crate::gui::{section, FerroApp, ACCENT, BAD, WARN};
use crate::service::{Command, DeviceView};
use eframe::egui::{self, Color32, RichText};
use opendevicecontrol_core::devices::{HW_LIGHTING_PREFIX, SOFTWARE_LIGHTING};
use opendevicecontrol_core::{Control, ControlKind, ControlValue, LedShape, Rgb};
use std::time::Duration;

/// Draw a device's LEDs where they physically sit (e.g. a keyboard's keys),
/// each filled with its live colour, scaled to the available width.
fn draw_layout(ui: &mut egui::Ui, shapes: &[LedShape], lights: &[Rgb]) {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for s in shapes {
        let [x, y, w, h] = s.rect;
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + w), y1.max(y + h));
    }
    let (bw, bh) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
    let pad = 10.0;
    let width = ui.available_width().min(900.0);
    let scale = ((width - 2.0 * pad) / bw).max(0.1);
    let size = egui::vec2(bw * scale + 2.0 * pad, bh * scale + 2.0 * pad);
    let (area, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(area);
    // Keyboard case.
    painter.rect_filled(area, 8.0, Color32::from_gray(28));
    painter.rect_stroke(area, 8.0, egui::Stroke::new(1.0, Color32::from_gray(60)), egui::StrokeKind::Inside);
    let font = egui::FontId::proportional((9.0 * scale).clamp(8.0, 13.0));
    let mut hovered = None;
    for s in shapes {
        let [x, y, w, h] = s.rect;
        let min = area.min + egui::vec2(pad + (x - x0) * scale, pad + (y - y0) * scale);
        let rect = egui::Rect::from_min_size(min, egui::vec2(w * scale, h * scale));
        let c = lights.get(s.led).copied().unwrap_or_default();
        let lit = c != Rgb::BLACK;
        let fill = if lit { to_c32(c) } else { Color32::from_gray(45) };
        let round = (3.0 * scale).clamp(2.0, 5.0);
        painter.rect_filled(rect, round, fill);
        painter.rect_stroke(rect, round, egui::Stroke::new(1.0, Color32::from_gray(80)), egui::StrokeKind::Inside);
        if !s.label.is_empty() {
            // Dark text on bright keys, light text on dark ones.
            let luma = 0.299 * c.r as f32 + 0.587 * c.g as f32 + 0.114 * c.b as f32;
            let text = if lit && luma > 140.0 { Color32::from_gray(20) } else { Color32::from_gray(225) };
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, &s.label, font.clone(), text);
        }
        if resp.hover_pos().is_some_and(|p| rect.contains(p)) {
            hovered = Some((s, c));
        }
    }
    if let Some((s, c)) = hovered {
        resp.on_hover_text(format!("{} {}", s.name, c.to_hex()));
    }
    ui.label(RichText::new("live preview").small().weak());
}

/// The device page's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Lighting,
    Keys,
    Settings,
}

impl Tab {
    fn name(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Lighting => "Lighting",
            Tab::Keys => "Keys",
            Tab::Settings => "Settings",
        }
    }
}

/// Where a device's lighting comes from (the lighting section's tabs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Effects,
    Sound,
    BuiltIn,
}

/// The `Device::preview_key` name of an egui key.
fn preview_key_name(k: egui::Key) -> Option<&'static str> {
    use egui::Key as K;
    Some(match k {
        K::Space => "space",
        K::Enter => "enter",
        K::Escape => "escape",
        K::Tab => "tab",
        K::Backspace => "backspace",
        K::ArrowLeft => "left",
        K::ArrowRight => "right",
        K::ArrowUp => "up",
        K::ArrowDown => "down",
        K::Home => "home",
        K::End => "end",
        K::PageUp => "pageup",
        K::PageDown => "pagedown",
        K::Insert => "insert",
        K::Delete => "delete",
        K::Minus => "minus",
        K::Equals => "equals",
        K::Comma => "comma",
        K::Period => "period",
        K::Slash => "slash",
        K::Semicolon => "semicolon",
        K::Quote => "quote",
        K::Backtick => "backtick",
        K::OpenBracket => "leftbracket",
        K::CloseBracket => "rightbracket",
        K::Backslash => "backslash",
        K::F1 => "f1",
        K::F2 => "f2",
        K::F3 => "f3",
        K::F4 => "f4",
        K::F5 => "f5",
        K::F6 => "f6",
        K::F7 => "f7",
        K::F8 => "f8",
        K::F9 => "f9",
        K::F10 => "f10",
        K::F11 => "f11",
        K::F12 => "f12",
        K::Num0 => "0",
        K::Num1 => "1",
        K::Num2 => "2",
        K::Num3 => "3",
        K::Num4 => "4",
        K::Num5 => "5",
        K::Num6 => "6",
        K::Num7 => "7",
        K::Num8 => "8",
        K::Num9 => "9",
        K::A => "a",
        K::B => "b",
        K::C => "c",
        K::D => "d",
        K::E => "e",
        K::F => "f",
        K::G => "g",
        K::H => "h",
        K::I => "i",
        K::J => "j",
        K::K => "k",
        K::L => "l",
        K::M => "m",
        K::N => "n",
        K::O => "o",
        K::P => "p",
        K::Q => "q",
        K::R => "r",
        K::S => "s",
        K::T => "t",
        K::U => "u",
        K::V => "v",
        K::W => "w",
        K::X => "x",
        K::Y => "y",
        K::Z => "z",
        _ => return None,
    })
}

/// Controls shown in the lighting section rather than under Settings.
fn is_lighting_control(id: &str) -> bool {
    id == SOFTWARE_LIGHTING || id.starts_with(HW_LIGHTING_PREFIX)
}

fn to_c32(c: Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

/// Colour button editing a `HexColor`; returns true when it changed.
fn color_button(ui: &mut egui::Ui, c: &mut HexColor) -> bool {
    let mut rgb = [c.0.r, c.0.g, c.0.b];
    let changed = egui::color_picker::color_edit_button_srgb(ui, &mut rgb).changed();
    if changed {
        c.0 = Rgb::new(rgb[0], rgb[1], rgb[2]);
    }
    changed
}

impl FerroApp {
    /// Apply lighting immediately; write it to disk only once the pointer is
    /// released (colour pickers and sliders change every frame while dragged).
    fn set_lighting(&self, ui: &egui::Ui, device: &str, l: Lighting) {
        let dragging = ui.input(|i| i.pointer.any_down());
        if dragging {
            self.h.config.write().lighting.insert(device.to_string(), l);
            let _ = self.h.cmd.send(Command::ConfigChanged);
        } else {
            self.h.update_config(|c| {
                c.lighting.insert(device.to_string(), l);
            });
        }
    }

    /// One device control: checkbox, choice or slider, sent to the device
    /// when changed.
    fn control_widget(&self, ui: &mut egui::Ui, dev: &str, c: &Control) {
        match (&c.kind, &c.value) {
            (ControlKind::Toggle, ControlValue::Bool(on)) => {
                let mut v = *on;
                let r = ui.checkbox(&mut v, &c.name);
                if r.changed() {
                    let _ = self.h.cmd.send(Command::SetControl {
                        device: dev.to_string(),
                        id: c.id.clone(),
                        value: ControlValue::Bool(v),
                    });
                }
                if let Some(h) = c.help.as_deref().filter(|h| !h.is_empty()) {
                    ui.label(RichText::new(h).small().weak());
                }
            }
            (ControlKind::Choice { options }, ControlValue::Choice(cur)) => {
                let mut picked = None;
                // Long lists (e.g. a keyboard's built-in effects)
                // get a drop-down; short ones stay one click away.
                if options.len() > 6 {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&c.name).strong());
                        egui::ComboBox::from_id_salt(("choice_control", &c.id)).selected_text(cur.as_str()).show_ui(
                            ui,
                            |ui| {
                                for o in options {
                                    if ui.selectable_label(cur == o, o).clicked() {
                                        picked = Some(o);
                                    }
                                }
                            },
                        );
                    });
                } else {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&c.name).strong());
                        for o in options {
                            if ui.selectable_label(cur == o, o).clicked() {
                                picked = Some(o);
                            }
                        }
                    });
                }
                if let Some(o) = picked.filter(|o| *o != cur) {
                    let _ = self.h.cmd.send(Command::SetControl {
                        device: dev.to_string(),
                        id: c.id.clone(),
                        value: ControlValue::Choice(o.clone()),
                    });
                }
                if let Some(h) = c.help.as_deref().filter(|h| !h.is_empty()) {
                    ui.label(RichText::new(h).small().weak());
                }
            }
            (ControlKind::Number { min, max, step, unit }, ControlValue::Number(cur)) => {
                // Keep the dragged value locally until release.
                let tmp = ui.scope_id().with(("number_control", &c.id));
                let mut v = ui.data(|m| m.get_temp::<f64>(tmp)).unwrap_or(*cur);
                let r = ui
                    .horizontal(|ui| {
                        ui.label(RichText::new(&c.name).strong());
                        ui.add(egui::Slider::new(&mut v, *min..=*max).step_by(*step).suffix(format!(" {unit}")))
                    })
                    .inner;
                // Send once the slider is released (or on keyboard/click edits).
                if r.dragged() {
                    ui.data_mut(|m| m.insert_temp(tmp, v));
                } else {
                    ui.data_mut(|m| m.remove::<f64>(tmp));
                }
                if (r.drag_stopped() || (r.changed() && !r.dragged())) && v != *cur {
                    let _ = self.h.cmd.send(Command::SetControl {
                        device: dev.to_string(),
                        id: c.id.clone(),
                        value: ControlValue::Number(v),
                    });
                }
                if let Some(h) = c.help.as_deref().filter(|h| !h.is_empty()) {
                    ui.label(RichText::new(h).small().weak());
                }
            }
            _ => {}
        }
    }

    pub(crate) fn peripheral_page(&mut self, ui: &mut egui::Ui, d: &DeviceView) {
        let dev = d.info.id.clone();
        ui.horizontal(|ui| {
            ui.heading(d.info.display_name());
            if d.online {
                ui.label(RichText::new("online").color(ACCENT));
            } else {
                ui.label(RichText::new("offline").color(BAD));
            }
            if let Some(fw) = &d.info.firmware {
                ui.label(RichText::new(format!("firmware {fw}")).weak());
            }
            if let Some(e) = &d.last_error {
                ui.label(RichText::new(format!("⚠ {e}")).color(WARN));
            }
        });
        ui.add_space(6.0);

        // Tabs, so busy devices (e.g. a keyboard with lighting, keys and
        // settings) don't become one long page. Only tabs with content are
        // shown, and none at all when there is only one.
        let settings: Vec<&Control> = d.controls.iter().filter(|c| !is_lighting_control(&c.id)).collect();
        let mut tabs = vec![Tab::Overview];
        if !d.zones.is_empty() {
            tabs.push(Tab::Lighting);
        }
        if !d.keys.is_empty() || d.keymap_info.is_some() {
            tabs.push(Tab::Keys);
        }
        if !settings.is_empty() {
            tabs.push(Tab::Settings);
        }
        let tab_id = ui.scope_id().with(("device_tab", &dev));
        let mut tab = ui.data(|m| m.get_temp::<Tab>(tab_id)).filter(|t| tabs.contains(t)).unwrap_or(tabs[0]);
        if tabs.len() > 1 {
            ui.horizontal(|ui| {
                for t in &tabs {
                    ui.selectable_value(&mut tab, *t, RichText::new(t.name()).strong().size(15.0));
                }
            });
            ui.separator();
            ui.add_space(4.0);
            ui.data_mut(|m| m.insert_temp(tab_id, tab));
        }
        // With no tab bar, everything is on the one page.
        let all = tabs.len() <= 1;
        let show = |t: Tab| all || tab == t;

        if show(Tab::Overview) {
            // ---- history chart (click any sensor below to chart it) ----
            if d.sensors.iter().any(|s| !s.kind.is_binary()) {
                if !self.charts.contains_key(&dev) {
                    let def = FerroApp::default_chart(d);
                    if !def.1.is_empty() {
                        self.charts.insert(dev.clone(), def);
                    }
                }
                let (title, series) = self.chart_for(&dev);
                let now = self.h.shared.read().started.elapsed().as_secs_f64();
                section(ui, "", |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&title).strong());
                        ui.label(RichText::new("· click any sensor below to chart it").small().weak());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            for (label, secs) in crate::gui::RANGES.iter().rev() {
                                if ui.selectable_label(self.range_secs == *secs, *label).clicked() {
                                    self.range_secs = *secs;
                                }
                            }
                        });
                    });
                    self.history_chart(ui, d, &series, now);
                });
                ui.add_space(6.0);
            }

            // ---- sensors (presence, load, temperatures, battery, fans…) ----
            if !d.sensors.is_empty() {
                let title = if d.info.kind == opendevicecontrol_core::DeviceKind::Computer { "Hardware sensors" } else { "Sensors" };
                self.device_tree(ui, d, title);
                if !d.online {
                    ui.label(RichText::new("Last known values (device is off or out of range).").small().weak());
                }
                ui.add_space(6.0);
            }
            // ---- fans and pumps ----
            if !d.fans.is_empty() {
                section(ui, "Fans and pumps", |ui| {
                    for (i, ch) in d.fans.iter().enumerate() {
                        if i > 0 {
                            ui.separator();
                        }
                        self.fan_controls(ui, d, ch);
                    }
                });
                ui.add_space(6.0);
            }
        }

        if show(Tab::Lighting) {
            // ---- lighting ----
            if !d.zones.is_empty() {
                let mut l = self.h.config.read().lighting(&dev);
                let before = l.clone();
                section(ui, "Lighting", |ui| {
                    // Live preview of what the device shows right now: the
                    // device's own drawing when it knows where its LEDs sit.
                    if !d.led_layout.is_empty() {
                        draw_layout(ui, &d.led_layout, &d.lights);
                    }
                    // Otherwise swatches; per-key devices have 100+ LEDs:
                    // smaller swatches, wrapped.
                    let many = d.zones.iter().map(|z| z.leds).sum::<usize>() > 16;
                    let (swatch, round) = if many { (egui::vec2(12.0, 12.0), 2.0) } else { (egui::vec2(56.0, 28.0), 6.0) };
                    if d.led_layout.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            if many {
                                ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
                            }
                            let mut i = 0;
                            for z in &d.zones {
                                for _ in 0..z.leds {
                                    let c = d.lights.get(i).copied().unwrap_or_default();
                                    let (rect, resp) = ui.allocate_exact_size(swatch, egui::Sense::hover());
                                    ui.painter().rect_filled(rect, round, to_c32(c));
                                    ui.painter().rect_stroke(rect, round, egui::Stroke::new(1.0, Color32::from_gray(70)), egui::StrokeKind::Inside);
                                    resp.on_hover_text(format!("{} {}", z.name, c.to_hex()));
                                    i += 1;
                                }
                            }
                            ui.label(RichText::new("live preview").small().weak());
                        });
                    }
                    ui.add_space(6.0);
                    // Where the lighting comes from: OpenDeviceControl's effects, its
                    // sound-reactive effects, or the device's own effects.
                    let sw = d.controls.iter().find(|c| c.id == SOFTWARE_LIGHTING);
                    let built_in = sw.is_some_and(|c| c.value == ControlValue::Bool(false));
                    let mode = if built_in {
                        Mode::BuiltIn
                    } else if l.effect.is_sound() {
                        Mode::Sound
                    } else {
                        Mode::Effects
                    };
                    let mut picked = mode;
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut picked, Mode::Effects, RichText::new("OpenDeviceControl effects").strong());
                        ui.selectable_value(&mut picked, Mode::Sound, RichText::new("Sound reactive").strong());
                        if sw.is_some() {
                            ui.selectable_value(&mut picked, Mode::BuiltIn, RichText::new("Built-in effect").strong());
                        }
                    });
                    if picked != mode {
                        if (picked == Mode::BuiltIn) != built_in {
                            let _ = self.h.cmd.send(Command::SetControl {
                                device: dev.clone(),
                                id: SOFTWARE_LIGHTING.into(),
                                value: ControlValue::Bool(picked != Mode::BuiltIn),
                            });
                        }
                        let want_sound = picked == Mode::Sound;
                        if picked != Mode::BuiltIn && l.effect.is_sound() != want_sound {
                            // Back to the last effect used in that family.
                            let names = if want_sound { Effect::SOUND_NAMES } else { Effect::LIGHT_NAMES };
                            let next = match l.other.take() {
                                Some(e) if e.is_sound() == want_sound => e,
                                _ => l.effect.switch_to(names[0]).unwrap_or_else(|| l.effect.clone()),
                            };
                            l.other = Some(std::mem::replace(&mut l.effect, next));
                        }
                    }
                    ui.add_space(4.0);
                    if picked == Mode::BuiltIn {
                        ui.label(
                            RichText::new(
                                "The device runs its own effect, stored in the device. The preview is drawn here; \
                                 effects that react to key presses also react to keys typed in this window.",
                            )
                            .small()
                            .weak(),
                        );
                        let keys: Vec<&'static str> = ui.input(|i| {
                            i.events
                                .iter()
                                .filter_map(|e| match e {
                                    egui::Event::Key { key, pressed: true, .. } => preview_key_name(*key),
                                    _ => None,
                                })
                                .collect()
                        });
                        for key in keys {
                            let _ = self.h.cmd.send(Command::PreviewKey { device: dev.clone(), key: key.into() });
                        }
                        ui.ctx().request_repaint_after(Duration::from_millis(33));
                        for c in d.controls.iter().filter(|c| c.id.starts_with(HW_LIGHTING_PREFIX)) {
                            self.control_widget(ui, &dev, c);
                        }
                        return;
                    }
                    if picked == Mode::Sound {
                        let a = self.h.audio.frame();
                        let msg = match &a.error {
                            Some(e) => RichText::new(format!("⚠ {e}")).color(WARN),
                            None => RichText::new("Follows what your computer is playing.").small().weak(),
                        };
                        ui.label(msg);
                        // Keep the preview moving with the music.
                        ui.ctx().request_repaint_after(Duration::from_millis(50));
                    }
                    let names = if picked == Mode::Sound { Effect::SOUND_NAMES } else { Effect::LIGHT_NAMES };
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut l.on, "On");
                        ui.separator();
                        for name in names {
                            if ui.selectable_label(l.effect.name() == *name, *name).clicked() {
                                if let Some(e) = l.effect.switch_to(name) {
                                    l.effect = e;
                                }
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Brightness");
                        ui.add(egui::Slider::new(&mut l.brightness, 0..=100).suffix(" %"));
                    });
                    let zones = &d.zones;
                    match &mut l.effect {
                        Effect::Static { colors } => {
                            let mut per_zone = colors.len() > 1;
                            if zones.len() > 1 && ui.checkbox(&mut per_zone, "Different colour per zone").changed() {
                                let first = colors.first().copied().unwrap_or_default();
                                *colors = if per_zone { vec![first; zones.len()] } else { vec![first] };
                            }
                            ui.horizontal_wrapped(|ui| {
                                if per_zone {
                                    colors.resize(zones.len(), colors.last().copied().unwrap_or_default());
                                    for (z, c) in zones.iter().zip(colors.iter_mut()) {
                                        ui.label(&z.name);
                                        color_button(ui, c);
                                        ui.add_space(8.0);
                                    }
                                } else {
                                    if colors.is_empty() {
                                        colors.push(HexColor::default());
                                    }
                                    ui.label("Colour");
                                    color_button(ui, &mut colors[0]);
                                }
                            });
                        }
                        Effect::Breathing { color, speed } => {
                            ui.horizontal(|ui| {
                                ui.label("Colour");
                                color_button(ui, color);
                                ui.add_space(12.0);
                                ui.label("Speed");
                                ui.add(egui::Slider::new(speed, 0.1..=4.0).fixed_decimals(1));
                            });
                        }
                        Effect::ColorCycle { speed } | Effect::RainbowWave { speed } => {
                            ui.horizontal(|ui| {
                                ui.label("Speed");
                                ui.add(egui::Slider::new(speed, 0.1..=4.0).fixed_decimals(1));
                            });
                        }
                        Effect::ColorWave { color, color2, speed } => {
                            ui.horizontal(|ui| {
                                ui.label("Colours");
                                color_button(ui, color);
                                color_button(ui, color2);
                                ui.add_space(12.0);
                                ui.label("Speed");
                                ui.add(egui::Slider::new(speed, 0.1..=4.0).fixed_decimals(1));
                            });
                        }
                        e => {
                            if let Some(st) = e.sound_style_mut() {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label("Colours");
                                    ui.selectable_value(&mut st.rainbow, true, "Rainbow");
                                    ui.selectable_value(&mut st.rainbow, false, "Custom");
                                    if !st.rainbow {
                                        color_button(ui, &mut st.color);
                                        color_button(ui, &mut st.color2);
                                    }
                                    ui.add_space(12.0);
                                    ui.label("Sensitivity");
                                    ui.add(egui::Slider::new(&mut st.sensitivity, 0.25..=4.0).logarithmic(true).fixed_decimals(2));
                                });
                            }
                        }
                    }
                    if d.keys.iter().any(|k| k.id == opendevicecontrol_core::devices::nxp::KEY_BRIGHTNESS) {
                        ui.label(
                            RichText::new("The keyboard's brightness key cycles 100 → 66 → 33 → off, like it does with iCUE.")
                                .small()
                                .weak(),
                        );
                    }
                });
                if l != before {
                    self.set_lighting(ui, &dev, l);
                } else if !ui.input(|i| i.pointer.any_down()) && self.h.config.read().lighting.get(&dev) != Some(&l) {
                    // A drag just ended: persist what was applied live.
                    self.h.update_config(|c| {
                        c.lighting.insert(dev.clone(), l);
                    });
                }
                ui.add_space(6.0);
            }
        }

        if show(Tab::Keys) {
            // ---- device-side remapping and macros ----
            if d.keymap_info.is_some() {
                self.keymap_editor(ui, d);
                ui.add_space(6.0);
            }
            // ---- programmable keys ----
            if !d.keys.is_empty() {
                section(ui, "Keys", |ui| {
                    ui.label(
                        RichText::new(
                            "Each press is also sent to Home Assistant as an event, so a key can trigger automations \
                             even with no action here.",
                        )
                        .small()
                        .weak(),
                    );
                    egui::Grid::new("keys").num_columns(4).spacing([12.0, 6.0]).striped(true).show(ui, |ui| {
                        for k in &d.keys {
                            let recent = d.last_key.as_ref().is_some_and(|(id, t)| id == &k.id && t.elapsed() < Duration::from_millis(800));
                            let name = RichText::new(&k.name).strong();
                            ui.label(if recent { name.color(ACCENT) } else { name });
                            if !k.assignable {
                                ui.label(RichText::new(match k.id.as_str() {
                                    opendevicecontrol_core::devices::nxp::KEY_BRIGHTNESS => "Cycles lighting brightness",
                                    opendevicecontrol_core::devices::nxp::KEY_WINLOCK => "Toggles Win lock",
                                    _ => "Built-in function",
                                }).weak());
                                ui.label("");
                                ui.label("");
                                ui.end_row();
                                continue;
                            }
                            let key = format!("{dev}/{}", k.id);
                            let saved = self.h.config.read().key_action(&dev, &k.id);
                            let draft = self.key_drafts.entry(key.clone()).or_insert_with(|| saved.clone());
                            egui::ComboBox::from_id_salt(("action", &key)).selected_text(draft.name()).show_ui(ui, |ui| {
                                for n in KeyAction::NAMES {
                                    if ui.selectable_label(draft.name() == n, n).clicked() && draft.name() != n {
                                        *draft = KeyAction::from_name(n);
                                    }
                                }
                            });
                            match draft {
                                KeyAction::None => {
                                    ui.label(RichText::new("Home Assistant event only").weak());
                                }
                                KeyAction::Hotkey { keys } => {
                                    ui.add(egui::TextEdit::singleline(keys).hint_text("e.g. ctrl+shift+esc").desired_width(220.0));
                                }
                                KeyAction::Text { text } => {
                                    ui.add(egui::TextEdit::singleline(text).hint_text("text to type").desired_width(220.0));
                                }
                                KeyAction::Launch { command } => {
                                    ui.add(egui::TextEdit::singleline(command).hint_text("program, file or URL").desired_width(220.0));
                                }
                                KeyAction::Media { key: mk } => {
                                    let label = crate::inject::MEDIA_KEYS.iter().find(|(id, _)| id == mk).map(|(_, n)| *n).unwrap_or("?");
                                    egui::ComboBox::from_id_salt(("media", &key)).selected_text(label).show_ui(ui, |ui| {
                                        for (id, n) in crate::inject::MEDIA_KEYS {
                                            ui.selectable_value(mk, id.to_string(), n);
                                        }
                                    });
                                }
                            }
                            let invalid = match &*draft {
                                KeyAction::Hotkey { keys } => crate::inject::parse_hotkey(keys).err(),
                                _ => None,
                            };
                            ui.horizontal(|ui| {
                                let dirty = *draft != saved;
                                if ui.add_enabled(dirty && invalid.is_none(), egui::Button::new("Save")).clicked() {
                                    let a = draft.clone();
                                    self.h.update_config(|c| {
                                        c.keys.insert(key.clone(), a);
                                    });
                                }
                                if let Some(e) = invalid {
                                    ui.label(RichText::new(e).color(WARN).small());
                                } else if dirty {
                                    ui.label(RichText::new("unsaved").color(WARN).small());
                                }
                            });
                            ui.end_row();
                        }
                    });
                    ui.label(
                        RichText::new(
                            "Hotkey names: ctrl, shift, alt, win, a–z, 0–9, f1–f24, esc, tab, enter, space, backspace, delete, \
                             home, end, pageup, pagedown, up, down, left, right, printscreen.",
                        )
                        .small()
                        .weak(),
                    );
                });
                ui.add_space(6.0);
            }
        }

        // ---- on/off and choice settings (built-in lighting controls are
        // shown with the lighting) ----
        if show(Tab::Settings) && !settings.is_empty() {
            section(ui, "Settings", |ui| {
                for c in settings {
                    self.control_widget(ui, &dev, c);
                }
            });
            ui.add_space(6.0);
        }

        if !show(Tab::Overview) {
            return;
        }
        egui::CollapsingHeader::new("Device information").default_open(false).show(ui, |ui| {
            egui::Grid::new("pinfo").striped(true).show(ui, |ui| {
                let i = &d.info;
                let rows = [
                    ("Model", i.model.clone()),
                    ("Firmware", i.firmware.clone().unwrap_or_else(|| "?".into())),
                    ("USB ID", format!("{:04x}:{:04x}", i.usb_vid, i.usb_pid)),
                    ("Serial", i.serial.clone().unwrap_or_default()),
                    ("OpenDeviceControl id", i.id.clone()),
                ];
                for (k, v) in rows {
                    ui.label(k);
                    ui.label(v);
                    ui.end_row();
                }
            });
        });
    }
}
