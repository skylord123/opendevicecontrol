//! Background device service.
//!
//! A manager thread discovers devices and starts one worker thread per
//! device. Each worker owns its device handle and runs everything for it:
//! sensor polling, fan policies, lighting effects (~30 fps) and key input
//! (macros, Home Assistant events). Slow devices therefore never delay
//! others — a 70 ms PSU read does not make keyboard effects stutter.

use crate::config::{eval_curve, split_source, Config, FanPolicy, State, SYSTEM_SOURCE};
use crossbeam_channel::{Receiver, Sender};
use opendevicecontrol_core::keymap::{KeyBinding, KeymapInfo, Macro};
use opendevicecontrol_core::{
    Control, ControlKind, ControlValue, Device, DeviceInfo, FanChannel, FanTarget, Hardware, InputEvent, InputKey, LedShape,
    Limit, LightZone, Rgb, Sensor, SensorKind,
};
use parking_lot::{Mutex, RwLock};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How much history the GUI keeps for every sensor (in memory).
pub const HISTORY_SECS: f64 = 60.0 * 60.0;

/// `[t, value]` samples, `t` in seconds since opendevicecontrol started.
pub type Series = VecDeque<[f64; 2]>;

/// Lighting frame interval while an effect is animating.
const FRAME: Duration = Duration::from_millis(33);
/// Key input polling interval for devices that report keys.
const INPUT_TICK: Duration = Duration::from_millis(10);

#[derive(Debug, Clone)]
pub struct DeviceView {
    pub info: DeviceInfo,
    pub online: bool,
    pub sensors: Vec<Sensor>,
    pub controls: Vec<Control>,
    pub limits: Vec<Limit>,
    pub fans: Vec<FanChannel>,
    pub zones: Vec<LightZone>,
    /// Where the LEDs sit, when the device knows (keyboard drawing).
    pub led_layout: Vec<LedShape>,
    /// Device-side key remapping, if supported.
    pub keymap_info: Option<KeymapInfo>,
    /// Layers read from the device (layer id → keys, zone order).
    pub keymap: BTreeMap<u8, Vec<KeyBinding>>,
    /// Macros read from the device.
    pub macros: Option<Vec<Macro>>,
    /// What the keymap worker is doing or last did ("Reading…", errors).
    pub keymap_status: Option<String>,
    /// The LED colours currently shown (for the GUI preview).
    pub lights: Vec<Rgb>,
    pub keys: Vec<InputKey>,
    /// Most recent key press, for the GUI.
    pub last_key: Option<(String, Instant)>,
    /// History per sensor id. Shared, so cloning a view (the GUI does it
    /// every frame) doesn't copy an hour of samples for hundreds of sensors.
    pub history: Arc<BTreeMap<String, Series>>,
    /// Lowest and highest value per sensor id since OpenDeviceControl started (or the
    /// last reset).
    pub stats: BTreeMap<String, [f64; 2]>,
    /// Hardware nodes of the sensor tree (This PC).
    pub hardware: Vec<Hardware>,
    pub last_error: Option<String>,
    pub last_update: Option<Instant>,
}

impl DeviceView {
    pub fn sensor(&self, id: &str) -> Option<&Sensor> {
        self.sensors.iter().find(|s| s.id == id)
    }
    pub fn value(&self, id: &str) -> Option<f64> {
        self.sensor(id).map(|s| s.value)
    }
    pub fn node(&self, id: &str) -> Option<&Hardware> {
        self.hardware.iter().find(|h| h.id == id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MqttStatus {
    Disabled,
    Connecting,
    Connected,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct Shared {
    pub started: Instant,
    pub devices: BTreeMap<String, DeviceView>,
    pub conflicts: Vec<String>,
    pub mqtt: MqttStatus,
    /// Host sensors (CPU temperature…) usable as fan-curve inputs, as
    /// `system/<id>` sources. Empty where the OS does not expose them.
    pub system_sensors: Vec<Sensor>,
    /// RAM (SMBus) support status for Settings; `None` while disabled.
    pub ram_status: Option<String>,
    /// Extra temperature sources (Windows: PawnIO, HWiNFO,
    /// LibreHardwareMonitor), one line each, for Settings.
    pub sensor_status: Vec<String>,
}

pub enum Command {
    SetControl { device: String, id: String, value: ControlValue },
    /// A key typed in the GUI while it previews the device's built-in effect
    /// (see `Device::preview_key`).
    PreviewKey { device: String, key: String },
    /// Read a keymap layer from the device into `DeviceView::keymap`.
    ReadKeymap { device: String, layer: u8 },
    /// Write a keymap layer (backs up the device's original first).
    WriteKeymap { device: String, layer: u8, keys: Vec<KeyBinding> },
    ReadMacros { device: String },
    WriteMacros { device: String, macros: Vec<Macro> },
    /// Configuration changed (fans, lighting…): re-apply it now.
    ConfigChanged,
    Rescan,
    /// Forget the lowest/highest values of every sensor.
    ResetStats,
    /// Energy counter last published (possibly by the other OS); the local
    /// counter continues from whichever is larger.
    SeedEnergy { device: String, kwh: f64 },
    Shutdown(Sender<()>),
}

/// A key press/release from a device.
#[derive(Debug, Clone)]
pub struct KeyEvent {
    pub device: String,
    pub key: String,
    pub pressed: bool,
}

type Listener = Box<dyn Fn() + Send + Sync>;
type KeyListener = Box<dyn Fn(&KeyEvent) + Send + Sync>;

#[derive(Clone)]
pub struct Handle {
    pub config: Arc<RwLock<Config>>,
    pub shared: Arc<RwLock<Shared>>,
    pub cmd: Sender<Command>,
    listeners: Arc<Mutex<Vec<Listener>>>,
    key_listeners: Arc<Mutex<Vec<KeyListener>>>,
    /// Set when the application is exiting.
    pub quitting: Arc<AtomicBool>,
    /// Set by the MQTT thread once it has said goodbye (or is idle).
    pub mqtt_done: Arc<AtomicBool>,
    /// What the computer is playing, for sound-reactive lighting.
    pub audio: crate::audio::Audio,
}

impl Handle {
    /// Register a callback invoked whenever device state changes.
    pub fn on_update(&self, f: impl Fn() + Send + Sync + 'static) {
        self.listeners.lock().push(Box::new(f));
    }

    pub fn notify(&self) {
        for l in self.listeners.lock().iter() {
            l();
        }
    }

    /// Register a callback invoked for every key press/release.
    pub fn on_key(&self, f: impl Fn(&KeyEvent) + Send + Sync + 'static) {
        self.key_listeners.lock().push(Box::new(f));
    }

    fn notify_key(&self, e: &KeyEvent) {
        for l in self.key_listeners.lock().iter() {
            l(e);
        }
    }

    /// Update configuration, persist it and let the service react.
    pub fn update_config(&self, f: impl FnOnce(&mut Config)) {
        let snapshot = {
            let mut c = self.config.write();
            f(&mut c);
            c.clone()
        };
        if let Err(e) = snapshot.save() {
            log::error!("saving config: {e}");
        }
        let _ = self.cmd.send(Command::ConfigChanged);
        self.notify();
    }

    pub fn set_fan_policy(&self, device: &str, channel: &str, p: FanPolicy) {
        self.update_config(|c| c.set_fan_policy(device, channel, p));
    }

    /// Stop everything: MQTT says goodbye, devices return to hardware
    /// control, energy counters are saved. Blocks up to ~4 s. Idempotent.
    pub fn shutdown(&self) {
        if self.quitting.swap(true, Ordering::SeqCst) {
            return;
        }
        log::info!("shutting down");
        self.notify();
        let deadline = Instant::now() + Duration::from_secs(1);
        while !self.mqtt_done.load(Ordering::Relaxed) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        if self.cmd.send(Command::Shutdown(tx)).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(4));
        }
    }
}

pub fn start(config: Arc<RwLock<Config>>) -> Handle {
    let (tx, rx) = crossbeam_channel::unbounded();
    let shared = Arc::new(RwLock::new(Shared {
        started: Instant::now(),
        devices: BTreeMap::new(),
        conflicts: Vec::new(),
        mqtt: MqttStatus::Disabled,
        system_sensors: Vec::new(),
        ram_status: None,
        sensor_status: Vec::new(),
    }));
    let handle = Handle {
        config,
        shared,
        cmd: tx,
        listeners: Arc::new(Mutex::new(Vec::new())),
        key_listeners: Arc::new(Mutex::new(Vec::new())),
        quitting: Arc::new(AtomicBool::new(false)),
        mqtt_done: Arc::new(AtomicBool::new(true)),
        audio: crate::audio::Audio::default(),
    };
    let h = handle.clone();
    std::thread::Builder::new()
        .name("opendevicecontrol-service".into())
        .spawn(move || manager(h, rx))
        .expect("spawn service");
    handle
}

// ---------------------------------------------------------------- manager

enum WorkerCmd {
    SetControl(String, ControlValue),
    PreviewKey(String),
    Keymap(KeymapCmd),
    ConfigChanged,
    Stop(Sender<()>),
}

struct Worker {
    id: String,
    path: CString,
    tx: Sender<WorkerCmd>,
    thread: std::thread::JoinHandle<()>,
}

fn manager(h: Handle, rx: Receiver<Command>) {
    let state = Arc::new(Mutex::new(State::load()));
    let mut workers: Vec<Worker> = Vec::new();
    let mut next_scan = Instant::now();
    let mut next_conflict = Instant::now();
    let mut next_save = Instant::now() + Duration::from_secs(60);
    let mut sys = sysinfo::System::new();
    let mut components = sysinfo::Components::new_with_refreshed_list();
    let mut next_system = Instant::now();
    // RAM is probed once when enabled and on Rescan, never periodically.
    let mut ram_scanned = false;
    h.config.read().sensors.apply();

    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Command::Shutdown(done)) => {
                let mut acks = Vec::new();
                for w in &workers {
                    let (tx, rx) = crossbeam_channel::bounded(1);
                    if w.tx.send(WorkerCmd::Stop(tx)).is_ok() {
                        acks.push(rx);
                    }
                }
                for a in acks {
                    let _ = a.recv_timeout(Duration::from_secs(3));
                }
                state.lock().save();
                h.notify();
                // Log before signalling: the caller may exit the process
                // as soon as it receives `done`.
                log::info!("service stopped (devices returned to hardware control)");
                log::logger().flush();
                let _ = done.send(());
                return;
            }
            Ok(Command::SetControl { device, id, value }) => {
                if let Some(w) = workers.iter().find(|w| w.id == device) {
                    let _ = w.tx.send(WorkerCmd::SetControl(id, value));
                }
            }
            Ok(Command::PreviewKey { device, key }) => {
                if let Some(w) = workers.iter().find(|w| w.id == device) {
                    let _ = w.tx.send(WorkerCmd::PreviewKey(key));
                }
            }
            Ok(Command::ReadKeymap { device, layer }) => send_keymap(&workers, &device, KeymapCmd::Read(layer)),
            Ok(Command::WriteKeymap { device, layer, keys }) => send_keymap(&workers, &device, KeymapCmd::Write(layer, keys)),
            Ok(Command::ReadMacros { device }) => send_keymap(&workers, &device, KeymapCmd::ReadMacros),
            Ok(Command::WriteMacros { device, macros }) => send_keymap(&workers, &device, KeymapCmd::WriteMacros(macros)),
            Ok(Command::ConfigChanged) => {
                for w in &workers {
                    let _ = w.tx.send(WorkerCmd::ConfigChanged);
                }
                ram_config_changed(&h, &mut workers, &mut ram_scanned, &mut next_scan);
                h.config.read().sensors.apply();
            }
            Ok(Command::SeedEnergy { device, kwh }) => {
                let mut st = state.lock();
                let e = st.energy_kwh.entry(device.clone()).or_insert(0.0);
                if kwh.is_finite() && kwh > *e {
                    log::info!("{device}: resuming energy counter from MQTT ({kwh:.3} kWh > local {:.3})", *e);
                    *e = kwh;
                    st.save();
                }
            }
            Ok(Command::Rescan) => {
                next_scan = Instant::now();
                ram_scanned = false;
            }
            Ok(Command::ResetStats) => {
                for v in h.shared.write().devices.values_mut() {
                    v.stats.clear();
                }
                h.notify();
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
        }

        // Workers exit on their own when their device fails or disappears.
        let before = workers.len();
        workers.retain(|w| !w.thread.is_finished());
        if workers.len() != before {
            next_scan = Instant::now() + Duration::from_secs(2);
        }

        if Instant::now() >= next_scan {
            let held: Vec<(CString, String)> = workers.iter().map(|w| (w.path.clone(), w.id.clone())).collect();
            match opendevicecontrol_core::discover(&held) {
                Ok(found) => {
                    for (path, dev) in found {
                        let id = dev.info().id.clone();
                        let (tx, wrx) = crossbeam_channel::unbounded();
                        let (h2, st2) = (h.clone(), state.clone());
                        let thread = std::thread::Builder::new()
                            .name(format!("opendevicecontrol-{id}"))
                            .spawn(move || worker(h2, st2, dev, wrx))
                            .expect("spawn worker");
                        workers.push(Worker { id, path, tx, thread });
                    }
                }
                Err(e) => log::warn!("device scan failed: {e}"),
            }
            if h.config.read().ram.enabled && !ram_scanned {
                ram_scanned = true;
                ram_scan(&h, &state, &mut workers);
            }
            next_scan = Instant::now() + Duration::from_secs(if workers.is_empty() { 5 } else { 30 });
        }

        if Instant::now() >= next_conflict {
            let c = crate::conflicts::detect(&mut sys);
            h.shared.write().conflicts = c;
            next_conflict = Instant::now() + Duration::from_secs(15);
        }

        if Instant::now() >= next_system {
            components.refresh(true);
            let sensors = system_sensors(&components);
            let status = opendevicecontrol_core::system::sensor_sources();
            {
                let mut sh = h.shared.write();
                sh.system_sensors = sensors;
                sh.sensor_status = status;
            }
            next_system = Instant::now() + Duration::from_secs(5);
        }

        if Instant::now() >= next_save {
            state.lock().save();
            next_save = Instant::now() + Duration::from_secs(60);
        }
    }
}

// ----------------------------------------------------------------- worker

struct WorkerState {
    applied_fans: HashMap<String, (FanTarget, Instant)>,
    last_sample: Option<Instant>,
    failures: u32,
}

/// Keymap work for a device worker.
enum KeymapCmd {
    Read(u8),
    Write(u8, Vec<KeyBinding>),
    ReadMacros,
    WriteMacros(Vec<Macro>),
}

fn send_keymap(workers: &[Worker], device: &str, cmd: KeymapCmd) {
    if let Some(w) = workers.iter().find(|w| w.id == device) {
        let _ = w.tx.send(WorkerCmd::Keymap(cmd));
    }
}

/// Where the device's original keymap is saved before OpenDeviceControl first
/// changes it (`<data dir>/keymap-backup/<device>-<what>.json`).
pub fn keymap_backup_path(device: &str, what: &str) -> std::path::PathBuf {
    crate::config::data_dir().join("keymap-backup").join(format!("{device}-{what}.json"))
}

/// Save `value` as the backup `what` of `device`, unless one exists.
fn backup_once<T: serde::Serialize>(device: &str, what: &str, value: &T) -> std::io::Result<()> {
    write_backup_once(&keymap_backup_path(device, what), value)
}

/// Write `value` to `path` unless a readable backup is already there. The
/// file is written to a temporary name and renamed into place, so an
/// interrupted save never leaves a partial file that later counts as the
/// original; a file that isn't valid JSON (from an older, interrupted
/// save) is replaced.
fn write_backup_once<T: serde::Serialize>(path: &std::path::Path, value: &T) -> std::io::Result<()> {
    if let Ok(bytes) = std::fs::read(path) {
        if serde_json::from_slice::<serde_json::Value>(&bytes).is_ok() {
            return Ok(());
        }
        log::warn!("{}: unreadable keymap backup, replacing it", path.display());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = std::path::PathBuf::from(tmp);
    let res = (|| {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

fn keymap_command(h: &Handle, id: &str, dev: &mut dyn Device, cmd: KeymapCmd) {
    let status = |h: &Handle, s: Option<String>| {
        if let Some(v) = h.shared.write().devices.get_mut(id) {
            v.keymap_status = s;
        }
        h.notify();
    };
    let info = dev.keymap_info();
    let layer_name = |l: u8| {
        info.as_ref().and_then(|i| i.layers.iter().find(|x| x.id == l).map(|x| x.name.clone())).unwrap_or_else(|| format!("layer {l}"))
    };
    match cmd {
        KeymapCmd::Read(layer) => {
            status(h, Some(format!("Reading the {} layer…", layer_name(layer))));
            match dev.read_keymap(layer) {
                Ok(keys) => {
                    if let Some(v) = h.shared.write().devices.get_mut(id) {
                        v.keymap.insert(layer, keys);
                    }
                    status(h, None);
                }
                Err(e) => status(h, Some(format!("Couldn't read the {} layer: {e}", layer_name(layer)))),
            }
        }
        KeymapCmd::Write(layer, keys) => {
            status(h, Some(format!("Saving the {} layer to the keyboard…", layer_name(layer))));
            // Keep the device's own keymap before the first change.
            let original = match h.shared.read().devices.get(id).and_then(|v| v.keymap.get(&layer).cloned()) {
                Some(k) => Ok(k),
                None => dev.read_keymap(layer),
            };
            let backed_up = original.map_err(|e| e.to_string()).and_then(|k| backup_once(id, &format!("layer{layer}"), &k).map_err(|e| e.to_string()));
            if let Err(e) = backed_up {
                status(h, Some(format!("Not saved: couldn't back up the current keys first ({e})")));
                return;
            }
            match dev.write_keymap(layer, &keys).and_then(|()| dev.read_keymap(layer)) {
                Ok(back) => {
                    let ok = back == keys;
                    if let Some(v) = h.shared.write().devices.get_mut(id) {
                        v.keymap.insert(layer, back);
                    }
                    status(h, Some(if ok { "Saved to the keyboard.".into() } else { "Saved, but the keyboard reads back differently.".into() }));
                    log::info!("{id}: keymap layer {layer} written");
                }
                Err(e) => status(h, Some(format!("Saving failed: {e}"))),
            }
        }
        KeymapCmd::ReadMacros => {
            status(h, Some("Reading macros…".into()));
            match dev.read_macros() {
                Ok(m) => {
                    if let Some(v) = h.shared.write().devices.get_mut(id) {
                        v.macros = Some(m);
                    }
                    status(h, None);
                }
                Err(e) => status(h, Some(format!("Couldn't read macros: {e}"))),
            }
        }
        KeymapCmd::WriteMacros(macros) => {
            status(h, Some("Saving macros to the keyboard…".into()));
            let original = match h.shared.read().devices.get(id).and_then(|v| v.macros.clone()) {
                Some(m) => Ok(m),
                None => dev.read_macros(),
            };
            if let Err(e) = original.map_err(|e| e.to_string()).and_then(|m| backup_once(id, "macros", &m).map_err(|e| e.to_string())) {
                status(h, Some(format!("Not saved: couldn't back up the current macros first ({e})")));
                return;
            }
            match dev.write_macros(&macros).and_then(|()| dev.read_macros()) {
                Ok(back) => {
                    if let Some(v) = h.shared.write().devices.get_mut(id) {
                        v.macros = Some(back);
                    }
                    status(h, Some("Macros saved to the keyboard.".into()));
                }
                Err(e) => {
                    // A failure part-way can leave some pages written; show
                    // what the keyboard holds now.
                    if let Ok(now) = dev.read_macros() {
                        if let Some(v) = h.shared.write().devices.get_mut(id) {
                            v.macros = Some(now);
                        }
                    }
                    let backup = keymap_backup_path(id, "macros");
                    status(h, Some(format!("Saving macros failed: {e}. The original macros are backed up in {}.", backup.display())));
                }
            }
        }
    }
}

fn worker(h: Handle, state: Arc<Mutex<State>>, mut dev: Box<dyn Device>, rx: Receiver<WorkerCmd>) {
    let id = dev.info().id.clone();
    // Re-apply remembered settings (pump mode, LED counts…) first: they can
    // change the device's lighting zones.
    {
        let cfg = h.config.read().clone();
        for cid in dev.remembered_controls() {
            if let Some(v) = cfg.settings.get(&format!("{id}/{cid}")) {
                if let Err(e) = dev.set_control(&cid, v) {
                    log::warn!("{id}: restoring {cid}: {e}");
                }
            }
        }
    }
    let mut zones = dev.light_zones();
    let mut layout = dev.led_layout();
    let keys = dev.input_keys();
    h.shared.write().devices.insert(
        id.clone(),
        DeviceView {
            info: dev.info().clone(),
            online: true,
            sensors: Vec::new(),
            controls: Vec::new(),
            limits: dev.limits().to_vec(),
            fans: dev.fan_channels(),
            zones: zones.clone(),
            led_layout: layout.clone(),
            keymap_info: dev.keymap_info(),
            keymap: BTreeMap::new(),
            macros: None,
            keymap_status: None,
            lights: Vec::new(),
            keys: keys.clone(),
            last_key: None,
            history: Arc::new(BTreeMap::new()),
            stats: BTreeMap::new(),
            hardware: Vec::new(),
            last_error: None,
            last_update: None,
        },
    );

    // Re-apply remembered toggles (Win lock etc.).
    {
        let cfg = h.config.read().clone();
        if let Ok(snap) = dev.poll() {
            for c in snap.controls.iter().filter(|c| c.kind == ControlKind::Toggle) {
                if let Some(&want) = cfg.toggles.get(&format!("{id}/{}", c.id)) {
                    if c.value != ControlValue::Bool(want) {
                        if let Err(e) = dev.set_control(&c.id, &ControlValue::Bool(want)) {
                            log::warn!("{id}: restoring {}: {e}", c.id);
                        }
                    }
                }
            }
        }
    }

    let mut ws = WorkerState { applied_fans: HashMap::new(), last_sample: None, failures: 0 };
    let started = h.shared.read().started;
    let mut next_poll = Instant::now();
    let mut next_frame = Instant::now();
    let mut lighting = h.config.read().lighting(&id);

    loop {
        // Sleep until the next thing is due, waking early for commands.
        let mut due = next_poll;
        if !zones.is_empty() {
            due = due.min(next_frame);
        }
        if !keys.is_empty() {
            due = due.min(Instant::now() + INPUT_TICK);
        }
        match rx.recv_timeout(due.saturating_duration_since(Instant::now())) {
            Ok(WorkerCmd::Stop(done)) => {
                dev.release();
                if let Some(v) = h.shared.write().devices.get_mut(&id) {
                    v.online = false;
                }
                let _ = done.send(());
                return;
            }
            Ok(WorkerCmd::SetControl(cid, value)) => {
                match dev.set_control(&cid, &value) {
                    Ok(()) => log::info!("{id}: {cid} -> {value:?}"),
                    Err(e) => log::warn!("{id}: setting {cid} failed: {e}"),
                }
                // Some settings change the zones or fans (e.g. LED counts).
                let (z, fans) = (dev.light_zones(), dev.fan_channels());
                let same = z.len() == zones.len() && z.iter().zip(&zones).all(|(a, b)| a.id == b.id && a.leds == b.leds);
                if !same {
                    zones = z;
                    next_frame = Instant::now();
                }
                if let Some(v) = h.shared.write().devices.get_mut(&id) {
                    v.zones = zones.clone();
                    layout = dev.led_layout();
                    v.led_layout = layout.clone();
                    v.fans = fans;
                }
                next_poll = Instant::now();
                continue;
            }
            Ok(WorkerCmd::Keymap(cmd)) => {
                keymap_command(&h, &id, &mut *dev, cmd);
                next_frame = Instant::now();
                continue;
            }
            Ok(WorkerCmd::PreviewKey(key)) => {
                dev.preview_key(&key);
                continue;
            }
            Ok(WorkerCmd::ConfigChanged) => {
                ws.applied_fans.clear();
                lighting = h.config.read().lighting(&id);
                next_frame = Instant::now();
                next_poll = next_poll.min(Instant::now() + Duration::from_millis(50));
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                dev.release();
                return;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();

        // ---- key input ----
        if !keys.is_empty() {
            for ev in dev.poll_input() {
                let (key, pressed) = match ev {
                    InputEvent::Pressed(k) => (k, true),
                    InputEvent::Released(k) => (k, false),
                };
                on_key(&h, &id, &key, pressed);
                if pressed && key == opendevicecontrol_core::devices::nxp::KEY_BRIGHTNESS && !zones.is_empty() {
                    let b = crate::lighting::next_brightness(lighting.brightness);
                    h.update_config(|c| {
                        let l = c.lighting.entry(id.clone()).or_default();
                        l.brightness = b;
                        l.on = true;
                    });
                }
                if key == opendevicecontrol_core::devices::nxp::KEY_WINLOCK {
                    next_poll = Instant::now(); // show the new lock state
                }
            }
        }

        // ---- lighting ----
        if !zones.is_empty() && now >= next_frame {
            let t = now.duration_since(started).as_secs_f64();
            // A device running its own lighting only gets previewed.
            let preview = dev.preview_lights(t);
            let own = preview.is_some();
            let frame = preview.unwrap_or_else(|| {
                let audio = (lighting.on && lighting.effect.is_sound()).then(|| h.audio.frame());
                crate::lighting::render(&lighting, &zones, &layout, t, audio.as_ref())
            });
            match if own { Ok(()) } else { dev.set_lights(&frame) } {
                Ok(()) => {
                    let changed = {
                        let mut s = h.shared.write();
                        match s.devices.get_mut(&id) {
                            Some(v) if v.lights != frame => {
                                v.lights = frame;
                                true
                            }
                            _ => false,
                        }
                    };
                    if changed {
                        h.notify();
                    }
                }
                Err(e) => {
                    if fail(&h, &id, &mut ws, &e.to_string()) {
                        dev.release();
                        return;
                    }
                }
            }
            next_frame = now + if own || crate::lighting::is_animated(&lighting) { FRAME } else { Duration::from_millis(500) };
        }

        // ---- sensors / controls ----
        if now >= next_poll {
            let interval = dev
                .poll_interval()
                .unwrap_or_else(|| Duration::from_millis(h.config.read().general.poll_interval_ms.clamp(500, 60_000)));
            next_poll = now + interval;
            match dev.poll() {
                Ok(mut snap) => {
                    ws.failures = 0;
                    let cfg = h.config.read().clone();
                    // Energy integration of the (estimated) AC input power.
                    let pin = snap.sensor("input_power").or_else(|| snap.sensor("output_power"));
                    if let Some(p) = pin {
                        let mut st = state.lock();
                        let e = st.energy_kwh.entry(id.clone()).or_insert(0.0);
                        if let Some(last) = ws.last_sample {
                            let dt = now.duration_since(last);
                            if dt < interval * 3 {
                                *e += p * dt.as_secs_f64() / 3_600_000.0;
                            }
                        }
                        snap.sensors.push(Sensor::new("energy", "Energy", SensorKind::Energy, *e).estimated());
                    }
                    ws.last_sample = Some(now);

                    apply_fans(&h, &mut *dev, &mut ws, &cfg, &snap.sensors);
                    remember_toggles(&h, &cfg, &id, &snap.controls);
                    remember_settings(&h, &cfg, &id, &dev.remembered_controls(), &snap.controls);

                    // A wireless device whose receiver answers but which is
                    // itself switched off is offline, not failing.
                    let connected = dev.connected();
                    let hardware = dev.hardware();
                    let keymap_info = dev.keymap_info();
                    let mut s = h.shared.write();
                    if let Some(v) = s.devices.get_mut(&id) {
                        v.online = connected;
                        v.keymap_info = keymap_info;
                        // Devices that change links (Aula) change their USB ids.
                        v.info = dev.info().clone();
                        v.last_error = None;
                        v.last_update = Some(now);
                        let t = now.duration_since(started).as_secs_f64();
                        let history = Arc::make_mut(&mut v.history);
                        for s in &snap.sensors {
                            let series = history.entry(s.id.clone()).or_default();
                            series.push_back([t, s.value]);
                            while series.front().is_some_and(|p| t - p[0] > HISTORY_SECS) {
                                series.pop_front();
                            }
                            let st = v.stats.entry(s.id.clone()).or_insert([s.value, s.value]);
                            st[0] = st[0].min(s.value);
                            st[1] = st[1].max(s.value);
                        }
                        // Sensors that went away (a tool was closed) keep
                        // their history for the chart until it ages out.
                        history.retain(|_, s| s.back().is_some_and(|p| t - p[0] <= HISTORY_SECS));
                        v.hardware = hardware;
                        v.sensors = snap.sensors;
                        v.controls = snap.controls;
                    }
                    drop(s);
                    h.notify();
                }
                Err(e) => {
                    if fail(&h, &id, &mut ws, &e.to_string()) {
                        dev.release();
                        return;
                    }
                }
            }
        }
    }
}

/// Record a failure; returns true when the worker should give up (the
/// manager then rescans, so an unplugged or reset device comes back).
fn fail(h: &Handle, id: &str, ws: &mut WorkerState, err: &str) -> bool {
    ws.failures += 1;
    log::warn!("{id}: {err} ({}x)", ws.failures);
    let give_up = ws.failures >= 5;
    if let Some(v) = h.shared.write().devices.get_mut(id) {
        v.last_error = Some(err.to_string());
        if give_up {
            v.online = false;
        }
    }
    h.notify();
    if give_up {
        log::warn!("{id}: giving up after repeated failures; will rescan");
    }
    give_up
}

fn on_key(h: &Handle, device: &str, key: &str, pressed: bool) {
    if pressed {
        log::debug!("{device}: {key} pressed");
        if let Some(v) = h.shared.write().devices.get_mut(device) {
            v.last_key = Some((key.to_string(), Instant::now()));
        }
        let action = h.config.read().key_action(device, key);
        crate::inject::run(action);
        h.notify();
    }
    h.notify_key(&KeyEvent { device: device.into(), key: key.into(), pressed });
}

/// Persist toggle controls whose value changed (by us, HA or the device
/// itself, e.g. the keyboard's Win-lock key) so they survive restarts.
fn remember_toggles(h: &Handle, cfg: &Config, id: &str, controls: &[Control]) {
    let changed: Vec<(String, bool)> = controls
        .iter()
        .filter_map(|c| match (&c.kind, &c.value) {
            (ControlKind::Toggle, ControlValue::Bool(b)) => {
                let key = format!("{id}/{}", c.id);
                (cfg.toggles.get(&key) != Some(b)).then_some((key, *b))
            }
            _ => None,
        })
        .collect();
    if !changed.is_empty() {
        let mut c = h.config.write();
        for (k, v) in changed {
            c.toggles.insert(k, v);
        }
        if let Err(e) = c.save() {
            log::warn!("saving config: {e}");
        }
    }
}

/// Persist values of the controls a driver asks to be remembered.
fn remember_settings(h: &Handle, cfg: &Config, id: &str, remembered: &[String], controls: &[Control]) {
    let changed: Vec<(String, ControlValue)> = controls
        .iter()
        .filter(|c| remembered.contains(&c.id))
        .filter_map(|c| {
            let key = format!("{id}/{}", c.id);
            (cfg.settings.get(&key) != Some(&c.value)).then(|| (key, c.value.clone()))
        })
        .collect();
    if !changed.is_empty() {
        let mut c = h.config.write();
        for (k, v) in changed {
            c.settings.insert(k, v);
        }
        if let Err(e) = c.save() {
            log::warn!("saving config: {e}");
        }
    }
}

/// Host temperatures (the OS, plus HWiNFO / LibreHardwareMonitor / PawnIO
/// on Windows), plus `cpu_temp`: the hottest CPU sensor.
fn system_sensors(components: &sysinfo::Components) -> Vec<Sensor> {
    opendevicecontrol_core::system::host_temperatures(components)
}

/// Value of a fan curve's input. `None` (missing, offline or stale source)
/// makes the caller fail safe to full speed.
fn curve_input(h: &Handle, own: &str, ch: &FanChannel, source: Option<&str>, local: &[Sensor]) -> Option<f64> {
    let find = |list: &[Sensor], sid: &str| list.iter().find(|s| s.id == sid).map(|s| s.value);
    let Some(src) = source else { return find(local, &ch.curve_sensor) };
    let (dev, sid) = split_source(src)?;
    if dev == own {
        return find(local, sid);
    }
    let shared = h.shared.read();
    if dev == SYSTEM_SOURCE {
        return find(&shared.system_sensors, sid);
    }
    let v = shared.devices.get(dev)?;
    if !v.online || v.last_update.is_none_or(|t| t.elapsed() > Duration::from_secs(30)) {
        return None;
    }
    find(&v.sensors, sid)
}

fn apply_fans(h: &Handle, dev: &mut dyn Device, ws: &mut WorkerState, cfg: &Config, sensors: &[Sensor]) {
    let id = dev.info().id.clone();
    for ch in dev.fan_channels() {
        let target = match cfg.fan_policy(&id, &ch.id) {
            FanPolicy::Hardware => FanTarget::Hardware,
            FanPolicy::Fixed { duty } => FanTarget::Duty(duty.clamp(ch.min_duty, 100)),
            FanPolicy::Curve { points, source } => {
                let temp = curve_input(h, &id, &ch, source.as_deref(), sensors);
                match temp {
                    // Fail safe: full speed if the curve input is missing.
                    None => FanTarget::Duty(100),
                    Some(t) => FanTarget::Duty(eval_curve(&points, t).clamp(ch.min_duty, 100)),
                }
            }
        };
        let needs = match ws.applied_fans.get(&ch.id) {
            None => true,
            Some((prev, at)) => {
                let changed = match (prev, &target) {
                    (FanTarget::Duty(a), FanTarget::Duty(b)) => a.abs_diff(*b) >= 2,
                    (a, b) => a != b,
                };
                // Periodically re-assert, in case another program changed it.
                changed || at.elapsed() > Duration::from_secs(30)
            }
        };
        if needs {
            match dev.set_fan(&ch.id, target) {
                Ok(()) => {
                    ws.applied_fans.insert(ch.id.clone(), (target, Instant::now()));
                }
                Err(e) => log::warn!("{id}: set fan {}: {e}", ch.id),
            }
        }
    }
}

// -------------------------------------------------------------------- RAM

/// Start a worker for a device found by a scan.
fn spawn_worker(h: &Handle, state: &Arc<Mutex<State>>, path: CString, dev: Box<dyn Device>) -> Worker {
    let id = dev.info().id.clone();
    let (tx, wrx) = crossbeam_channel::unbounded();
    let (h2, st2) = (h.clone(), state.clone());
    let thread = std::thread::Builder::new()
        .name(format!("opendevicecontrol-{id}"))
        .spawn(move || worker(h2, st2, dev, wrx))
        .expect("spawn worker");
    Worker { id, path, tx, thread }
}

fn is_ram_worker(w: &Worker) -> bool {
    w.path.as_bytes().starts_with(b"smbus:")
}

/// Probe the SMBus for Corsair RGB memory (only when enabled in Settings).
fn ram_scan(h: &Handle, state: &Arc<Mutex<State>>, workers: &mut Vec<Worker>) {
    let held: Vec<CString> = workers.iter().filter(|w| is_ram_worker(w)).map(|w| w.path.clone()).collect();
    let status = match opendevicecontrol_core::devices::ram::discover(&held) {
        Ok(scan) => {
            let found: Vec<String> = scan.devices.iter().map(|(_, d)| d.info().model.clone()).collect();
            for (path, dev) in scan.devices {
                workers.push(spawn_worker(h, state, path, dev));
            }
            let held_now = workers.iter().filter(|w| is_ram_worker(w)).count();
            if !found.is_empty() {
                format!("Found Corsair {} (SMBus: {})", found.join(", "), scan.buses.join(", "))
            } else if held_now > 0 {
                format!("In use (SMBus: {})", scan.buses.join(", "))
            } else {
                format!("No Corsair RGB memory found (SMBus: {})", scan.buses.join(", "))
            }
        }
        Err(e) => {
            log::warn!("RAM: {e}");
            e.to_string()
        }
    };
    h.shared.write().ram_status = Some(status);
    h.notify();
}

/// RAM support was switched on or off.
fn ram_config_changed(h: &Handle, workers: &mut Vec<Worker>, ram_scanned: &mut bool, next_scan: &mut Instant) {
    if h.config.read().ram.enabled {
        if !*ram_scanned {
            *next_scan = Instant::now();
        }
        return;
    }
    *ram_scanned = false;
    let mut acks = Vec::new();
    let mut ids = Vec::new();
    workers.retain(|w| {
        if !is_ram_worker(w) {
            return true;
        }
        ids.push(w.id.clone());
        let (tx, rx) = crossbeam_channel::bounded(1);
        if w.tx.send(WorkerCmd::Stop(tx)).is_ok() {
            acks.push(rx);
        }
        false
    });
    for a in acks {
        let _ = a.recv_timeout(Duration::from_secs(3));
    }
    let changed = {
        let mut sh = h.shared.write();
        for id in &ids {
            sh.devices.remove(id);
        }
        sh.ram_status.take().is_some() || !ids.is_empty()
    };
    if changed {
        h.notify();
    }
}

#[cfg(test)]
mod backup_tests {
    use super::write_backup_once;

    #[test]
    fn backup_is_kept_once_and_replaced_when_corrupt() {
        let dir = std::env::temp_dir().join(format!("odc-backup-test-{}", std::process::id()));
        let path = dir.join("kb-macros.json");
        let _ = std::fs::remove_dir_all(&dir);

        write_backup_once(&path, &vec![1, 2, 3]).unwrap();
        // A second save keeps the original.
        write_backup_once(&path, &vec![9]).unwrap();
        let v: Vec<i32> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(v, vec![1, 2, 3]);

        // A truncated file from an interrupted older save is not a backup.
        std::fs::write(&path, b"[1, 2").unwrap();
        write_backup_once(&path, &vec![4]).unwrap();
        let v: Vec<i32> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(v, vec![4]);

        // No temporary files left behind.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
