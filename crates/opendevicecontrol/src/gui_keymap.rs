//! Keys tab: device-side key remapping (layers drawn as the keyboard, click
//! a key to change what it does) and macros stored in the device.
//!
//! Edits are drafts until "Save to keyboard": every save rewrites the
//! device's flash, so nothing is written while the user is still clicking.

use crate::gui::{section, FerroApp, ACCENT, WARN};
use crate::service::{keymap_backup_path, Command, DeviceView};
use eframe::egui::{self, Color32, RichText};
use opendevicecontrol_core::keymap::{
    modifier, usage_name, KeyBinding, KeymapInfo, Macro, MacroAction, MacroMode, MacroStep, MouseAction, MEDIA_USAGES, MODIFIER_NAMES,
};
use opendevicecontrol_core::LedShape;
use std::time::Instant;

/// Sub-tab of the Keys tab: a layer id, or the macros.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sub {
    Layer(u8),
    Macros,
}

/// Per-device editor state (kept in egui memory).
#[derive(Debug, Clone, Default)]
struct EditorState {
    sub: Option<Sub>,
    selected: Option<usize>,
    /// Waiting for a key press to assign to the selected key.
    capturing: bool,
    /// Draft layers (layer id → keys) not yet saved.
    drafts: Vec<(u8, Vec<KeyBinding>)>,
    /// Layers already asked for.
    requested: Vec<u8>,
    macros_requested: bool,
    macro_draft: Option<Vec<Macro>>,
    macro_selected: usize,
    recording: Option<Recording>,
}

#[derive(Debug, Clone)]
struct Recording {
    last: Instant,
    modifiers: u8,
}

/// The kind picker's entries.
const KINDS: [&str; 8] = ["Key or shortcut", "Media", "Mouse", "Macro", "OpenDeviceControl key", "Device function", "Layer", "Nothing"];

fn kind_of(b: &KeyBinding) -> &'static str {
    match b {
        KeyBinding::Key { .. } => KINDS[0],
        KeyBinding::Media(_) => KINDS[1],
        KeyBinding::Mouse(_) => KINDS[2],
        KeyBinding::Macro { .. } => KINDS[3],
        KeyBinding::Host(_) => KINDS[4],
        KeyBinding::Function(_) => KINDS[5],
        KeyBinding::Layer(_) => KINDS[6],
        KeyBinding::None => KINDS[7],
        KeyBinding::Raw(_) => "Device action",
    }
}

/// A default binding of `kind`.
fn new_of_kind(kind: &str, info: &KeymapInfo) -> KeyBinding {
    match kind {
        "Key or shortcut" => KeyBinding::Key { modifiers: 0, usage: 0x04 },
        "Media" => KeyBinding::Media(MEDIA_USAGES[0].0),
        "Mouse" => KeyBinding::Mouse(MouseAction::Button(1)),
        "Macro" => KeyBinding::Macro { slot: 0, mode: MacroMode::Times(1) },
        "OpenDeviceControl key" => KeyBinding::Host(1),
        "Device function" => KeyBinding::Function(info.functions.first().map(|f| f.id).unwrap_or(0)),
        "Layer" => KeyBinding::Layer(info.layers.iter().map(|l| l.id).find(|&i| i != 0).unwrap_or(1)),
        _ => KeyBinding::None,
    }
}

/// HID keyboard usage of an egui key.
pub fn hid_usage(k: egui::Key) -> Option<u8> {
    use egui::Key as K;
    let letters = [
        K::A, K::B, K::C, K::D, K::E, K::F, K::G, K::H, K::I, K::J, K::K, K::L, K::M, K::N, K::O, K::P, K::Q, K::R, K::S, K::T, K::U, K::V,
        K::W, K::X, K::Y, K::Z,
    ];
    if let Some(i) = letters.iter().position(|&l| l == k) {
        return Some(0x04 + i as u8);
    }
    let digits = [K::Num1, K::Num2, K::Num3, K::Num4, K::Num5, K::Num6, K::Num7, K::Num8, K::Num9, K::Num0];
    if let Some(i) = digits.iter().position(|&d| d == k) {
        return Some(0x1e + i as u8);
    }
    let fkeys = [K::F1, K::F2, K::F3, K::F4, K::F5, K::F6, K::F7, K::F8, K::F9, K::F10, K::F11, K::F12];
    if let Some(i) = fkeys.iter().position(|&f| f == k) {
        return Some(0x3a + i as u8);
    }
    Some(match k {
        K::Enter => 0x28,
        K::Escape => 0x29,
        K::Backspace => 0x2a,
        K::Tab => 0x2b,
        K::Space => 0x2c,
        K::Minus => 0x2d,
        K::Equals => 0x2e,
        K::OpenBracket => 0x2f,
        K::CloseBracket => 0x30,
        K::Backslash => 0x31,
        K::Semicolon => 0x33,
        K::Quote => 0x34,
        K::Backtick => 0x35,
        K::Comma => 0x36,
        K::Period => 0x37,
        K::Slash => 0x38,
        K::Insert => 0x49,
        K::Home => 0x4a,
        K::PageUp => 0x4b,
        K::Delete => 0x4c,
        K::End => 0x4d,
        K::PageDown => 0x4e,
        K::ArrowRight => 0x4f,
        K::ArrowLeft => 0x50,
        K::ArrowDown => 0x51,
        K::ArrowUp => 0x52,
        _ => return None,
    })
}

/// HID modifier bits held, from egui's (left variants).
fn mods_of(m: egui::Modifiers) -> u8 {
    (if m.ctrl { modifier::LCTRL } else { 0 })
        | (if m.shift { modifier::LSHIFT } else { 0 })
        | (if m.alt { modifier::LALT } else { 0 })
        | (if m.mac_cmd { modifier::LGUI } else { 0 })
}

/// Keyboard usages offered in the key picker, in a sensible order.
fn picker_usages() -> Vec<u8> {
    let mut v: Vec<u8> = (0x04..=0x65).collect();
    v.extend(0x68..=0x73);
    v.extend(0xe0..=0xe7);
    v.retain(|u| usage_name(*u).is_some());
    v
}

/// Draw the keyboard; returns the clicked key (zone index).
#[allow(clippy::too_many_arguments)]
fn draw_keys(
    ui: &mut egui::Ui,
    shapes: &[LedShape],
    keys: &[KeyBinding],
    saved: &[KeyBinding],
    original: Option<&[KeyBinding]>,
    selected: Option<usize>,
    info: &KeymapInfo,
    layer: u8,
) -> Option<usize> {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for s in shapes {
        let [x, y, w, h] = s.rect;
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + w), y1.max(y + h));
    }
    let (bw, bh) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
    let pad = 10.0;
    let width = ui.available_width().min(900.0);
    let scale = ((width - 2.0 * pad) / bw).max(0.1);
    let (area, resp) = ui.allocate_exact_size(egui::vec2(bw * scale + 2.0 * pad, bh * scale + 2.0 * pad), egui::Sense::click());
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 8.0, Color32::from_gray(28));
    let font = egui::FontId::proportional((8.5 * scale).clamp(8.0, 12.0));
    let mut hovered = None;
    let mut clicked = None;
    for s in shapes {
        let [x, y, w, h] = s.rect;
        let rect = egui::Rect::from_min_size(area.min + egui::vec2(pad + (x - x0) * scale, pad + (y - y0) * scale), egui::vec2(w * scale, h * scale));
        let fixed = info.fixed.iter().find(|f| f.layer == layer && f.key == s.led);
        let b = match fixed {
            Some(f) => f.does.clone(),
            None => keys.get(s.led).cloned().unwrap_or(KeyBinding::None),
        };
        let pending = fixed.is_none() && saved.get(s.led) != keys.get(s.led);
        let remapped = original.is_some_and(|o| o.get(s.led) != Some(&b));
        let remapped = remapped && fixed.is_none();
        let fill = if fixed.is_some() {
            Color32::from_gray(38)
        } else if remapped {
            ACCENT.gamma_multiply(0.45)
        } else {
            Color32::from_gray(50)
        };
        let round = (3.0 * scale).clamp(2.0, 5.0);
        painter.rect_filled(rect, round, fill);
        let stroke = if Some(s.led) == selected {
            egui::Stroke::new(2.5, Color32::WHITE)
        } else if pending {
            egui::Stroke::new(2.0, WARN)
        } else {
            egui::Stroke::new(1.0, Color32::from_gray(85))
        };
        painter.rect_stroke(rect, round, stroke, egui::StrokeKind::Inside);
        // Keys doing something else show that, the rest their own label.
        // Keys doing something else show that (the longest label that
        // fits), locked keys just the lock; hover shows the full action.
        let candidates = if fixed.is_some() {
            vec!["🔒".to_string()]
        } else if remapped || pending {
            b.key_labels(info)
        } else {
            vec![s.label.clone()]
        };
        let colour = Color32::from_gray(230);
        let room = rect.width() - 4.0;
        let mut fitted = None;
        'fit: for size in [font.size, font.size * 0.85, font.size * 0.72] {
            for c in &candidates {
                let g = painter.layout_no_wrap(c.clone(), egui::FontId::proportional(size), colour);
                if g.size().x <= room {
                    fitted = Some(g);
                    break 'fit;
                }
            }
        }
        let g = fitted.unwrap_or_else(|| painter.layout_no_wrap("•".into(), font.clone(), colour));
        if !candidates.iter().all(|c| c.is_empty()) {
            painter.galley(rect.center() - g.size() / 2.0, g, colour);
        }
        if resp.hover_pos().is_some_and(|p| rect.contains(p)) {
            hovered = Some((s, b.clone()));
            if resp.clicked() {
                clicked = Some(s.led);
            }
        }
    }
    if let Some((s, b)) = hovered {
        resp.on_hover_text(format!("{}: {}", s.name, b.describe(info)));
    }
    clicked
}

impl FerroApp {
    /// The Keys tab's device-side remapping. Only for devices with
    /// [`KeymapInfo`].
    pub(crate) fn keymap_editor(&mut self, ui: &mut egui::Ui, d: &DeviceView) {
        let Some(info) = d.keymap_info.clone() else { return };
        let dev = d.info.id.clone();
        let sid = ui.scope_id().with(("keymap_editor", &dev));
        let mut st: EditorState = ui.data(|m| m.get_temp(sid)).unwrap_or_default();
        let sub = st.sub.unwrap_or(Sub::Layer(info.layers.first().map(|l| l.id).unwrap_or(0)));

        section(ui, "Key assignments", |ui| {
            ui.horizontal_wrapped(|ui| {
                let mut s = sub;
                for l in &info.layers {
                    ui.selectable_value(&mut s, Sub::Layer(l.id), RichText::new(&l.name).strong());
                }
                if info.macro_slots > 0 {
                    ui.separator();
                    ui.selectable_value(&mut s, Sub::Macros, RichText::new("Macros").strong());
                }
                if s != sub {
                    st.sub = Some(s);
                    st.selected = None;
                    st.capturing = false;
                }
            });
            if let Some(msg) = &d.keymap_status {
                ui.label(RichText::new(msg).color(WARN));
            }
            ui.add_space(4.0);
            match st.sub.unwrap_or(sub) {
                Sub::Layer(layer) => self.layer_editor(ui, d, &info, layer, &mut st),
                Sub::Macros => self.macro_editor(ui, d, &info, &mut st),
            }
        });
        ui.data_mut(|m| m.insert_temp(sid, st));
    }

    fn layer_editor(&mut self, ui: &mut egui::Ui, d: &DeviceView, info: &KeymapInfo, layer: u8, st: &mut EditorState) {
        let dev = d.info.id.clone();
        if let Some(l) = info.layers.iter().find(|l| l.id == layer) {
            ui.label(RichText::new(&l.help).small().weak());
        }
        let Some(saved) = d.keymap.get(&layer).cloned() else {
            if !st.requested.contains(&layer) {
                st.requested.push(layer);
                let _ = self.h.cmd.send(Command::ReadKeymap { device: dev, layer });
            }
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading the keys from the keyboard…");
            });
            return;
        };
        let original: Option<Vec<KeyBinding>> =
            std::fs::read(keymap_backup_path(&dev, &format!("layer{layer}"))).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let draft_pos = st.drafts.iter().position(|(l, _)| *l == layer);
        let mut keys = draft_pos.map(|i| st.drafts[i].1.clone()).unwrap_or_else(|| saved.clone());

        if let Some(i) = draw_keys(ui, &d.led_layout, &keys, &saved, original.as_deref(), st.selected, info, layer) {
            st.selected = Some(i);
            st.capturing = false;
        }
        ui.horizontal(|ui| {
            legend(ui, ACCENT.gamma_multiply(0.45), "changed from the keyboard's original");
            legend(ui, WARN, "not saved yet");
            if info.fixed.iter().any(|f| f.layer == layer) {
                legend(ui, Color32::from_gray(38), "🔒 fixed by the keyboard in this OS mode");
            }
        });
        ui.add_space(6.0);

        if let Some(i) = st.selected.filter(|&i| i < keys.len()) {
            let name = d.led_layout.iter().find(|s| s.led == i).map(|s| s.name.clone()).unwrap_or_else(|| format!("Key {i}"));
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&name).strong().size(15.0));
                    ui.label(RichText::new(format!("currently: {}", saved[i].describe(info))).weak());
                });
                if let Some(f) = info.fixed.iter().find(|f| f.layer == layer && f.key == i) {
                    ui.label(RichText::new(format!("🔒 Actually does: {}", f.does.describe(info))).strong());
                    ui.label(RichText::new(&f.reason).small().weak());
                    ui.add_space(4.0);
                }
                binding_editor(ui, &mut keys[i], info, &mut st.capturing, d.macros.as_deref());
            });
        } else {
            ui.label(RichText::new("Click a key to change what it does.").weak());
        }

        let changed = keys != saved;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let n = keys.iter().zip(&saved).filter(|(a, b)| a != b).count();
            if ui.add_enabled(changed, egui::Button::new(RichText::new(format!("Save to keyboard ({n})")).strong())).clicked() {
                let _ = self.h.cmd.send(Command::WriteKeymap { device: dev.clone(), layer, keys: keys.clone() });
            }
            if ui.add_enabled(changed, egui::Button::new("Discard changes")).clicked() {
                keys = saved.clone();
            }
            if let Some(o) = &original {
                if ui.add_enabled(&keys != o, egui::Button::new("Restore original")).on_hover_text("The keys as they were before OpenDeviceControl first changed them (not saved until you click Save)").clicked() {
                    keys = o.clone();
                }
            }
        });
        ui.label(RichText::new("Saving rewrites the keyboard's memory, so changes are collected and saved together.").small().weak());
        if keys != saved {
            match draft_pos {
                Some(p) => st.drafts[p].1 = keys,
                None => st.drafts.push((layer, keys)),
            }
        } else if let Some(p) = draft_pos {
            st.drafts.remove(p);
        }
    }

    fn macro_editor(&mut self, ui: &mut egui::Ui, d: &DeviceView, info: &KeymapInfo, st: &mut EditorState) {
        let dev = d.info.id.clone();
        let Some(saved) = d.macros.clone() else {
            if !st.macros_requested {
                st.macros_requested = true;
                let _ = self.h.cmd.send(Command::ReadMacros { device: dev });
            }
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading macros from the keyboard…");
            });
            return;
        };
        let mut macros = st.macro_draft.clone().unwrap_or_else(|| saved.clone());
        macros.resize_with(info.macro_slots as usize, Macro::default);
        let mut saved_full = saved.clone();
        saved_full.resize_with(info.macro_slots as usize, Macro::default);

        ui.label(RichText::new("Macros play on the keyboard itself. Assign one to a key with \"Macro\" in a layer.").small().weak());
        ui.add_space(4.0);
        ui.columns(2, |cols| {
            let ui = &mut cols[0];
            egui::ScrollArea::vertical().id_salt("macro_list").max_height(320.0).show(ui, |ui| {
                for (i, m) in macros.iter().enumerate() {
                    let label = if m.steps.is_empty() {
                        RichText::new(format!("{}  (empty)", i + 1)).weak()
                    } else {
                        RichText::new(format!("{}  {}  · {} steps", i + 1, if m.name.is_empty() { "Macro" } else { &m.name }, m.steps.len()))
                    };
                    if ui.selectable_label(st.macro_selected == i, label).clicked() {
                        st.macro_selected = i;
                        st.recording = None;
                    }
                }
            });
            let ui = &mut cols[1];
            if let Some(m) = macros.get_mut(st.macro_selected) {
                macro_steps(ui, m, info, &mut st.recording);
            }
        });
        let changed = macros != saved_full;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(changed, egui::Button::new(RichText::new("Save macros to keyboard").strong())).clicked() {
                let _ = self.h.cmd.send(Command::WriteMacros { device: dev.clone(), macros: macros.clone() });
            }
            if ui.add_enabled(changed, egui::Button::new("Discard changes")).clicked() {
                macros = saved_full.clone();
                st.recording = None;
            }
        });
        st.macro_draft = if macros != saved_full { Some(macros) } else { None };
    }
}

fn legend(ui: &mut egui::Ui, c: Color32, text: &str) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    ui.painter().rect_filled(r, 2.0, c);
    ui.label(RichText::new(text).small().weak());
    ui.add_space(8.0);
}

/// Edit one key's binding.
fn binding_editor(ui: &mut egui::Ui, b: &mut KeyBinding, info: &KeymapInfo, capturing: &mut bool, macros: Option<&[Macro]>) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Does");
        let mut kind = kind_of(b);
        egui::ComboBox::from_id_salt("binding_kind").selected_text(kind).show_ui(ui, |ui| {
            for k in KINDS {
                if k == "Macro" && info.macro_slots == 0 || k == "Layer" && info.layers.len() < 2 || k == "Device function" && info.functions.is_empty() {
                    continue;
                }
                ui.selectable_value(&mut kind, k, k);
            }
        });
        if kind != kind_of(b) {
            *b = new_of_kind(kind, info);
        }
    });
    match b {
        KeyBinding::Key { modifiers, usage } => {
            ui.horizontal_wrapped(|ui| {
                for (bit, name) in MODIFIER_NAMES.iter().take(4) {
                    let mut on = *modifiers & bit != 0;
                    if ui.checkbox(&mut on, *name).changed() {
                        *modifiers ^= bit;
                    }
                }
                ui.label("+");
                let current = usage_name(*usage).unwrap_or("(none)");
                egui::ComboBox::from_id_salt("binding_usage").selected_text(current).height(320.0).show_ui(ui, |ui| {
                    ui.selectable_value(usage, 0, "(modifiers only)");
                    for u in picker_usages() {
                        ui.selectable_value(usage, u, usage_name(u).unwrap_or(""));
                    }
                });
                let label = if *capturing { "Press a key… (Esc cancels)" } else { "Press a key to set" };
                if ui.selectable_label(*capturing, label).clicked() {
                    *capturing = !*capturing;
                }
            });
            if *capturing {
                let got = ui.input(|i| {
                    i.events.iter().find_map(|e| match e {
                        egui::Event::Key { key, pressed: true, modifiers: m, .. } => Some((*key, mods_of(*m))),
                        _ => None,
                    })
                });
                if let Some((k, m)) = got {
                    if k != egui::Key::Escape {
                        if let Some(u) = hid_usage(k) {
                            *usage = u;
                            *modifiers = m;
                        }
                    }
                    *capturing = false;
                }
            }
        }
        KeyBinding::Media(u) => {
            let current = MEDIA_USAGES.iter().find(|(c, _)| c == u).map(|(_, n)| *n).unwrap_or("Other");
            egui::ComboBox::from_id_salt("binding_media").selected_text(current).show_ui(ui, |ui| {
                for (c, n) in MEDIA_USAGES {
                    ui.selectable_value(u, *c, *n);
                }
            });
        }
        KeyBinding::Mouse(m) => {
            let options: [(MouseAction, &str); 11] = [
                (MouseAction::Button(1), "Left click"),
                (MouseAction::Button(2), "Right click"),
                (MouseAction::Button(3), "Middle click"),
                (MouseAction::Button(4), "Back button"),
                (MouseAction::Button(5), "Forward button"),
                (MouseAction::Wheel(1), "Scroll up"),
                (MouseAction::Wheel(-1), "Scroll down"),
                (MouseAction::MoveX(-1), "Move pointer left"),
                (MouseAction::MoveX(1), "Move pointer right"),
                (MouseAction::MoveY(-1), "Move pointer up"),
                (MouseAction::MoveY(1), "Move pointer down"),
            ];
            let current = options.iter().find(|(a, _)| a == m).map(|(_, n)| *n).unwrap_or("Mouse action");
            egui::ComboBox::from_id_salt("binding_mouse").selected_text(current).show_ui(ui, |ui| {
                for (a, n) in options {
                    ui.selectable_value(m, a, n);
                }
            });
        }
        KeyBinding::Macro { slot, mode } => {
            ui.horizontal_wrapped(|ui| {
                let name = |i: u8| {
                    macros
                        .and_then(|m| m.get(i as usize))
                        .filter(|m| !m.name.is_empty())
                        .map(|m| format!("{} {}", i + 1, m.name))
                        .unwrap_or_else(|| format!("Macro {}", i + 1))
                };
                egui::ComboBox::from_id_salt("binding_macro").selected_text(name(*slot)).show_ui(ui, |ui| {
                    for i in 0..info.macro_slots {
                        ui.selectable_value(slot, i, name(i));
                    }
                });
                let mut m = match mode {
                    MacroMode::Times(_) => 0,
                    MacroMode::WhileHeld => 1,
                    MacroMode::Toggle => 2,
                };
                ui.selectable_value(&mut m, 0, "Play");
                ui.selectable_value(&mut m, 1, "Repeat while held");
                ui.selectable_value(&mut m, 2, "Toggle on/off");
                *mode = match (m, *mode) {
                    (0, MacroMode::Times(n)) => MacroMode::Times(n),
                    (0, _) => MacroMode::Times(1),
                    (1, _) => MacroMode::WhileHeld,
                    _ => MacroMode::Toggle,
                };
                if let MacroMode::Times(n) = mode {
                    ui.add(egui::DragValue::new(n).range(1..=255).suffix(" ×"));
                }
            });
        }
        KeyBinding::Host(n) => {
            ui.horizontal_wrapped(|ui| {
                ui.label("Number");
                ui.add(egui::DragValue::new(n).range(1..=16));
            });
            let note = if info.host_keys {
                "Pressing it reports \"OpenDeviceControl key\" to OpenDeviceControl instead of typing: give it an action in the Programmable keys section, or use it in Home Assistant."
            } else {
                "Reaches OpenDeviceControl only while the keyboard is connected by cable."
            };
            ui.label(RichText::new(note).small().weak());
        }
        KeyBinding::Function(id) => {
            let current = info.functions.iter().find(|f| f.id == *id).map(|f| format!("{}: {}", f.group, f.name)).unwrap_or_default();
            egui::ComboBox::from_id_salt("binding_fn").selected_text(current).height(320.0).show_ui(ui, |ui| {
                let mut group = "";
                for f in &info.functions {
                    if f.group != group {
                        group = &f.group;
                        ui.label(RichText::new(group).strong().small());
                    }
                    ui.selectable_value(id, f.id, format!("  {}", f.name));
                }
            });
        }
        KeyBinding::Layer(l) => {
            let current = info.layers.iter().find(|x| x.id == *l).map(|x| x.name.clone()).unwrap_or_default();
            egui::ComboBox::from_id_salt("binding_layer").selected_text(current).show_ui(ui, |ui| {
                for x in info.layers.iter().filter(|x| x.id != 0) {
                    ui.selectable_value(l, x.id, &x.name);
                }
            });
            ui.label(RichText::new("While held, the keys act as in that layer.").small().weak());
        }
        KeyBinding::Raw(_) => {
            ui.label(RichText::new("A device action OpenDeviceControl doesn't know; it is kept unless you pick something else.").small().weak());
        }
        KeyBinding::None => {
            ui.label(RichText::new("The key does nothing in this layer.").small().weak());
        }
    }
}

const STEP_KINDS: [&str; 10] = ["Key down", "Key up", "Modifier down", "Modifier up", "Mouse down", "Mouse up", "Move X", "Move Y", "Wheel", "Pan"];

fn step_kind(a: &MacroAction) -> usize {
    match a {
        MacroAction::KeyDown(_) => 0,
        MacroAction::KeyUp(_) => 1,
        MacroAction::ModifierDown(_) => 2,
        MacroAction::ModifierUp(_) => 3,
        MacroAction::MouseDown(_) => 4,
        MacroAction::MouseUp(_) => 5,
        MacroAction::MoveX(_) => 6,
        MacroAction::MoveY(_) => 7,
        MacroAction::Wheel(_) => 8,
        MacroAction::Pan(_) => 9,
    }
}

fn step_of_kind(k: usize) -> MacroAction {
    match k {
        0 => MacroAction::KeyDown(0x04),
        1 => MacroAction::KeyUp(0x04),
        2 => MacroAction::ModifierDown(modifier::LCTRL),
        3 => MacroAction::ModifierUp(modifier::LCTRL),
        4 => MacroAction::MouseDown(1),
        5 => MacroAction::MouseUp(1),
        6 => MacroAction::MoveX(10),
        7 => MacroAction::MoveY(10),
        8 => MacroAction::Wheel(1),
        _ => MacroAction::Pan(1),
    }
}

/// Name, steps and recording for one macro.
fn macro_steps(ui: &mut egui::Ui, m: &mut Macro, info: &KeymapInfo, recording: &mut Option<Recording>) {
    ui.horizontal(|ui| {
        ui.label("Name");
        ui.add(egui::TextEdit::singleline(&mut m.name).desired_width(160.0).char_limit(16));
    });
    ui.horizontal(|ui| {
        if recording.is_some() {
            if ui.button(RichText::new("■ Stop recording").color(WARN)).clicked() {
                *recording = None;
            }
            ui.label(RichText::new("Type the macro in this window…").small());
        } else if ui.button("● Record").on_hover_text("Replaces the steps with what you type next, with its timing").clicked() {
            m.steps.clear();
            *recording = Some(Recording { last: Instant::now(), modifiers: 0 });
        }
        if ui.button("Clear").clicked() {
            m.steps.clear();
        }
    });
    if let Some(r) = recording {
        record_input(ui, m, r, info.macro_max_steps);
        ui.ctx().request_repaint();
    }
    egui::ScrollArea::vertical().id_salt("macro_steps").max_height(260.0).show(ui, |ui| {
        let mut remove = None;
        for (i, s) in m.steps.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{:>2}", i + 1)).weak().monospace());
                let mut k = step_kind(&s.action);
                egui::ComboBox::from_id_salt(("step_kind", i)).width(110.0).selected_text(STEP_KINDS[k]).show_ui(ui, |ui| {
                    for (j, n) in STEP_KINDS.iter().enumerate() {
                        ui.selectable_value(&mut k, j, *n);
                    }
                });
                if k != step_kind(&s.action) {
                    s.action = step_of_kind(k);
                }
                match &mut s.action {
                    MacroAction::KeyDown(u) | MacroAction::KeyUp(u) => {
                        egui::ComboBox::from_id_salt(("step_key", i)).width(110.0).selected_text(usage_name(*u).unwrap_or("?")).show_ui(ui, |ui| {
                            for x in picker_usages() {
                                ui.selectable_value(u, x, usage_name(x).unwrap_or(""));
                            }
                        });
                    }
                    MacroAction::ModifierDown(b) | MacroAction::ModifierUp(b) => {
                        let name = MODIFIER_NAMES.iter().find(|(x, _)| x == b).map(|(_, n)| *n).unwrap_or("?");
                        egui::ComboBox::from_id_salt(("step_mod", i)).width(110.0).selected_text(name).show_ui(ui, |ui| {
                            for (x, n) in MODIFIER_NAMES {
                                ui.selectable_value(b, x, n);
                            }
                        });
                    }
                    MacroAction::MouseDown(b) | MacroAction::MouseUp(b) => {
                        ui.add(egui::DragValue::new(b).range(1..=5).prefix("button "));
                    }
                    MacroAction::MoveX(v) | MacroAction::MoveY(v) | MacroAction::Wheel(v) | MacroAction::Pan(v) => {
                        ui.add(egui::DragValue::new(v).range(-127..=127));
                    }
                }
                ui.label("then wait");
                ui.add(egui::DragValue::new(&mut s.delay_ms).range(0..=60000).suffix(" ms"));
                if ui.small_button("✕").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            m.steps.remove(i);
        }
    });
    if m.steps.len() < info.macro_max_steps && ui.button("+ Add step").clicked() {
        m.steps.push(MacroStep { action: MacroAction::KeyDown(0x04), delay_ms: 20 });
    }
}

/// Turn key presses in the window into macro steps, with their timing.
fn record_input(ui: &egui::Ui, m: &mut Macro, r: &mut Recording, max: usize) {
    let (events, mods) = ui.input(|i| (i.events.clone(), mods_of(i.modifiers)));
    let mut push = |m: &mut Macro, a: MacroAction| {
        if m.steps.len() >= max {
            return;
        }
        let gap = r.last.elapsed().as_millis().min(60000) as u16;
        r.last = Instant::now();
        if let Some(prev) = m.steps.last_mut() {
            prev.delay_ms = gap;
        }
        m.steps.push(MacroStep { action: a, delay_ms: 0 });
    };
    for bit in [modifier::LCTRL, modifier::LSHIFT, modifier::LALT, modifier::LGUI] {
        if mods & bit != 0 && r.modifiers & bit == 0 {
            push(m, MacroAction::ModifierDown(bit));
        } else if mods & bit == 0 && r.modifiers & bit != 0 {
            push(m, MacroAction::ModifierUp(bit));
        }
    }
    r.modifiers = mods;
    for e in events {
        if let egui::Event::Key { key, pressed, repeat: false, .. } = e {
            if let Some(u) = hid_usage(key) {
                push(m, if pressed { MacroAction::KeyDown(u) } else { MacroAction::KeyUp(u) });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn egui_keys_map_to_hid() {
        assert_eq!(hid_usage(egui::Key::A), Some(0x04));
        assert_eq!(hid_usage(egui::Key::Num0), Some(0x27));
        assert_eq!(hid_usage(egui::Key::F12), Some(0x45));
        assert_eq!(hid_usage(egui::Key::ArrowUp), Some(0x52));
        for u in picker_usages() {
            assert!(usage_name(u).is_some());
        }
    }

    #[test]
    fn kinds_round_trip() {
        let info = KeymapInfo { macro_slots: 4, ..Default::default() };
        for k in KINDS {
            assert_eq!(kind_of(&new_of_kind(k, &info)), k);
        }
    }
}
