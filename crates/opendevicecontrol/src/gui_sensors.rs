//! The sensor tree, in the style of HWMonitor: every device and every piece
//! of hardware with its sensors grouped by type (Voltages, Temperatures,
//! Fans…), with current, lowest and highest values and a trend line.
//!
//! * click a sensor to graph it, Ctrl/Shift+click to add it to the graph,
//!   double-click a type group to graph the group;
//! * right-click for more (graph, copy, Home Assistant);
//! * the house column (when Home Assistant is on) shows, and toggles, what
//!   is sent to Home Assistant.
//!
//! The same widget, in "pick" mode, is the Home Assistant sensor chooser
//! (Settings → Home Assistant → Choose sensors).

use crate::config::Config;
use crate::gui::{fmt_value, FerroApp, ACCENT, RANGES, WARN};
use crate::icons;
use crate::service::DeviceView;
use eframe::egui::{self, pos2, vec2, Color32, FontId, Rect, RichText, Sense, Stroke};
use opendevicecontrol_core::{DeviceKind, Hardware, HardwareKind, Sensor, SensorKind};
use std::collections::{BTreeMap, HashMap};

const ROW_H: f32 = 22.0;
const INDENT: f32 = 16.0;
/// Most series a graph window shows.
const MAX_SERIES: usize = 12;
/// Seconds of history in the trend column.
const TREND_SECS: f64 = 120.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TreeMode {
    /// Values, min/max, trend; the house column toggles Home Assistant.
    View,
    /// Home Assistant chooser: a checkbox per sensor and group.
    Pick,
}

/// Per-tree view state (the Sensors page and the chooser each have one).
#[derive(Default)]
pub(crate) struct TreeUi {
    pub filter: String,
    /// Rows the user opened or closed (key → open), overriding the default.
    pub open: HashMap<String, bool>,
    pub only_sent: bool,
    /// After "Expand all" (true) or "Collapse all" (false): the default
    /// for every row, instead of the built-in one.
    pub expand_all: Option<bool>,
}

impl TreeUi {
    pub fn set_all(&mut self, open: bool) {
        self.open.clear();
        self.expand_all = Some(open);
    }
}

/// Which chart a click in a tree goes to.
pub(crate) enum ChartTarget {
    /// The Sensors page graph (any device).
    Sensors,
    /// The device page's own chart.
    Device,
}

enum RowKind {
    Device(DeviceKind, bool),
    Node(HardwareKind),
    Category(SensorKind),
    Sensor(usize),
}

struct Row {
    key: String,
    depth: usize,
    kind: RowKind,
    label: String,
    device: String,
    /// Sensor indices (into the device's list) in this row's subtree.
    members: Vec<usize>,
    open: bool,
}

/// What the user did in the tree this frame.
pub(crate) enum TreeAction {
    /// Graph these sensors of a device: replace the graph, or (true)
    /// toggle them in it.
    Chart(String, Vec<String>, bool),
    SetSends(String, Vec<String>, bool),
    Copy(String),
}

/// Top-level order: This PC first, then devices by kind and name.
pub(crate) fn device_order<'a>(devices: impl Iterator<Item = &'a DeviceView>) -> Vec<&'a DeviceView> {
    let mut v: Vec<&DeviceView> = devices.collect();
    v.sort_by_key(|d| (d.info.kind != DeviceKind::Computer, d.info.model.clone(), d.info.id.clone()));
    v
}

fn matches(filter: &str, d: &DeviceView, s: &Sensor) -> bool {
    if filter.is_empty() {
        return true;
    }
    let f = filter.to_lowercase();
    let node = s.hardware.as_deref().and_then(|h| d.node(h)).map(|h| h.name.to_lowercase()).unwrap_or_default();
    s.name.to_lowercase().contains(&f)
        || s.id.contains(&f)
        || node.contains(&f)
        || s.kind.category().to_lowercase().contains(&f)
        || d.info.model.to_lowercase().contains(&f)
}

/// Rows open by default: everything, except long sensor lists (per-thread
/// loads) and child network interfaces.
fn default_open(kind: &RowKind, members: usize, child: bool) -> bool {
    match kind {
        RowKind::Category(_) => members <= 16,
        RowKind::Node(HardwareKind::Network) => !child,
        _ => true,
    }
}

struct Builder<'a> {
    st: &'a mut TreeUi,
    cfg: &'a Config,
    rows: Vec<Row>,
    filtering: bool,
}

impl Builder<'_> {
    fn is_open(&mut self, key: &str, kind: &RowKind, members: usize, child: bool) -> bool {
        // Collapsing everything still lists the devices' hardware.
        let fallback = match self.st.expand_all {
            Some(all) => all || matches!(kind, RowKind::Device(..)),
            None => default_open(kind, members, child),
        };
        self.filtering || self.st.open.get(key).copied().unwrap_or(fallback)
    }

    fn device(&mut self, d: &DeviceView, multi: bool) {
        let visible: Vec<usize> = (0..d.sensors.len())
            .filter(|&i| {
                let s = &d.sensors[i];
                matches(&self.st.filter, d, s) && (!self.st.only_sent || self.cfg.sends(&d.info.id, s))
            })
            .collect();
        if visible.is_empty() {
            return;
        }
        let key = d.info.id.clone();
        let mut depth = 0;
        if multi {
            let kind = RowKind::Device(d.info.kind, d.online);
            let open = self.is_open(&key, &kind, visible.len(), false);
            self.rows.push(Row { key: key.clone(), depth: 0, kind, label: d.info.display_name(), device: d.info.id.clone(), members: visible.clone(), open });
            if !open {
                return;
            }
            depth = 1;
        }
        let known = |h: &str| d.hardware.iter().any(|n| n.id == h);
        let mut tops: Vec<&Hardware> = d.hardware.iter().filter(|h| h.parent.as_deref().is_none_or(|p| !known(p))).collect();
        tops.sort_by_key(|h| h.kind.order());
        for h in tops {
            self.node(d, h, &visible, depth, &key);
        }
        let loose: Vec<usize> = visible.iter().copied().filter(|&i| d.sensors[i].hardware.as_deref().is_none_or(|h| !known(h))).collect();
        self.categories(d, &loose, depth, &key);
    }

    fn subtree(d: &DeviceView, h: &Hardware, visible: &[usize]) -> Vec<usize> {
        let mut ids = vec![h.id.as_str()];
        let mut i = 0;
        while i < ids.len() {
            let cur = ids[i];
            for c in d.hardware.iter().filter(|c| c.parent.as_deref() == Some(cur)) {
                if !ids.contains(&c.id.as_str()) {
                    ids.push(c.id.as_str());
                }
            }
            i += 1;
        }
        visible.iter().copied().filter(|&i| d.sensors[i].hardware.as_deref().is_some_and(|x| ids.contains(&x))).collect()
    }

    fn node(&mut self, d: &DeviceView, h: &Hardware, visible: &[usize], depth: usize, parent_key: &str) {
        let members = Self::subtree(d, h, visible);
        if members.is_empty() {
            return;
        }
        let key = format!("{parent_key}/{}", h.id);
        let kind = RowKind::Node(h.kind);
        let open = self.is_open(&key, &kind, members.len(), h.parent.is_some());
        self.rows.push(Row { key: key.clone(), depth, kind, label: h.name.clone(), device: d.info.id.clone(), members, open });
        if !open {
            return;
        }
        let own: Vec<usize> = visible.iter().copied().filter(|&i| d.sensors[i].hardware.as_deref() == Some(h.id.as_str())).collect();
        self.categories(d, &own, depth + 1, &key);
        let mut children: Vec<&Hardware> = d.hardware.iter().filter(|c| c.parent.as_deref() == Some(h.id.as_str())).collect();
        children.sort_by_key(|c| c.kind.order());
        for c in children {
            self.node(d, c, visible, depth + 1, &key);
        }
    }

    fn categories(&mut self, d: &DeviceView, list: &[usize], depth: usize, parent_key: &str) {
        let mut groups: BTreeMap<(u8, &'static str), (SensorKind, Vec<usize>)> = BTreeMap::new();
        for &i in list {
            let k = d.sensors[i].kind;
            groups.entry((k.category_order(), k.category())).or_insert((k, Vec::new())).1.push(i);
        }
        for ((_, name), (kind, members)) in groups {
            let key = format!("{parent_key}/:{name}");
            let rk = RowKind::Category(kind);
            let open = self.is_open(&key, &rk, members.len(), false);
            self.rows.push(Row { key, depth, kind: rk, label: name.to_string(), device: d.info.id.clone(), members: members.clone(), open });
            if open {
                for i in members {
                    self.rows.push(Row {
                        key: format!("{}/{}", d.info.id, d.sensors[i].id),
                        depth: depth + 1,
                        kind: RowKind::Sensor(i),
                        label: d.sensors[i].name.clone(),
                        device: d.info.id.clone(),
                        members: vec![i],
                        open: false,
                    });
                }
            }
        }
    }
}

/// Every sensor as indented text (value, min, max), like a hardware
/// monitor's "save monitoring data".
pub(crate) fn report(devices: &[&DeviceView]) -> String {
    let mut st = TreeUi { expand_all: Some(true), ..Default::default() };
    let cfg = Config::default();
    let mut b = Builder { filtering: false, st: &mut st, cfg: &cfg, rows: Vec::new() };
    for d in devices {
        b.device(d, true);
    }
    let mut out = format!("OpenDeviceControl {} sensor report\n\n", env!("CARGO_PKG_VERSION"));
    out.push_str(&format!("{:<48} {:>14} {:>14} {:>14}\n", "", "Value", "Min", "Max"));
    for r in &b.rows {
        let indent = "  ".repeat(r.depth);
        let Some(d) = devices.iter().find(|d| d.info.id == r.device) else { continue };
        match r.kind {
            RowKind::Sensor(i) => {
                let s = &d.sensors[i];
                let (lo, hi) = d.stats.get(&s.id).map(|m| (fmt_value(m[0], s.kind), fmt_value(m[1], s.kind))).unwrap_or_default();
                let name = format!("{indent}{}", s.name);
                out.push_str(&format!("{name:<48} {:>14} {lo:>14} {hi:>14}\n", fmt_value(s.value, s.kind)));
            }
            _ => out.push_str(&format!("{indent}{}\n", r.label)),
        }
    }
    out
}

/// Text cut to `width` with an ellipsis.
fn galley(p: &egui::Painter, text: &str, font: FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: width.max(8.0), max_rows: 1, break_anywhere: true, ..Default::default() };
    p.layout_job(job)
}

/// Value as shown in the tree (no "estimated" decorations).
fn value_text(s: &Sensor, v: f64) -> String {
    fmt_value(v, s.kind)
}

/// Draws the trend line of the last [`TREND_SECS`] seconds.
fn sparkline(p: &egui::Painter, r: Rect, h: Option<&crate::service::Series>, now: f64, color: Color32) {
    let Some(h) = h else { return };
    let pts: Vec<[f64; 2]> = h.iter().rev().take_while(|s| now - s[0] <= TREND_SECS).copied().collect();
    if pts.len() < 2 {
        return;
    }
    let (lo, hi) = pts.iter().fold((f64::MAX, f64::MIN), |(a, b), s| (a.min(s[1]), b.max(s[1])));
    let flat = hi - lo < 1e-9 || (hi - lo) <= hi.abs().max(lo.abs()) * 1e-4;
    let span = if flat { 1.0 } else { hi - lo };
    let r = r.shrink2(vec2(2.0, 4.0));
    let line: Vec<egui::Pos2> = pts
        .iter()
        .rev()
        .map(|s| {
            let x = r.right() - ((now - s[0]) / TREND_SECS) as f32 * r.width();
            let y = if flat { r.center().y } else { r.bottom() - ((s[1] - lo) / span) as f32 * r.height() };
            pos2(x, y)
        })
        .collect();
    // A faint area under the line, as thin vertical strokes.
    for w in line.windows(2).filter(|_| !flat) {
        let x = w[0].x;
        p.line_segment([pos2(x, w[0].y), pos2(x, r.bottom())], Stroke::new((w[1].x - w[0].x).max(1.0), color.linear_multiply(0.12)));
    }
    p.add(egui::Shape::line(line, Stroke::new(1.3, color)));
}

/// A small check box glyph.
fn check_glyph(p: &egui::Painter, r: Rect, state: Option<bool>, on: Color32, off: Color32) {
    let b = Rect::from_center_size(r.center(), vec2(14.0, 14.0));
    match state {
        Some(true) => {
            p.rect_filled(b, 3.0, on);
            p.add(egui::Shape::line(
                vec![pos2(b.left() + 3.0, b.center().y), pos2(b.center().x - 1.0, b.bottom() - 3.5), pos2(b.right() - 3.0, b.top() + 3.5)],
                Stroke::new(1.8, Color32::from_gray(20)),
            ));
        }
        Some(false) => {
            p.rect_stroke(b, 3.0, Stroke::new(1.2, off), egui::StrokeKind::Inside);
        }
        None => {
            p.rect_stroke(b, 3.0, Stroke::new(1.2, on), egui::StrokeKind::Inside);
            p.rect_filled(b.shrink(4.0), 1.5, on);
        }
    }
}

struct Columns {
    name: Rect,
    value: Rect,
    min: Option<Rect>,
    max: Option<Rect>,
    trend: Option<Rect>,
    ha: Rect,
}

fn columns(row: Rect, mode: TreeMode, ha: bool) -> Columns {
    let w = row.width();
    let ha_w = match (mode, ha) {
        (TreeMode::Pick, _) => 44.0,
        (TreeMode::View, true) => 34.0,
        (TreeMode::View, false) => 0.0,
    };
    let value_w = 104.0;
    let (mm_w, trend_w) = match mode {
        TreeMode::Pick => (0.0, 0.0),
        TreeMode::View => (if w > 640.0 { 92.0 } else { 0.0 }, if w > 800.0 { 118.0 } else { 0.0 }),
    };
    let mut x = row.right();
    let mut take = |width: f32| {
        let r = Rect::from_min_max(pos2(x - width, row.top()), pos2(x, row.bottom()));
        x -= width;
        r
    };
    let ha = take(ha_w);
    let trend = (trend_w > 0.0).then(|| take(trend_w));
    let max = (mm_w > 0.0).then(|| take(mm_w));
    let min = (mm_w > 0.0).then(|| take(mm_w));
    let value = take(value_w);
    let name = Rect::from_min_max(row.min, pos2(x - 8.0, row.bottom()));
    Columns { name, value, min, max, trend, ha }
}

impl FerroApp {
    /// The tree for `devices` (ids). Returns what the user did; the caller
    /// applies it (see [`FerroApp::apply_tree_actions`]).
    /// `charted`: (device, sensor) on the graph, marked with their colour.
    /// `ha`: show the Home Assistant column.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn sensor_tree(
        &self,
        ui: &mut egui::Ui,
        st: &mut TreeUi,
        devices: &[&DeviceView],
        mode: TreeMode,
        salt: &str,
        charted: &[(String, String)],
        ha: bool,
    ) -> Vec<TreeAction> {
        let ha = ha || mode == TreeMode::Pick;
        let cfg = self.h.config.read().clone();
        // Trend lines end at each device's newest sample, so they only move
        // when a new value arrives (not on every repaint).
        let data_now: HashMap<&str, f64> = devices.iter().map(|d| (d.info.id.as_str(), crate::gui::data_now(d))).collect();
        let mut b = Builder { filtering: !st.filter.trim().is_empty(), st, cfg: &cfg, rows: Vec::new() };
        let multi = devices.len() > 1;
        for d in devices {
            b.device(d, multi);
        }
        let rows = std::mem::take(&mut b.rows);
        let by_id: HashMap<&str, &DeviceView> = devices.iter().map(|d| (d.info.id.as_str(), *d)).collect();
        let mut actions = Vec::new();

        let visuals = ui.visuals().clone();
        let text = visuals.text_color();
        let weak = visuals.weak_text_color();
        let font = FontId::proportional(13.5);
        let small = FontId::proportional(12.0);

        // ---- header ----
        let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
        let p = ui.painter_at(hr);
        let c = columns(hr, mode, ha);
        let head = |r: Rect, t: &str, right: bool| {
            let pos = if right { pos2(r.right() - 6.0, r.center().y) } else { pos2(r.left() + 6.0, r.center().y) };
            p.text(pos, if right { egui::Align2::RIGHT_CENTER } else { egui::Align2::LEFT_CENTER }, t, small.clone(), weak);
        };
        head(c.name, "Sensor", false);
        head(c.value, "Value", true);
        if let Some(r) = c.min {
            head(r, "Min", true);
        }
        if let Some(r) = c.max {
            head(r, "Max", true);
        }
        if let Some(r) = c.trend {
            head(r, "Last 2 min", true);
        }
        if ha {
            p.text(c.ha.center(), egui::Align2::CENTER_CENTER, if mode == TreeMode::Pick { "Send" } else { "HA" }, small.clone(), weak);
        }
        p.line_segment([hr.left_bottom(), hr.right_bottom()], Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color));

        if rows.is_empty() {
            ui.add_space(12.0);
            ui.label(RichText::new(if st.filter.is_empty() { "No sensors yet." } else { "No sensor matches the filter." }).weak());
            return actions;
        }

        // Dense rows, like a hardware monitor.
        let spacing = ui.spacing().item_spacing;
        ui.spacing_mut().item_spacing.y = 0.0;
        for (n, row) in rows.iter().enumerate() {
            let Some(d) = by_id.get(row.device.as_str()) else { continue };
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
            if !ui.is_rect_visible(rect) {
                continue;
            }
            let p = ui.painter_at(rect);
            let c = columns(rect, mode, ha);
            let is_group = !matches!(row.kind, RowKind::Sensor(_));
            // Position on the graph, if charted.
            let on_chart = match row.kind {
                RowKind::Sensor(i) => charted.iter().position(|(dv, id)| *dv == row.device && *id == d.sensors[i].id),
                _ => None,
            };
            let selected = on_chart.is_some();
            // Background: top-level rows tinted, others striped.
            let bg = if selected {
                ACCENT.linear_multiply(0.28)
            } else if resp.hovered() {
                visuals.widgets.hovered.weak_bg_fill.linear_multiply(0.7)
            } else if is_group && row.depth == 0 {
                visuals.faint_bg_color.gamma_multiply(1.6)
            } else if n % 2 == 1 {
                visuals.faint_bg_color
            } else {
                Color32::TRANSPARENT
            };
            p.rect_filled(rect, 0.0, bg);

            // Tree guide lines.
            for lvl in 0..row.depth {
                let x = rect.left() + 10.0 + lvl as f32 * INDENT;
                p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, weak.linear_multiply(0.18)));
            }

            let x0 = rect.left() + 4.0 + row.depth as f32 * INDENT;
            let icon_r = Rect::from_min_size(pos2(x0 + 14.0, rect.center().y - 8.0), vec2(16.0, 16.0));
            if is_group {
                icons::chevron(&p, Rect::from_min_size(pos2(x0, rect.center().y - 6.0), vec2(12.0, 12.0)), row.open, weak);
            }
            let (label_color, label_font) = match row.kind {
                RowKind::Device(k, online) => {
                    let col = if online { ACCENT } else { crate::gui::BAD };
                    icons::device(&p, icon_r, k, col);
                    (text, FontId::proportional(14.5))
                }
                RowKind::Node(k) => {
                    icons::hardware(&p, icon_r, k, icons::hardware_color(k));
                    (text, FontId::proportional(14.0))
                }
                RowKind::Category(k) => {
                    icons::category(&p, icon_r.shrink(1.0), k, icons::category_color(k));
                    (weak.lerp_to_gamma(text, 0.45), font.clone())
                }
                RowKind::Sensor(_) => (text, font.clone()),
            };
            let label_x = if matches!(row.kind, RowKind::Sensor(_)) { x0 + 14.0 } else { icon_r.right() + 6.0 };
            if let (Some(pos), RowKind::Sensor(i)) = (on_chart, &row.kind) {
                let col = crate::gui::series_color(pos, charted.len(), d.sensors[*i].kind);
                p.rect_filled(Rect::from_center_size(pos2(x0 + 6.0, rect.center().y), vec2(8.0, 8.0)), 2.0, col);
            }
            let mut name_w = c.name.right() - label_x;
            // Group size after the name, and for nodes, where they come from.
            let suffix = match row.kind {
                RowKind::Sensor(_) => String::new(),
                _ if !row.open => format!("  {}", row.members.len()),
                _ => String::new(),
            };
            let g = galley(&p, &row.label, label_font.clone(), label_color, name_w);
            name_w = g.size().x;
            let strong = matches!(row.kind, RowKind::Device(..) | RowKind::Node(_));
            let ty = rect.center().y - g.size().y / 2.0;
            p.galley(pos2(label_x, ty), g.clone(), label_color);
            if strong {
                // Faux bold: a second pass offset by a fraction of a pixel.
                p.galley(pos2(label_x + 0.5, ty), g, label_color);
            }
            if !suffix.is_empty() {
                p.text(pos2(label_x + name_w + 2.0, rect.center().y), egui::Align2::LEFT_CENTER, suffix, small.clone(), weak);
            }

            // ---- values ----
            let right_text = |r: Rect, t: &str, color: Color32, f: &FontId| {
                p.text(pos2(r.right() - 6.0, r.center().y), egui::Align2::RIGHT_CENTER, t, f.clone(), color);
            };
            if let RowKind::Sensor(i) = row.kind {
                let s = &d.sensors[i];
                let color = if s.kind.is_binary() && s.value > 0.0 { ACCENT } else { text };
                let val = value_text(s, s.value);
                right_text(c.value, &val, if d.online { color } else { weak }, &font);
                if !s.kind.is_binary() {
                    if let (Some(r), Some(st)) = (c.min, d.stats.get(&s.id)) {
                        right_text(r, &value_text(s, st[0]), weak, &font);
                    }
                    if let (Some(r), Some(st)) = (c.max, d.stats.get(&s.id)) {
                        right_text(r, &value_text(s, st[1]), weak, &font);
                    }
                    if let Some(r) = c.trend {
                        let now = data_now.get(row.device.as_str()).copied().unwrap_or(0.0);
                        sparkline(&p, r, d.history.get(&s.id), now, icons::category_color(s.kind));
                    }
                }
            } else if let RowKind::Category(k) = row.kind {
                // A small summary for collapsed groups: hottest temperature etc.
                if !row.open && matches!(k, SensorKind::Temperature | SensorKind::Load | SensorKind::FanSpeed) {
                    let max = row.members.iter().map(|&i| d.sensors[i].value).fold(f64::MIN, f64::max);
                    right_text(c.value, &format!("max {}", fmt_value(max, k)), weak, &small);
                }
            }

            // ---- Home Assistant column ----
            let sent: Vec<bool> = row.members.iter().map(|&i| cfg.sends(&row.device, &d.sensors[i])).collect();
            let state = if sent.iter().all(|b| *b) {
                Some(true)
            } else if sent.iter().all(|b| !*b) {
                Some(false)
            } else {
                None
            };
            let ids: Vec<String> = row.members.iter().map(|&i| d.sensors[i].id.clone()).collect();
            if ha {
                let ha_resp = ui.interact(c.ha, ui.scope_id().with((salt, "ha", &row.key)), Sense::click());
                let off = if ha_resp.hovered() { text } else { weak.linear_multiply(0.55) };
                match mode {
                    TreeMode::Pick => check_glyph(&p, c.ha, state, ACCENT, off),
                    TreeMode::View => icons::house(&p, Rect::from_center_size(c.ha.center(), vec2(15.0, 15.0)), state, ACCENT, off),
                }
                if ha_resp.clicked() {
                    actions.push(TreeAction::SetSends(row.device.clone(), ids.clone(), state != Some(true)));
                }
                ha_resp.on_hover_text(match (state, is_group) {
                (Some(true), false) => "Sent to Home Assistant. Click to stop sending it.",
                (Some(false), false) => "Not sent to Home Assistant. Click to send it.",
                (Some(true), true) => "All sensors here are sent to Home Assistant. Click to stop sending them.",
                (Some(false), true) => "None of these sensors are sent to Home Assistant. Click to send all of them.",
                    (None, _) => "Some of these sensors are sent to Home Assistant. Click to send all of them.",
                });
            }

            // ---- interaction ----
            let chartable: Vec<String> =
                row.members.iter().filter(|&&i| !d.sensors[i].kind.is_binary()).map(|&i| d.sensors[i].id.clone()).collect();
            let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
            if is_group {
                if resp.clicked() {
                    st.open.insert(row.key.clone(), !row.open);
                }
                if mode == TreeMode::View && resp.double_clicked() && matches!(row.kind, RowKind::Category(_)) && !chartable.is_empty() {
                    // The two clicks toggled it twice: it is as it was.
                    actions.push(TreeAction::Chart(row.device.clone(), chartable.iter().take(MAX_SERIES).cloned().collect(), false));
                }
            } else if let RowKind::Sensor(i) = row.kind {
                let s = &d.sensors[i];
                if mode == TreeMode::Pick {
                    if resp.clicked() {
                        actions.push(TreeAction::SetSends(row.device.clone(), vec![s.id.clone()], state != Some(true)));
                    }
                } else if resp.clicked() && !s.kind.is_binary() {
                    actions.push(TreeAction::Chart(row.device.clone(), vec![s.id.clone()], add));
                }
            }
            let still = ui.input(|i| i.pointer.time_since_last_movement()) > 0.6;
            let resp = if still && !is_group {
                resp.on_hover_ui_at_pointer(|ui| {
                    if let RowKind::Sensor(i) = row.kind {
                        let s = &d.sensors[i];
                        ui.label(RichText::new(&s.name).strong());
                        let node = s.hardware.as_deref().and_then(|h| d.node(h)).map(|h| h.name.clone());
                        egui::Grid::new("tip").num_columns(2).spacing([10.0, 2.0]).show(ui, |ui| {
                            if let Some(n) = node {
                                ui.label(RichText::new("Hardware").weak());
                                ui.label(n);
                                ui.end_row();
                            }
                            ui.label(RichText::new("Source").weak());
                            ui.label(s.source.as_deref().unwrap_or("OpenDeviceControl"));
                            ui.end_row();
                            ui.label(RichText::new("Sensor id").weak());
                            ui.label(RichText::new(&s.id).monospace());
                            ui.end_row();
                        });
                        if !s.kind.is_binary() && mode == TreeMode::View {
                            ui.label(RichText::new("Click to graph it, Ctrl+click to add it to the graph.").small().weak());
                        }
                    }
                })
            } else {
                resp
            };
            resp.context_menu(|ui| {
                ui.set_min_width(220.0);
                if let RowKind::Sensor(i) = row.kind {
                    let s = &d.sensors[i];
                    if !s.kind.is_binary() && mode == TreeMode::View {
                        if ui.button("📈 Graph only this").clicked() {
                            actions.push(TreeAction::Chart(row.device.clone(), vec![s.id.clone()], false));
                            ui.close();
                        }
                        let on = charted.iter().any(|(dv, id)| *dv == row.device && *id == s.id);
                        if ui.button(if on { "➖ Remove from graph" } else { "➕ Add to graph" }).clicked() {
                            actions.push(TreeAction::Chart(row.device.clone(), vec![s.id.clone()], true));
                            ui.close();
                        }
                        ui.separator();
                    }
                    if ha {
                        let mut on = state == Some(true);
                        if ui.checkbox(&mut on, "Send to Home Assistant").changed() {
                            actions.push(TreeAction::SetSends(row.device.clone(), vec![s.id.clone()], on));
                        }
                        ui.separator();
                    }
                    if ui.button("Copy name").clicked() {
                        actions.push(TreeAction::Copy(s.name.clone()));
                        ui.close();
                    }
                    if ui.button("Copy value").clicked() {
                        actions.push(TreeAction::Copy(value_text(s, s.value)));
                        ui.close();
                    }
                    if ui.button("Copy sensor id").clicked() {
                        actions.push(TreeAction::Copy(s.id.clone()));
                        ui.close();
                    }
                } else {
                    if mode == TreeMode::View && !chartable.is_empty() {
                        let n = chartable.len().min(MAX_SERIES);
                        let label = if chartable.len() > MAX_SERIES {
                            format!("📈 Graph the first {n} of these")
                        } else {
                            format!("📈 Graph these {n} sensors")
                        };
                        if ui.button(label).clicked() {
                            actions.push(TreeAction::Chart(row.device.clone(), chartable.iter().take(MAX_SERIES).cloned().collect(), false));
                            ui.close();
                        }
                        ui.separator();
                    }
                    if ha {
                        if ui.button(format!("Send all {} to Home Assistant", ids.len())).clicked() {
                            actions.push(TreeAction::SetSends(row.device.clone(), ids.clone(), true));
                            ui.close();
                        }
                        if ui.button("Send none of these").clicked() {
                            actions.push(TreeAction::SetSends(row.device.clone(), ids.clone(), false));
                            ui.close();
                        }
                        ui.separator();
                    }
                    // The motherboard's, CPU's... model, to look up.
                    if ui.button("Copy name").clicked() {
                        actions.push(TreeAction::Copy(row.label.clone()));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Expand everything below").clicked() {
                        for r in rows.iter().filter(|r| r.key.starts_with(&row.key)) {
                            st.open.insert(r.key.clone(), true);
                        }
                        st.open.insert(row.key.clone(), true);
                        ui.close();
                    }
                }
            });
        }
        ui.spacing_mut().item_spacing = spacing;
        actions
    }

    /// Apply what the tree reported; graph changes go to `target`'s chart.
    pub(crate) fn apply_tree_actions(&mut self, ctx: &egui::Context, actions: Vec<TreeAction>, target: ChartTarget) {
        for a in actions {
            match a {
                TreeAction::Chart(dev, ids, add) => match &target {
                    ChartTarget::Sensors => {
                        let keys: Vec<(String, String)> = ids.into_iter().map(|id| (dev.clone(), id)).collect();
                        if add {
                            for k in keys {
                                if let Some(p) = self.sensors_chart.iter().position(|x| *x == k) {
                                    self.sensors_chart.remove(p);
                                } else if self.sensors_chart.len() < MAX_SERIES {
                                    self.sensors_chart.push(k);
                                }
                            }
                        } else {
                            self.sensors_chart = keys;
                        }
                        self.sensors_chart_open = true;
                    }
                    ChartTarget::Device => {
                        let d = self.h.shared.read().devices.get(&dev).cloned();
                        if let Some(d) = d {
                            self.chart_ids(&dev, &d, ids, add);
                        }
                    }
                },
                TreeAction::SetSends(dev, ids, on) => {
                    let d = self.h.shared.read().devices.get(&dev).cloned();
                    if let Some(d) = d {
                        self.h.update_config(|c| {
                            for id in &ids {
                                if let Some(s) = d.sensor(id) {
                                    c.set_sends(&dev, s, on);
                                }
                            }
                        });
                    }
                }
                TreeAction::Copy(t) => ctx.copy_text(t),
            }
        }
    }

    /// Settings → Home Assistant → Choose sensors.
    pub(crate) fn ha_picker(&mut self, ctx: &egui::Context) {
        if !self.picker_open {
            return;
        }
        let shared = self.h.shared.read().clone();
        let devices = device_order(shared.devices.values());
        let mut open = true;
        let mut st = std::mem::take(&mut self.picker_tree);
        let mut actions = Vec::new();
        egui::Window::new(RichText::new("🏠 Sensors sent to Home Assistant").strong())
            .id(egui::Id::unique("ha-picker"))
            .open(&mut open)
            .default_size([820.0, 640.0])
            .min_width(520.0)
            .min_height(320.0)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "Tick the sensors Home Assistant should get. Each one becomes an entity of its device. Sensors that \
                         were already sent before keep their entity ids; detail sensors (per core, per drive, other tools' \
                         readings…) are off until you pick them. Changes apply right away.",
                    )
                    .small()
                    .weak(),
                );
                ui.add_space(4.0);
                let cfg = self.h.config.read().clone();
                let total: usize = devices.iter().map(|d| d.sensors.len()).sum();
                let sent: usize = devices.iter().map(|d| d.sensors.iter().filter(|s| cfg.sends(&d.info.id, s)).count()).sum();
                ui.horizontal_wrapped(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut st.filter).hint_text("🔍 Filter sensors…").desired_width(220.0));
                    ui.toggle_value(&mut st.only_sent, "Only sent");
                    ui.separator();
                    let shown: Vec<(String, Vec<String>)> = devices
                        .iter()
                        .map(|d| {
                            let ids = d
                                .sensors
                                .iter()
                                .filter(|s| matches(&st.filter, d, s) && (!st.only_sent || cfg.sends(&d.info.id, s)))
                                .map(|s| s.id.clone())
                                .collect();
                            (d.info.id.clone(), ids)
                        })
                        .collect();
                    let what = if st.filter.is_empty() { "all" } else { "all shown" };
                    if ui.button(format!("Send {what}")).clicked() {
                        actions.extend(shown.iter().map(|(d, ids)| TreeAction::SetSends(d.clone(), ids.clone(), true)));
                    }
                    if ui.button(format!("Send none ({what})")).clicked() {
                        actions.extend(shown.iter().map(|(d, ids)| TreeAction::SetSends(d.clone(), ids.clone(), false)));
                    }
                    if ui.button("Defaults").on_hover_text("Send the standard sensors only, as a fresh install does").clicked() {
                        self.h.update_config(|c| c.ha_sensors.clear());
                    }
                    ui.separator();
                    if ui.button("Expand all").clicked() {
                        st.set_all(true);
                    }
                    if ui.button("Collapse all").clicked() {
                        st.set_all(false);
                    }
                });
                ui.label(RichText::new(format!("Sending {sent} of {total} sensors.")).color(ACCENT));
                if !cfg.mqtt.enabled {
                    ui.label(RichText::new("The Home Assistant integration is off: nothing is sent until you enable it.").small().color(WARN));
                }
                ui.add_space(2.0);
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    actions.extend(self.sensor_tree(ui, &mut st, &devices, TreeMode::Pick, "picker", &[], true));
                });
            });
        self.picker_tree = st;
        if !open {
            self.picker_open = false;
        }
        self.apply_tree_actions(ctx, actions, ChartTarget::Sensors);
    }

    /// The graph at the top of the Sensors page: any sensors of any devices.
    fn sensors_graph(&mut self, ui: &mut egui::Ui, devices: &[&DeviceView]) {
        let find = |dev: &str, id: &str| devices.iter().find(|d| d.info.id == dev).and_then(|d| Some((*d, d.sensor(id)?)));
        // First visit: the CPU and GPU temperatures, when there are some.
        if !self.sensors_chart_init {
            self.sensors_chart_init = true;
            for id in ["cpu_temp", "gpu_temp"] {
                if find(opendevicecontrol_core::system::ID, id).is_some() {
                    self.sensors_chart.push((opendevicecontrol_core::system::ID.into(), id.into()));
                }
            }
        }
        self.sensors_chart.retain(|(dev, id)| find(dev, id).is_some());
        let header = if self.sensors_chart.is_empty() { "Graph".to_string() } else { format!("Graph ({})", self.sensors_chart.len()) };
        let resp = egui::CollapsingHeader::new(RichText::new(header).strong())
            .id_salt("sensors-graph")
            .open(Some(self.sensors_chart_open))
            .show(ui, |ui| {
                if self.sensors_chart.is_empty() {
                    ui.label(RichText::new("Click a sensor below to graph it; Ctrl+click (or Shift+click) adds more.").weak());
                    return;
                }
                let n = self.sensors_chart.len();
                let mut remove = None;
                ui.horizontal_wrapped(|ui| {
                    for (i, (dev, id)) in self.sensors_chart.iter().enumerate() {
                        let Some((d, s)) = find(dev, id) else { continue };
                        let color = crate::gui::series_color(i, n, s.kind);
                        let node = s.hardware.as_deref().and_then(|h| d.node(h)).map(|h| h.name.clone()).unwrap_or_else(|| d.info.model.clone());
                        egui::Frame::new().fill(color.linear_multiply(0.12)).corner_radius(10.0).inner_margin(egui::Margin::symmetric(8, 2)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("●").color(color));
                                ui.label(RichText::new(&s.name).strong()).on_hover_text(&node);
                                ui.label(RichText::new(fmt_value(s.value, s.kind)).color(color));
                                if ui.small_button("✖").on_hover_text("Remove from the graph").clicked() {
                                    remove = Some(i);
                                }
                            });
                        });
                    }
                    if ui.button("Clear").clicked() {
                        remove = Some(usize::MAX);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (label, secs) in RANGES.iter().rev() {
                            if ui.selectable_label(self.range_secs == *secs, *label).clicked() {
                                self.range_secs = *secs;
                            }
                        }
                    });
                });
                match remove {
                    Some(usize::MAX) => self.sensors_chart.clear(),
                    Some(i) => {
                        self.sensors_chart.remove(i);
                    }
                    None => {}
                }
                let data: Vec<crate::gui::PlotSeries> = self
                    .sensors_chart
                    .iter()
                    .enumerate()
                    .filter_map(|(i, (dev, id))| {
                        let (d, s) = find(dev, id)?;
                        Some(crate::gui::PlotSeries {
                            name: s.name.clone(),
                            series: d.history.get(id)?,
                            color: crate::gui::series_color(i, n, s.kind),
                            kind: s.kind,
                        })
                    })
                    .collect();
                if data.windows(2).any(|w| w[0].kind.unit() != w[1].kind.unit()) {
                    ui.label(RichText::new("These sensors have different units; the axis shows the first one's.").small().color(WARN));
                }
                self.plot_series(ui, &data, self.range_secs, 240.0, "sensors-graph-plot", true);
            });
        if resp.header_response.clicked() {
            self.sensors_chart_open = !self.sensors_chart_open;
        }
    }

    /// The Sensors page: a graph, then every device and every sensor.
    pub(crate) fn sensors_page(&mut self, ui: &mut egui::Ui) {
        let shared = self.h.shared.read().clone();
        let devices = device_order(shared.devices.values());
        let cfg = self.h.config.read().clone();
        let ha = cfg.mqtt.enabled;
        ui.horizontal(|ui| {
            ui.heading("Sensors");
            let total: usize = devices.iter().map(|d| d.sensors.len()).sum();
            let mut info = format!("{total} sensors on {} devices", devices.len());
            if ha {
                let sent: usize = devices.iter().map(|d| d.sensors.iter().filter(|s| cfg.sends(&d.info.id, s)).count()).sum();
                info.push_str(&format!(" · {sent} sent to Home Assistant"));
            }
            ui.label(RichText::new(info).weak());
        });
        for s in &shared.sensor_status {
            let good = s.contains("connected") || s.contains("reading") || (s.contains(" sensors") && !s.contains("no sensors"));
            ui.label(RichText::new(format!("{} {s}", if good { "●" } else { "○" })).small().color(if good { ACCENT } else { Color32::GRAY }));
        }
        ui.add_space(4.0);
        crate::gui::section(ui, "", |ui| self.sensors_graph(ui, &devices));
        ui.add_space(6.0);
        let mut st = std::mem::take(&mut self.tree);
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::TextEdit::singleline(&mut st.filter).hint_text("🔍 Filter (name, hardware, type…)").desired_width(260.0));
            if !st.filter.is_empty() && ui.small_button("✖").clicked() {
                st.filter.clear();
            }
            ui.separator();
            if ui.button("Expand all").clicked() {
                st.set_all(true);
            }
            if ui.button("Collapse all").clicked() {
                st.set_all(false);
            }
            if ui.button("Reset min/max").on_hover_text("Forget the lowest and highest values seen").clicked() {
                let _ = self.h.cmd.send(crate::service::Command::ResetStats);
            }
            if ui.button("📋 Copy report").on_hover_text("Copy every sensor with its value, min and max as text").clicked() {
                ui.ctx().copy_text(report(&devices));
            }
            if ha {
                ui.toggle_value(&mut st.only_sent, "🏠 Only sent to HA");
                ui.separator();
                if ui.button("🏠 Choose what Home Assistant gets…").clicked() {
                    self.picker_open = true;
                }
            }
        });
        let hint = if ha {
            "Click a sensor to graph it · Ctrl+click adds it · double-click a group to graph the group · right-click for more · the house column toggles Home Assistant"
        } else {
            "Click a sensor to graph it · Ctrl+click adds it · double-click a group to graph the group · right-click for more"
        };
        ui.label(RichText::new(hint).small().weak());
        ui.add_space(4.0);
        if !ha {
            st.only_sent = false;
        }
        let charted = self.sensors_chart.clone();
        let actions = self.sensor_tree(ui, &mut st, &devices, TreeMode::View, "main", &charted, ha);
        self.tree = st;
        self.apply_tree_actions(ui.ctx(), actions, ChartTarget::Sensors);
    }

    /// This PC's page: its tree, below the page chart (a click charts there).
    /// A device page's sensors as a tree, like the Sensors page.
    pub(crate) fn device_tree(&mut self, ui: &mut egui::Ui, d: &DeviceView, title: &str) {
        let mut st = self.device_trees.remove(&d.info.id).unwrap_or_default();
        let ha = self.h.config.read().mqtt.enabled;
        let charted: Vec<(String, String)> = self.chart_for(&d.info.id).1.into_iter().map(|id| (d.info.id.clone(), id)).collect();
        crate::gui::section(ui, "", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(title).strong());
                ui.label(RichText::new(format!("{} sensors", d.sensors.len())).weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Reset min/max").clicked() {
                        let _ = self.h.cmd.send(crate::service::Command::ResetStats);
                    }
                    if ui.button("Collapse all").clicked() {
                        st.set_all(false);
                    }
                    if ui.button("Expand all").clicked() {
                        st.set_all(true);
                    }
                    ui.add(egui::TextEdit::singleline(&mut st.filter).hint_text("🔍 Filter").desired_width(160.0));
                });
            });
            let hint = if ha {
                "Click a sensor to chart it above · Ctrl+click adds it · the house column toggles Home Assistant"
            } else {
                "Click a sensor to chart it above · Ctrl+click adds it"
            };
            ui.label(RichText::new(hint).small().weak());
            let salt = format!("dev-{}", d.info.id);
            let actions = self.sensor_tree(ui, &mut st, &[d], TreeMode::View, &salt, &charted, ha);
            self.device_trees.insert(d.info.id.clone(), st);
            self.apply_tree_actions(ui.ctx(), actions, ChartTarget::Device);
        });
    }
}
