//! Main window (egui). The window can be hidden to the tray; the device
//! service and MQTT bridge keep running independently of it.

use crate::config::{eval_curve, Config, FanPolicy};
use crate::service::{Command, DeviceView, Handle, MqttStatus, Series};
use eframe::egui::{self, Color32, RichText};
use egui_plot::{Legend, Line, Plot, PlotPoints, Points, VLine};
use opendevicecontrol_core::{ControlKind, ControlValue, SensorKind};
use std::collections::HashMap;
use std::sync::Arc;

#[cfg(test)]
#[path = "gui_shots.rs"]
mod shots;

pub(crate) const ACCENT: Color32 = Color32::from_rgb(34, 190, 178);
pub(crate) const WARN: Color32 = Color32::from_rgb(240, 180, 40);
pub(crate) const BAD: Color32 = Color32::from_rgb(230, 80, 70);

#[derive(PartialEq, Clone)]
enum Page {
    /// Every device's sensors as one tree.
    Sensors,
    Device(String),
    Settings,
}

pub struct FerroApp {
    pub(crate) h: Handle,
    page: Option<Page>,
    /// Draft copy of the settings page (applied on "Save").
    draft: Config,
    draft_dirty: bool,
    autostart_error: Option<String>,
    curve_drafts: HashMap<String, Vec<(f64, u8)>>,
    fixed_drafts: HashMap<String, u8>,
    show_password: bool,
    /// Per device: chart title and the sensor ids it shows.
    pub(crate) charts: HashMap<String, (String, Vec<String>)>,
    /// Unsaved key-action edits per "device/key".
    pub(crate) key_drafts: HashMap<String, crate::config::KeyAction>,
    /// Time range of the history chart, seconds.
    pub(crate) range_secs: f64,
    /// The Sensors page graph: (device, sensor) pairs.
    pub(crate) sensors_chart: Vec<(String, String)>,
    pub(crate) sensors_chart_init: bool,
    pub(crate) sensors_chart_open: bool,
    /// Sensors page tree, This PC page tree and Home Assistant chooser.
    pub(crate) tree: crate::gui_sensors::TreeUi,
    /// Each device page's tree (filter, open groups), by device id.
    pub(crate) device_trees: HashMap<String, crate::gui_sensors::TreeUi>,
    pub(crate) picker_tree: crate::gui_sensors::TreeUi,
    pub(crate) picker_open: bool,
    /// Result of the last sensor service install / remove.
    sensor_service_msg: Arc<parking_lot::Mutex<Option<String>>>,
    /// Set when the window closes to the tray (OpenDeviceControl keeps running).
    to_tray: bool,
    placement: Option<Placement>,
}

/// The open window's context. `None` while OpenDeviceControl is in the tray: the
/// window is then closed completely, which frees its OpenGL context and the
/// graphics driver's memory, and is opened again on demand with the same
/// [`FerroApp`] state.
static WINDOW: std::sync::Mutex<Option<egui::Context>> = std::sync::Mutex::new(None);
/// Asks the GUI thread to open the window.
static OPEN: std::sync::OnceLock<crossbeam_channel::Sender<()>> = std::sync::OnceLock::new();

fn window() -> std::sync::MutexGuard<'static, Option<egui::Context>> {
    WINDOW.lock().unwrap_or_else(|e| e.into_inner())
}

/// Show the main window: bring the open one to the front, or open it.
/// Safe to call from any thread.
pub fn show() {
    let open = window().clone();
    if let Some(ctx) = open {
        show_window(&ctx);
    } else if let Some(tx) = OPEN.get() {
        let _ = tx.try_send(());
    }
}

fn show_window(ctx: &egui::Context) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW};
        let hwnd = HWND.load(std::sync::atomic::Ordering::Relaxed);
        if hwnd != 0 {
            // A minimized window runs no frames on Windows, so it must be
            // restored directly; egui then repaints.
            unsafe {
                ShowWindow(hwnd as _, SW_SHOW);
                ShowWindow(hwnd as _, SW_RESTORE);
                SetForegroundWindow(hwnd as _);
            }
        }
    }
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    ctx.request_repaint();
}

#[cfg(windows)]
static HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Where the window was, so it reopens there.
#[derive(Clone, Copy)]
pub(crate) struct Placement {
    pos: egui::Pos2,
    size: egui::Vec2,
    maximized: bool,
}

/// Run the GUI until OpenDeviceControl quits. The tray icon and "show" requests from a
/// second launch open the window; closing it to the tray closes it.
pub fn run(h: Handle, start_hidden: bool, start_page: Option<String>) -> eframe::Result<()> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let _ = OPEN.set(tx);
    let show_fn: crate::tray::ShowFn = Arc::new(show);
    crate::tray::spawn(h.clone(), show_fn.clone());
    crate::ipc::set_show(show_fn);
    h.on_update(|| {
        if let Some(ctx) = window().as_ref() {
            ctx.request_repaint();
        }
    });

    let mut app = FerroApp::new(h.clone());
    app.page = Some(match start_page.as_deref() {
        Some("settings") => Page::Settings,
        Some(p) => Page::Device(p.to_string()),
        None => Page::Sensors,
    });
    let quitting = || h.quitting.load(std::sync::atomic::Ordering::Relaxed);
    let mut opened = false;
    // Without a tray icon a hidden window could not be opened again.
    let mut wait = start_hidden && crate::tray::wait_ready(std::time::Duration::from_secs(3));
    if start_hidden && !wait {
        log::warn!("no tray icon; showing the window instead of starting minimized");
    }
    loop {
        if wait {
            // In the tray: no window, no graphics context. Tray Quit and
            // `opendevicecontrol --quit` exit the process from their own threads.
            loop {
                match rx.recv_timeout(std::time::Duration::from_millis(500)) {
                    Ok(()) => break,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) if !quitting() => {}
                    Err(_) => return Ok(()),
                }
            }
        }
        wait = true;
        match open_window(&mut app) {
            Ok(()) => opened = true,
            // No display at all: the caller keeps the service running.
            Err(e) if !opened => return Err(e),
            Err(e) => log::error!("could not open the window: {e}"),
        }
        if quitting() {
            return Ok(());
        }
    }
}

/// Open the window and run it until it is closed.
fn open_window(app: &mut FerroApp) -> eframe::Result<()> {
    let icon = egui::IconData { rgba: crate::icon::rgba(64), width: 64, height: 64 };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("OpenDeviceControl")
        .with_app_id("opendevicecontrol")
        .with_inner_size([1100.0, 860.0])
        // Below ~820 px the two-column sections stack; this is the
        // narrowest width at which every section still fits.
        .with_min_inner_size([620.0, 480.0])
        .with_icon(Arc::new(icon));
    if let Some(p) = app.placement {
        viewport = viewport.with_position(p.pos).with_inner_size(p.size).with_maximized(p.maximized);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    app.to_tray = false;
    let r = eframe::run_native(
        "opendevicecontrol",
        options,
        Box::new(|cc| {
            #[cfg(windows)]
            {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                if let Ok(wh) = cc.window_handle() {
                    if let RawWindowHandle::Win32(w) = wh.as_raw() {
                        HWND.store(w.hwnd.get(), std::sync::atomic::Ordering::Relaxed);
                        // Opened from the tray or a second launch: come to
                        // the front rather than flash in the taskbar.
                        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(w.hwnd.get() as _) };
                    }
                }
            }
            style(&cc.egui_ctx);
            *window() = Some(cc.egui_ctx.clone());
            Ok(Box::new(OpenWindow(app)))
        }),
    );
    *window() = None;
    #[cfg(windows)]
    HWND.store(0, std::sync::atomic::Ordering::Relaxed);
    r
}

/// The app while its window is open (the app itself outlives the window).
struct OpenWindow<'a>(&'a mut FerroApp);

impl eframe::App for OpenWindow<'_> {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        eframe::App::ui(self.0, ui, frame);
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        eframe::App::on_exit(self.0, gl);
    }
}

fn style(ctx: &egui::Context) {
    ctx.set_visuals(egui::Visuals::dark());
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        // Chart tooltips must follow the cursor without waiting for it to stop.
        s.interaction.tooltip_delay = 0.0;
        s.interaction.show_tooltips_only_when_still = false;
        s.visuals.selection.bg_fill = ACCENT.linear_multiply(0.6);
        s.visuals.hyperlink_color = ACCENT;
    });
}

impl FerroApp {
    fn new(h: Handle) -> Self {
        let draft = h.config.read().clone();
        Self {
            h,
            page: None,
            draft,
            draft_dirty: false,
            autostart_error: None,
            curve_drafts: HashMap::new(),
            fixed_drafts: HashMap::new(),
            show_password: false,
            charts: HashMap::new(),
            key_drafts: HashMap::new(),
            range_secs: 900.0,
            sensors_chart: Vec::new(),
            sensors_chart_init: false,
            sensors_chart_open: true,
            tree: Default::default(),
            device_trees: HashMap::new(),
            picker_tree: Default::default(),
            picker_open: false,
            sensor_service_msg: Default::default(),
            to_tray: false,
            placement: None,
        }
    }
}

impl eframe::App for FerroApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Remember where the window is, to reopen it there.
        let (outer, inner, maximized, minimized) = ctx.input(|i| {
            let v = i.viewport();
            (v.outer_rect, v.inner_rect, v.maximized.unwrap_or(false), v.minimized.unwrap_or(false))
        });
        if let (Some(outer), Some(inner), false) = (outer, inner, minimized) {
            self.placement = Some(match self.placement {
                // While maximized keep the restored size and place.
                Some(p) if maximized => Placement { maximized: true, ..p },
                _ => Placement { pos: outer.min, size: inner.size(), maximized },
            });
        }
        // Close button -> to the tray (the window closes, OpenDeviceControl keeps
        // running), or quit.
        if ctx.input(|i| i.viewport().close_requested()) {
            // Only to a working tray icon: otherwise nothing could reopen it.
            if self.h.config.read().general.close_to_tray
                && crate::tray::is_ready()
                && !self.h.quitting.load(std::sync::atomic::Ordering::Relaxed)
            {
                self.to_tray = true;
                // From now on "show" opens a new window.
                *window() = None;
            } else {
                let _ = self.h.shutdown();
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(1));

        let shared = self.h.shared.read().clone();
        // Open the first device once it has been discovered (discovery runs
        // in the background and usually finishes after the first frame).
        if self.page.is_none() {
            self.page = shared.devices.keys().next().map(|k| Page::Device(k.clone()));
        }

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("⚡ OpenDeviceControl").strong().size(18.0).color(ACCENT));
                ui.separator();
                if ui.selectable_label(self.page == Some(Page::Sensors), "📊 Sensors").clicked() {
                    self.page = Some(Page::Sensors);
                }
                // This PC first, then devices by id.
                let mut tabs: Vec<(&String, &DeviceView)> = shared.devices.iter().collect();
                tabs.sort_by_key(|(id, d)| (d.info.kind != opendevicecontrol_core::DeviceKind::Computer, (*id).clone()));
                for (id, d) in tabs {
                    let p = Page::Device(id.clone());
                    let label = RichText::new(format!("{} {}", if d.online { "●" } else { "○" }, d.info.model))
                        .color(if d.online { ACCENT } else { BAD });
                    if ui.selectable_label(self.page.as_ref() == Some(&p), label).clicked() {
                        self.page = Some(p);
                    }
                }
                if shared.devices.is_empty() {
                    ui.label(RichText::new("searching for devices…").italics().weak());
                }
                if ui.selectable_label(self.page == Some(Page::Settings), "⚙ Settings").clicked() {
                    self.page = Some(Page::Settings);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (txt, col) = match &shared.mqtt {
                        MqttStatus::Disabled => ("Home Assistant: off".to_string(), Color32::GRAY),
                        MqttStatus::Connecting => ("Home Assistant: connecting…".to_string(), WARN),
                        MqttStatus::Connected => ("Home Assistant: connected".to_string(), ACCENT),
                        MqttStatus::Error(_) => ("Home Assistant: error".to_string(), BAD),
                    };
                    let r = ui.label(RichText::new(txt).color(col));
                    if let MqttStatus::Error(e) = &shared.mqtt {
                        r.on_hover_text(e);
                    }
                });
            });
            ui.add_space(4.0);
        });

        if !shared.conflicts.is_empty() {
            egui::Panel::top("conflicts").show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("⚠").color(WARN).size(16.0));
                    ui.label(
                        RichText::new(format!(
                            "Other Corsair software is running: {}. It talks to the same USB devices, which can cause dropped readings and \
                             will fight over fan settings. OpenDeviceControl retries automatically, but you should quit it or disable its services.",
                            shared.conflicts.join(", ")
                        ))
                        .color(WARN),
                    );
                });
            });
        }

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.page.clone() {
                Some(Page::Device(id)) => match shared.devices.get(&id) {
                    Some(d) if d.info.kind == opendevicecontrol_core::DeviceKind::PowerSupply => self.device_page(ui, d),
                    Some(d) => self.peripheral_page(ui, d),
                    None => {
                        ui.label("Device disconnected.");
                    }
                },
                Some(Page::Settings) => self.settings_page(ui),
                Some(Page::Sensors) if !shared.devices.is_empty() => self.sensors_page(ui),
                Some(Page::Sensors) | None => {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.spinner();
                        ui.label("Looking for supported Corsair devices…");
                        ui.label(
                            RichText::new(
                                "On Linux, make sure the udev rule is installed (see README) so OpenDeviceControl can access USB HID devices.",
                            )
                            .small()
                            .weak(),
                        );
                    });
                }
            });
        });
        self.ha_picker(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if !self.to_tray {
            let _ = self.h.shutdown();
        }
    }
}

pub(crate) fn fmt_value(v: f64, kind: SensorKind) -> String {
    match kind {
        SensorKind::Duration => fmt_duration(v),
        SensorKind::Charging => (if v > 0.0 { "charging" } else { "not charging" }).to_string(),
        SensorKind::State => (if v > 0.0 { "yes" } else { "no" }).to_string(),
        _ => format!("{:.*} {}", kind.precision(), v, kind.unit()),
    }
}

fn fmt_duration(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (d, h, m, s) = (s / 86400, (s / 3600) % 24, (s / 60) % 60, s % 60);
    // Short enough for the sensor tree's value columns.
    if d >= 365 {
        format!("{:.1} years", secs / (365.25 * 86400.0))
    } else if d > 0 {
        format!("{d}d {h}h {m:02}m")
    } else {
        format!("{h:02}:{m:02}:{s:02}")
    }
}

/// Series colours, in order of appearance in a chart.
pub(crate) const SERIES_COLORS: [Color32; 6] = [
    Color32::from_rgb(240, 180, 40),
    Color32::from_rgb(34, 190, 178),
    Color32::from_rgb(170, 120, 240),
    Color32::from_rgb(230, 80, 70),
    Color32::from_rgb(90, 160, 240),
    Color32::from_rgb(130, 200, 90),
];

/// Colour of series `i` of `n` in a chart: a lone series takes the colour
/// of its sensor type (temperatures orange, voltages yellow…).
pub(crate) fn series_color(i: usize, n: usize, kind: SensorKind) -> Color32 {
    if n == 1 { crate::icons::category_color(kind) } else { SERIES_COLORS[i % SERIES_COLORS.len()] }
}

/// Layouts narrower than this stack the two-column sections vertically.
const TWO_COLUMN_MIN_WIDTH: f32 = 820.0;

/// Chart time ranges offered above the chart.
pub(crate) const RANGES: [(&str, f64); 4] = [("5 min", 300.0), ("15 min", 900.0), ("30 min", 1800.0), ("1 hour", 3600.0)];

fn fmt_ago(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    match s {
        0..=1 => "now".into(),
        2..=59 => format!("{s} s ago"),
        _ => format!("{} min {:02} s ago", s / 60, s % 60),
    }
}

/// A framed section that always fills the width it is given, so sections in
/// neighbouring columns line up with each other and with the chart above.
pub(crate) fn section<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !title.is_empty() {
                ui.label(RichText::new(title).strong());
                ui.add_space(2.0);
            }
            add(ui)
        })
        .inner
}

/// Lay out two sections side by side when there is room, stacked otherwise.
fn two_columns(ui: &mut egui::Ui, left: impl FnOnce(&mut egui::Ui), right: impl FnOnce(&mut egui::Ui)) {
    if ui.available_width() >= TWO_COLUMN_MIN_WIDTH {
        ui.columns(2, |cols| {
            left(&mut cols[0]);
            right(&mut cols[1]);
        });
    } else {
        left(ui);
        ui.add_space(6.0);
        right(ui);
    }
}

/// A label that can be clicked to chart a sensor. Returns true when clicked.
pub(crate) fn chart_link(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>, charted: bool, hint: &str) -> bool {
    let text: egui::WidgetText = text.into();
    let text = if charted { text.color(ACCENT) } else { text };
    let r = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
    let r = r.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(hint);
    r.clicked()
}

pub(crate) const CHART_HINT: &str = "Click to chart. Shift+click to add to the current chart.";

fn rail_cells(v: &impl Fn(&str) -> f64, key: &str) -> [(String, String); 3] {
    [
        (format!("rail_{key}_voltage"), format!("{:.3} V", v(&format!("rail_{key}_voltage")))),
        (format!("rail_{key}_current"), format!("{:.2} A", v(&format!("rail_{key}_current")))),
        (format!("rail_{key}_power"), format!("{:.1} W", v(&format!("rail_{key}_power")))),
    ]
}

/// Sample closest in time to `t`, if one is within 10 s.
fn nearest(h: &Series, t: f64) -> Option<[f64; 2]> {
    let i = h.partition_point(|s| s[0] < t);
    let cand = [i.checked_sub(1).and_then(|j| h.get(j)), h.get(i)];
    cand.into_iter()
        .flatten()
        .min_by(|a, b| (a[0] - t).abs().total_cmp(&(b[0] - t).abs()))
        .filter(|s| (s[0] - t).abs() <= 10.0)
        .copied()
}

/// One line of a chart.
pub(crate) struct PlotSeries<'a> {
    pub name: String,
    pub series: &'a Series,
    pub color: Color32,
    pub kind: SensorKind,
}

/// Time of a device's newest sample (seconds since start): charts and trend
/// lines end here, so they move only when a new value arrives.
pub(crate) fn data_now(d: &DeviceView) -> f64 {
    d.history.values().filter_map(|s| s.back()).map(|p| p[0]).fold(0.0, f64::max)
}

/// One hovered value in the chart tooltip.
struct HoverRow {
    name: String,
    color: Color32,
    value: f64,
    kind: SensorKind,
    t: f64,
}

impl FerroApp {
    /// The page chart (fixed height, the page's time range).
    pub(crate) fn history_chart(&self, ui: &mut egui::Ui, d: &DeviceView, series: &[String], _now: f64) {
        let data: Vec<PlotSeries> = series
            .iter()
            .enumerate()
            .filter_map(|(i, id)| {
                let s = d.sensor(id)?;
                Some(PlotSeries {
                    name: if s.estimated { format!("{} (est.)", s.name) } else { s.name.clone() },
                    series: d.history.get(id)?,
                    color: series_color(i, series.len(), s.kind),
                    kind: s.kind,
                })
            })
            .collect();
        self.plot_series(ui, &data, self.range_secs, 240.0, ("history", &d.info.id), false);
    }

    /// History chart with a hover cursor: a vertical line, a marker on
    /// every series and a tooltip listing all values at that time.
    ///
    /// The time axis ends at the newest sample and the bounds are fixed, so
    /// the chart only changes when a new value arrives — not while the
    /// pointer moves or the window repaints.
    pub(crate) fn plot_series(
        &self,
        ui: &mut egui::Ui,
        series: &[PlotSeries],
        range: f64,
        height: f32,
        id: impl std::hash::Hash + std::fmt::Debug,
        fit_y: bool,
    ) {
        let data: Vec<(String, &Series, Color32, SensorKind)> =
            series.iter().map(|s| (s.name.clone(), s.series, s.color, s.kind)).collect();
        let now = data.iter().filter_map(|x| x.1.back()).map(|p| p[0]).fold(0.0, f64::max);
        let unit = data.first().map(|x| x.3.unit()).unwrap_or("");
        let to_x = move |t: f64| (t - now) / 60.0;
        // Graph windows fit the visible values; page charts start at zero.
        // A little room above and below, so a steady value is a line in the
        // middle rather than a 0–10 axis.
        let visible = || data.iter().flat_map(|x| x.1.iter().filter(|s| now - s[0] <= range).map(|s| s[1]));
        let lo = visible().fold(f64::INFINITY, f64::min);
        let hi = visible().fold(f64::NEG_INFINITY, f64::max);
        let (lowest, highest) = if lo.is_finite() && hi.is_finite() {
            let pad = ((hi - lo) * 0.15).max(hi.abs().max(lo.abs()) * 0.02).max(0.01);
            if fit_y {
                (lo - pad, hi + pad)
            } else {
                (lo.min(0.0), (hi + pad).max(lo.min(0.0) + 1.0))
            }
        } else {
            (0.0, 1.0)
        };

        let resp = Plot::new(id)
            .height(height)
            .legend(Legend::default().position(egui_plot::Corner::LeftTop))
            .y_axis_label(unit)
            .x_axis_formatter(|m, _| if m.value.abs() < 1e-9 { "now".into() } else { format!("{:.0} min", m.value) })
            .auto_bounds(false)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .allow_boxed_zoom(false)
            .allow_double_click_reset(false)
            .show_x(false)
            .show_y(false)
            .show(ui, |p| {
                p.set_plot_bounds_x(-range / 60.0..=0.0);
                p.set_plot_bounds_y(lowest..=highest);
                for (name, h, color, _) in &data {
                    let pts: PlotPoints = h.iter().filter(|s| now - s[0] <= range).map(|s| [to_x(s[0]), s[1]]).collect();
                    p.line(Line::new(name.clone(), pts).color(*color).width(1.5));
                }
                let c = p.pointer_coordinate()?;
                if c.x < -range / 60.0 || c.x > 0.0 {
                    return None;
                }
                let t = now + c.x * 60.0;
                let mut rows = Vec::new();
                for (name, h, color, kind) in &data {
                    if let Some(s) = nearest(h, t) {
                        p.points(Points::new("", vec![[to_x(s[0]), s[1]]]).radius(4.0).color(*color));
                        rows.push(HoverRow { name: name.clone(), color: *color, value: s[1], kind: *kind, t: s[0] });
                    }
                }
                if let Some(r) = rows.first() {
                    p.vline(VLine::new("", to_x(r.t)).color(Color32::from_gray(140)));
                }
                Some(rows)
            });

        if let Some(rows) = resp.inner.filter(|r| !r.is_empty()) {
            if resp.response.hovered() {
                let t = rows[0].t;
                resp.response.on_hover_ui_at_pointer(|ui| {
                    ui.label(RichText::new(fmt_ago(now - t)).weak());
                    egui::Grid::new("chart-tip").num_columns(3).spacing([8.0, 2.0]).show(ui, |ui| {
                        for r in rows {
                            ui.label(RichText::new("■").color(r.color));
                            ui.label(r.name);
                            ui.label(RichText::new(fmt_value(r.value, r.kind)).strong());
                            ui.end_row();
                        }
                    });
                });
            }
        }
    }
}

struct CardDef {
    title: &'static str,
    value: String,
    sub: String,
    color: Color32,
    chart_title: &'static str,
    series: &'static [&'static str],
}

impl FerroApp {
    /// The chart shown when the user hasn't picked one for this device.
    pub(crate) fn default_chart(d: &DeviceView) -> (String, Vec<String>) {
        let has = |id: &str| d.sensor(id).is_some();
        if d.info.kind == opendevicecontrol_core::DeviceKind::Computer {
            let s: Vec<String> = ["cpu_load", "gpu_load", "memory_load"].iter().filter(|i| has(i)).map(|i| i.to_string()).collect();
            return ("CPU, GPU and memory load".into(), s);
        }
        if has("input_power") {
            return ("Power".into(), vec!["input_power".into(), "output_power".into()]);
        }
        match d.sensors.iter().find(|s| !s.diagnostic && !s.kind.is_binary()) {
            Some(s) => (s.name.clone(), vec![s.id.clone()]),
            None => (String::new(), Vec::new()),
        }
    }

    pub(crate) fn chart_for(&self, device: &str) -> (String, Vec<String>) {
        self.charts.get(device).cloned().unwrap_or_else(|| {
            ("Power".to_string(), vec!["input_power".to_string(), "output_power".to_string()])
        })
    }

    /// Chart `ids` (replace) or, with `add`, toggle them in the device's chart.
    pub(crate) fn chart_ids(&mut self, device: &str, d: &DeviceView, ids: Vec<String>, add: bool) {
        let (title, mut series) = self.chart_for(device);
        if add {
            for id in ids {
                if let Some(pos) = series.iter().position(|s| *s == id) {
                    if series.len() > 1 {
                        series.remove(pos);
                    }
                } else {
                    series.push(id);
                }
            }
        } else {
            series = ids;
        }
        let title = match series.as_slice() {
            [only] => d.sensor(only).map(|s| s.name.clone()).unwrap_or(title),
            _ => "Custom chart".to_string(),
        };
        self.charts.insert(device.to_string(), (title, series));
    }

    /// Chart `id` (replace) or, with Shift held, toggle it in the current chart.
    pub(crate) fn select_series(&mut self, ctx: &egui::Context, device: &str, d: &DeviceView, id: &str) {
        let add = ctx.input(|i| i.modifiers.shift || i.modifiers.command);
        let (title, mut series) = self.chart_for(device);
        let name = d.sensor(id).map(|s| s.name.clone()).unwrap_or_else(|| id.to_string());
        if add {
            if let Some(pos) = series.iter().position(|s| s == id) {
                if series.len() > 1 {
                    series.remove(pos);
                }
            } else {
                series.push(id.to_string());
            }
            let title = match series.as_slice() {
                [only] => d.sensor(only).map(|s| s.name.clone()).unwrap_or(title),
                _ => "Custom chart".to_string(),
            };
            self.charts.insert(device.to_string(), (title, series));
        } else {
            self.charts.insert(device.to_string(), (name, vec![id.to_string()]));
        }
    }

    fn device_page(&mut self, ui: &mut egui::Ui, d: &DeviceView) {
        ui.horizontal(|ui| {
            ui.heading(d.info.display_name());
            if d.online {
                ui.label(RichText::new("online").color(ACCENT));
            } else {
                ui.label(RichText::new("offline").color(BAD));
            }
            if let Some(e) = &d.last_error {
                ui.label(RichText::new(format!("⚠ {e}")).color(WARN));
            }
        });
        if d.sensors.is_empty() {
            ui.spinner();
            return;
        }
        let v = |id: &str| d.value(id).unwrap_or(0.0);
        let dev = d.info.id.clone();
        let (chart_title, chart_series) = self.chart_for(&dev);

        // ---- summary cards (click to chart) ----
        let rpm = v("fan_speed");
        let cards = [
            CardDef {
                title: "Input power",
                value: format!("{:.0} W", v("input_power")),
                sub: "estimated AC".into(),
                color: SERIES_COLORS[0],
                chart_title: "Power",
                series: &["input_power", "output_power"],
            },
            CardDef {
                title: "Output power",
                value: format!("{:.0} W", v("output_power")),
                sub: "measured DC · by rail".into(),
                color: SERIES_COLORS[1],
                chart_title: "Output power by rail",
                series: &["output_power", "rail_12v_power", "rail_5v_power", "rail_3v3_power"],
            },
            CardDef {
                title: "Efficiency",
                value: format!("{:.1} %", v("efficiency")),
                sub: "estimated".into(),
                color: Color32::WHITE,
                chart_title: "Efficiency",
                series: &["efficiency"],
            },
            CardDef {
                title: "Temperature",
                value: format!("{:.1} °C", v("temp_vrm")),
                sub: format!("case {:.1} °C", v("temp_case")),
                color: Color32::WHITE,
                chart_title: "Temperature",
                series: &["temp_vrm", "temp_case"],
            },
            CardDef {
                title: "Fan",
                value: if rpm < 1.0 { "idle".into() } else { format!("{rpm:.0} RPM") },
                sub: format!("duty {:.0} %", v("fan_duty")),
                color: Color32::WHITE,
                chart_title: "Fan speed",
                series: &["fan_speed"],
            },
            CardDef {
                title: "Energy",
                value: format!("{:.3} kWh", v("energy")),
                sub: "estimated AC input".into(),
                color: Color32::WHITE,
                chart_title: "Energy",
                series: &["energy"],
            },
        ];
        ui.add_space(6.0);
        // Explicit rows (egui cannot wrap framed groups). Cards stretch so
        // every row spans the full width, aligned with the chart below.
        const CARD_MIN: f32 = 138.0;
        const FRAME_EXTRA: f32 = 2.0 * 10.0 + 2.0; // inner margin + stroke
        let gap = ui.spacing().item_spacing.x;
        let avail = ui.available_width();
        let per_row = (((avail + gap) / (CARD_MIN + FRAME_EXTRA + gap)).floor() as usize).clamp(1, cards.len());
        let rows = cards.len().div_ceil(per_row);
        let per_row = cards.len().div_ceil(rows); // balance rows (e.g. 3+3, not 4+2)
        let inner_w = ((avail - gap * (per_row as f32 - 1.0)) / per_row as f32 - FRAME_EXTRA).floor();
        for row in cards.chunks(per_row) {
            ui.horizontal(|ui| {
                for c in row {
                    let selected =
                        chart_series.len() == c.series.len() && chart_series.iter().zip(c.series).all(|(a, b)| a == b);
                    let stroke = if selected {
                        egui::Stroke::new(1.5, ACCENT)
                    } else {
                        ui.style().visuals.widgets.noninteractive.bg_stroke
                    };
                    let resp = egui::Frame::group(ui.style())
                        .stroke(stroke)
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(inner_w);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(c.title).weak());
                                ui.label(RichText::new(&c.value).size(24.0).strong().color(c.color));
                                ui.add(egui::Label::new(RichText::new(&c.sub).small().weak()).truncate());
                            });
                        })
                        .response
                        .interact(egui::Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("Click to chart");
                    if resp.clicked() {
                        self.charts.insert(
                            dev.clone(),
                            (c.chart_title.to_string(), c.series.iter().map(|s| s.to_string()).collect()),
                        );
                    }
                }
            });
        }

        // ---- history chart ----
        ui.add_space(6.0);
        let now = self.h.shared.read().started.elapsed().as_secs_f64();
        section(ui, "", |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&chart_title).strong());
                ui.label(RichText::new("· click any card, rail value or sensor to chart it").small().weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for (label, secs) in RANGES.iter().rev() {
                        if ui.selectable_label(self.range_secs == *secs, *label).clicked() {
                            self.range_secs = *secs;
                        }
                    }
                });
            });
            self.history_chart(ui, d, &chart_series, now);
        });

        // ---- rails + controls ----
        ui.add_space(6.0);
        let mut clicked: Option<String> = None;
        two_columns(
            ui,
            |ui| {
                section(ui, "Output rails", |ui| {
                    egui::Grid::new("rails").num_columns(4).striped(true).spacing([28.0, 6.0]).show(ui, |ui| {
                        ui.label("");
                        ui.label(RichText::new("Voltage").weak());
                        ui.label(RichText::new("Current").weak());
                        ui.label(RichText::new("Power").weak());
                        ui.end_row();
                        let cells: [(&str, [(String, String); 3]); 4] = [
                            ("+12V", rail_cells(&v, "12v")),
                            ("+5V", rail_cells(&v, "5v")),
                            ("+3.3V", rail_cells(&v, "3v3")),
                            (
                                "AC in",
                                [
                                    ("input_voltage".into(), format!("{:.0} V", v("input_voltage"))),
                                    (
                                        "input_current".into(),
                                        d.value("input_current").map(|a| format!("{a:.2} A")).unwrap_or_else(|| "—".into()),
                                    ),
                                    ("input_power".into(), format!("{:.0} W*", v("input_power"))),
                                ],
                            ),
                        ];
                        for (label, row) in cells {
                            ui.label(RichText::new(label).strong());
                            for (id, text) in row {
                                let charted = chart_series.contains(&id);
                                if d.sensor(&id).is_some() {
                                    if chart_link(ui, text, charted, CHART_HINT) {
                                        clicked = Some(id);
                                    }
                                } else {
                                    ui.label(text);
                                }
                            }
                            ui.end_row();
                        }
                    });
                    ui.label(RichText::new("* estimated: HXi/RMi units do not measure AC input power").small().weak());
                });
            },
            |ui| {
                section(ui, "Power supply settings", |ui| {
                    for ch in d.fans.clone() {
                        self.fan_controls(ui, d, &ch);
                        ui.separator();
                    }
                    for c in &d.controls {
                        if let (ControlKind::Choice { options }, ControlValue::Choice(cur)) = (&c.kind, &c.value) {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&c.name).strong());
                                for o in options {
                                    if ui.selectable_label(cur == o, o).clicked() && cur != o {
                                        let _ = self.h.cmd.send(Command::SetControl {
                                            device: d.info.id.clone(),
                                            id: c.id.clone(),
                                            value: ControlValue::Choice(o.clone()),
                                        });
                                    }
                                }
                            });
                            if c.id == opendevicecontrol_core::devices::hxi::CTRL_OCP {
                                ui.label(
                                    RichText::new(
                                        "Multi rail: each +12V output connector trips at its own 40 A limit (safer). \
                                         Single rail: one combined limit for the whole +12V rail.",
                                    )
                                    .small()
                                    .weak(),
                                );
                            }
                        }
                    }
                });
            },
        );

        // ---- every sensor, clickable ----
        ui.add_space(6.0);
        self.device_tree(ui, d, "All sensors");
        if let Some(id) = clicked {
            self.select_series(ui.ctx(), &dev, d, &id);
        }

        ui.add_space(8.0);
        egui::CollapsingHeader::new("Protection limits").default_open(false).show(ui, |ui| {
            egui::Grid::new("limits").striped(true).show(ui, |ui| {
                for l in &d.limits {
                    ui.label(&l.name);
                    ui.label(fmt_value(l.value, l.kind));
                    ui.end_row();
                }
            });
        });
        egui::CollapsingHeader::new("Device information").default_open(false).show(ui, |ui| {
            egui::Grid::new("info").striped(true).show(ui, |ui| {
                let i = &d.info;
                let rows = [
                    ("Model", i.model.clone()),
                    ("Firmware vendor", i.vendor_string.clone().unwrap_or_default()),
                    ("Firmware product", i.product_string.clone().unwrap_or_default()),
                    ("USB ID", format!("{:04x}:{:04x}", i.usb_vid, i.usb_pid)),
                    ("OpenDeviceControl id", i.id.clone()),
                    ("Uptime (since power on)", fmt_duration(v("uptime"))),
                    ("Total lifetime uptime", fmt_duration(v("total_uptime"))),
                ];
                for (k, val) in rows {
                    ui.label(k);
                    ui.label(val);
                    ui.end_row();
                }
            });
        });
    }

    /// Curve inputs offered for a channel: its own default sensor first,
    /// then every temperature of every device and of the computer.
    /// Returns (source, label, current value).
    fn curve_sources(&self, d: &DeviceView, ch: &opendevicecontrol_core::FanChannel) -> Vec<(Option<String>, String, Option<f64>)> {
        let shared = self.h.shared.read();
        let own_name = d.sensor(&ch.curve_sensor).map(|s| s.name.clone());
        let first = match own_name {
            Some(n) => format!("{n} (default)"),
            None if ch.curve_sensor.is_empty() => "No default sensor: choose an input".to_string(),
            None => format!("{} (default)", ch.curve_sensor),
        };
        let mut out = vec![(None, first, d.value(&ch.curve_sensor))];
        for (dev_id, v) in &shared.devices {
            for s in v.sensors.iter().filter(|s| s.kind == SensorKind::Temperature) {
                out.push((Some(format!("{dev_id}/{}", s.id)), format!("{}: {}", v.info.model, s.name), Some(s.value)));
            }
        }
        for s in &shared.system_sensors {
            out.push((
                Some(format!("{}/{}", crate::config::SYSTEM_SOURCE, s.id)),
                format!("Computer: {}", s.name),
                Some(s.value),
            ));
        }
        out
    }

    pub(crate) fn fan_controls(&mut self, ui: &mut egui::Ui, d: &DeviceView, ch: &opendevicecontrol_core::FanChannel) {
        let key = format!("{}/{}", d.info.id, ch.id);
        let policy = self.h.config.read().fan_policy(&d.info.id, &ch.id);
        ui.horizontal(|ui| {
            ui.label(RichText::new(&ch.name).strong());
            for m in FanPolicy::MODES {
                if ui.selectable_label(policy.mode_name() == m, m).clicked() && policy.mode_name() != m {
                    let fixed = *self.fixed_drafts.get(&key).unwrap_or(&50);
                    let p = match m {
                        "Curve" => FanPolicy::Curve {
                            points: self.curve_drafts.get(&key).cloned().unwrap_or_else(FanPolicy::default_curve),
                            source: None,
                        },
                        other => FanPolicy::from_mode(other, fixed).unwrap(),
                    };
                    self.h.set_fan_policy(&d.info.id, &ch.id, p);
                }
            }
        });
        match policy {
            FanPolicy::Hardware => {
                let text = if d.info.kind == opendevicecontrol_core::DeviceKind::PowerSupply {
                    "The PSU's own fan curve, including zero-RPM mode at low load."
                } else {
                    "The device's own default curve. While opendevicecontrol drives a hub's lighting the hub is in software mode and \
                     opendevicecontrol applies a built-in coolant-based default instead; the firmware takes over again when opendevicecontrol exits."
                };
                ui.label(RichText::new(text).small().weak());
            }
            FanPolicy::Fixed { duty } => {
                let draft = self.fixed_drafts.entry(key.clone()).or_insert(duty);
                let r = ui.add(egui::Slider::new(draft, ch.min_duty..=100).suffix(" %").text("duty"));
                if (r.drag_stopped() || (r.changed() && !r.dragged())) && *draft != duty {
                    let duty = *draft;
                    self.h.set_fan_policy(&d.info.id, &ch.id, FanPolicy::Fixed { duty });
                }
            }
            FanPolicy::Curve { points, source } => {
                let sources = self.curve_sources(d, ch);
                let current = sources.iter().find(|s| s.0 == source);
                let temp = current.and_then(|s| s.2);
                let label = current.map(|s| s.1.clone()).unwrap_or_else(|| source.clone().unwrap_or_default());
                let mut chosen = None;
                ui.horizontal(|ui| {
                    ui.label("Input");
                    egui::ComboBox::from_id_salt(("curve-src", &key)).selected_text(label).width(320.0).show_ui(ui, |ui| {
                        for (src, name, val) in &sources {
                            let text = match val {
                                Some(v) => format!("{name}  ({v:.1} °C)"),
                                None => name.clone(),
                            };
                            if ui.selectable_label(*src == source, text).clicked() && *src != source {
                                chosen = Some(src.clone());
                            }
                        }
                    });
                });
                if let Some(src) = chosen {
                    self.h.set_fan_policy(&d.info.id, &ch.id, FanPolicy::Curve { points: points.clone(), source: src });
                }
                let draft = self.curve_drafts.entry(key.clone()).or_insert_with(|| points.clone());
                match temp {
                    Some(t) => ui.label(
                        RichText::new(format!(
                            "Duty follows the input ({t:.1} °C → {} %). Points are interpolated linearly.",
                            eval_curve(draft, t).max(ch.min_duty)
                        ))
                        .small()
                        .weak(),
                    ),
                    None => ui.label(
                        RichText::new("No reading from the input: the fan runs at 100 % until one arrives.").small().color(WARN),
                    ),
                };
                let temp = temp.unwrap_or(0.0);
                let mut remove = None;
                egui::Grid::new(format!("curve-{key}")).show(ui, |ui| {
                    for (i, (t, dty)) in draft.iter_mut().enumerate() {
                        ui.add(egui::DragValue::new(t).range(0.0..=100.0).suffix(" °C").speed(0.5));
                        ui.add(egui::DragValue::new(dty).range(ch.min_duty..=100).suffix(" %"));
                        if ui.small_button("✖").clicked() {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                });
                if let Some(i) = remove {
                    if draft.len() > 2 {
                        draft.remove(i);
                    }
                }
                let preview: PlotPoints = (0..=100).map(|t| [t as f64, eval_curve(draft, t as f64).max(ch.min_duty) as f64]).collect();
                Plot::new(format!("curveplot-{key}"))
                    .height(110.0)
                    .include_y(0.0)
                    .include_y(100.0)
                    .allow_drag(false)
                    .allow_zoom(false)
                    .allow_scroll(false)
                    .show(ui, |p| {
                        p.line(Line::new("curve", preview).color(ACCENT));
                        p.points(Points::new("now", vec![[temp, eval_curve(draft, temp).max(ch.min_duty) as f64]]).radius(4.0).color(WARN));
                    });
                ui.horizontal(|ui| {
                    if ui.button("➕ Add point").clicked() {
                        let last = draft.last().copied().unwrap_or((50.0, 50));
                        draft.push(((last.0 + 5.0).min(100.0), last.1.saturating_add(10).min(100)));
                    }
                    let changed = *draft != points;
                    if ui.add_enabled(changed, egui::Button::new("Apply curve")).clicked() {
                        let mut pts = draft.clone();
                        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
                        self.h.set_fan_policy(&d.info.id, &ch.id, FanPolicy::Curve { points: pts, source: source.clone() });
                    }
                    if ui.add_enabled(changed, egui::Button::new("Revert")).clicked() {
                        *draft = points.clone();
                    }
                });
            }
        }
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        // Keep the draft in sync with changes made elsewhere (e.g. HA) when not editing.
        if !self.draft_dirty {
            self.draft = self.h.config.read().clone();
        }
        let before = self.draft.clone();

        ui.heading("Settings");
        ui.add_space(4.0);
        ui.group(|ui| {
            ui.label(RichText::new("General").strong());
            let mut auto = crate::autostart::is_enabled();
            if ui.checkbox(&mut auto, "Start OpenDeviceControl when I log in").changed() {
                match crate::autostart::set_enabled(auto) {
                    Ok(()) => {
                        self.autostart_error = None;
                        self.h.update_config(|c| c.general.autostart = auto);
                        self.draft.general.autostart = auto;
                    }
                    Err(e) => self.autostart_error = Some(e.to_string()),
                }
            }
            ui.label(RichText::new(format!("Entry: {}", crate::autostart::location())).small().weak());
            if let Some(e) = &self.autostart_error {
                ui.label(RichText::new(e).color(BAD));
            }
            ui.checkbox(&mut self.draft.general.start_minimized, "Start hidden in the tray");
            ui.checkbox(&mut self.draft.general.close_to_tray, "Closing the window keeps OpenDeviceControl running in the tray");
            ui.checkbox(&mut self.draft.general.allow_remote_control, "Allow Home Assistant to change fan and PSU settings");
            ui.horizontal(|ui| {
                ui.label("Poll interval");
                ui.add(egui::DragValue::new(&mut self.draft.general.poll_interval_ms).range(500..=60_000).speed(50).suffix(" ms"));
            });
        });

        ui.add_space(6.0);
        ui.group(|ui| {
            ui.label(RichText::new("Home Assistant (MQTT)").strong());
            ui.label(
                RichText::new(
                    "Uses MQTT discovery: install the Mosquitto broker add-on and the MQTT integration in Home Assistant, \
                     then enter the broker here. Devices appear automatically.",
                )
                .small()
                .weak(),
            );
            ui.checkbox(&mut self.draft.mqtt.enabled, "Enable Home Assistant integration");
            let m = &mut self.draft.mqtt;
            egui::Grid::new("mqtt").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Broker host");
                ui.text_edit_singleline(&mut m.host);
                ui.end_row();
                ui.label("Port");
                ui.add(egui::DragValue::new(&mut m.port).range(1..=65535));
                ui.end_row();
                ui.label("Username");
                ui.text_edit_singleline(&mut m.username);
                ui.end_row();
                ui.label("Password");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut m.password).password(!self.show_password));
                    ui.checkbox(&mut self.show_password, "show");
                });
                ui.end_row();
                ui.label("Node ID");
                ui.text_edit_singleline(&mut m.node_id);
                ui.end_row();
                ui.label("");
                ui.label(
                    RichText::new("Dual-boot: use the SAME node ID on Windows and Linux so both share one HA device and history.")
                        .small()
                        .color(WARN),
                );
                ui.end_row();
                ui.label("Discovery prefix");
                ui.text_edit_singleline(&mut m.discovery_prefix);
                ui.end_row();
                ui.label("Base topic");
                ui.text_edit_singleline(&mut m.base_topic);
                ui.end_row();
                ui.label("Publish every");
                ui.add(egui::DragValue::new(&mut m.publish_interval_s).range(1..=300).suffix(" s"));
                ui.end_row();
            });
            if let MqttStatus::Error(e) = &self.h.shared.read().mqtt {
                ui.label(RichText::new(format!("Last error: {e}")).color(BAD));
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button("🏠 Choose sensors…").clicked() {
                    self.picker_open = true;
                }
                let shared = self.h.shared.read();
                let cfg = self.h.config.read();
                let total: usize = shared.devices.values().map(|d| d.sensors.len()).sum();
                let sent: usize =
                    shared.devices.values().map(|d| d.sensors.iter().filter(|s| cfg.sends(&d.info.id, s)).count()).sum();
                ui.label(RichText::new(format!("{sent} of {total} sensors are sent. Applies immediately.")).weak());
            });
        });

        ui.add_space(6.0);
        self.ram_settings(ui);
        ui.add_space(6.0);
        self.sensor_settings(ui);

        if self.draft != before {
            self.draft_dirty = true;
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(self.draft_dirty, egui::Button::new("💾 Save")).clicked() {
                let mut d = self.draft.clone();
                d.mqtt.node_id = crate::config::sanitize_id(&d.mqtt.node_id);
                self.h.update_config(|c| {
                    c.general = d.general.clone();
                    c.mqtt = d.mqtt.clone();
                });
                self.draft_dirty = false;
            }
            if ui.add_enabled(self.draft_dirty, egui::Button::new("Discard")).clicked() {
                self.draft_dirty = false;
            }
            if ui.button("🔄 Rescan devices").clicked() {
                let _ = self.h.cmd.send(Command::Rescan);
            }
        });

        ui.add_space(6.0);
        ui.group(|ui| {
            ui.label(RichText::new("Files").strong());
            ui.label(format!("Config: {}", Config::path().display()));
            ui.label(format!("Log: {}", crate::config::data_dir().join("opendevicecontrol.log").display()));
            ui.label(format!("OpenDeviceControl {} — GPL-3.0-or-later. Not affiliated with Corsair or any other device maker.", env!("CARGO_PKG_VERSION")));
        });
    }
}

impl FerroApp {
    /// Settings → Memory (RAM) lighting. Applied immediately (not via Save):
    /// enabling probes the SMBus once, disabling stops all SMBus access.
    fn ram_settings(&mut self, ui: &mut egui::Ui) {
        ui.group(|ui| {
            ui.label(RichText::new("Memory (RAM) lighting — experimental").strong());
            ui.label(
                RichText::new(
                    "Corsair RGB memory is controlled over the motherboard's SMBus, which also holds the memory's SPD \
                     chips. OpenDeviceControl only writes to positively identified Corsair controllers and never to SPD \
                     addresses, but this support is untested on real hardware. Enable it at your own risk.",
                )
                .small()
                .color(WARN),
            );
            if cfg!(windows) {
                ui.label(
                    RichText::new(
                        "Windows: needs the PawnIO driver (pawnio.eu) and OpenDeviceControl running as administrator.",
                    )
                    .small()
                    .weak(),
                );
            } else {
                ui.label(
                    RichText::new("Linux: needs the i2c-dev module and OpenDeviceControl's udev rule (see docs/ram.md).")
                        .small()
                        .weak(),
                );
            }
            let mut on = self.h.config.read().ram.enabled;
            if ui.checkbox(&mut on, "Enable Corsair RGB memory support (SMBus)").changed() {
                self.h.update_config(|c| c.ram.enabled = on);
            }
            if let Some(s) = &self.h.shared.read().ram_status {
                ui.label(RichText::new(s).small());
            }
        });
    }

    /// Where This PC's hardware sensors come from.
    fn sensor_settings(&mut self, ui: &mut egui::Ui) {
        if !cfg!(windows) {
            ui.group(|ui| {
                ui.label(RichText::new("Hardware sensors").strong());
                ui.label(
                    RichText::new(
                        "OpenDeviceControl reads every kernel hwmon chip (CPU, motherboard Super I/O, drives, GPUs, memory), batteries, \
                         NVML for NVIDIA GPUs and amdgpu for AMD GPUs. If your motherboard's voltages and fans are missing, \
                         run lm-sensors' sensors-detect and load the module it suggests (often nct6775 or it87).",
                    )
                    .small()
                    .weak(),
                );
                for s in &self.h.shared.read().sensor_status {
                    ui.label(RichText::new(s).small());
                }
            });
            return;
        }
        ui.group(|ui| {
            ui.label(RichText::new("Hardware sensors (Windows)").strong());
            ui.label(
                RichText::new(
                    "Windows only gives CPU and motherboard sensors to programs with a kernel driver. OpenDeviceControl's sensor \
                     service reads them through the signed PawnIO driver and hands them to OpenDeviceControl, which itself keeps \
                     running without administrator rights. The installer sets it up; you can also install or remove it \
                     here (Windows asks for administrator rights).",
                )
                .small()
                .weak(),
            );
            let status = self.h.shared.read().sensor_status.clone();
            let service_line = status.iter().find(|s| s.starts_with("Sensor service:")).cloned();
            let running = service_line.as_deref().is_some_and(|l| !l.ends_with("not installed"));
            ui.horizontal_wrapped(|ui| {
                let label = if running { "Reinstall sensor service…" } else { "Install sensor service…" };
                let mut run = None;
                if ui.button(label).on_hover_text("Runs opendevicecontrol-sensors.exe --install as administrator").clicked() {
                    run = Some("--install");
                }
                if running && ui.button("Remove sensor service…").clicked() {
                    run = Some("--uninstall");
                }
                if let Some(arg) = run {
                    // UAC prompt and install take a while: off the UI thread.
                    *self.sensor_service_msg.lock() = Some("Waiting for administrator approval…".into());
                    let (msg, ctx) = (self.sensor_service_msg.clone(), ui.ctx().clone());
                    std::thread::spawn(move || {
                        let r = crate::sensor_service::run_elevated(arg);
                        opendevicecontrol_core::system::sensor_sources_changed();
                        *msg.lock() = Some(r);
                        ctx.request_repaint();
                    });
                }
            });
            if let Some(m) = self.sensor_service_msg.lock().as_ref() {
                ui.label(RichText::new(m).small().weak());
            }
            for s in &status {
                ui.label(RichText::new(s).small());
            }
            egui::CollapsingHeader::new("Other sensor sources").id_salt("other-sources").show(ui, |ui| {
                ui.label(
                    RichText::new(
                        "OpenDeviceControl also reads every sensor of HWiNFO (Settings → Shared Memory Support) or \
                         LibreHardwareMonitor (Options → Remote Web Server, port below) when one of them runs.",
                    )
                    .small()
                    .weak(),
                );
                let (mut pawnio, mut port) = {
                    let c = self.h.config.read();
                    (c.sensors.pawnio, c.sensors.lhm_port)
                };
                ui.horizontal(|ui| {
                    ui.label("LibreHardwareMonitor web server port");
                    if ui.add(egui::DragValue::new(&mut port).range(1..=65535)).changed() {
                        self.h.update_config(|c| c.sensors.lhm_port = port);
                    }
                });
                if ui.checkbox(&mut pawnio, "Without the service: read the CPU temperature through PawnIO in OpenDeviceControl itself").changed() {
                    self.h.update_config(|c| c.sensors.pawnio = pawnio);
                }
                ui.label(
                    RichText::new("Needs PawnIO, its module next to OpenDeviceControl, and OpenDeviceControl running as administrator.")
                        .small()
                        .weak(),
                );
            });
        });
    }
}
