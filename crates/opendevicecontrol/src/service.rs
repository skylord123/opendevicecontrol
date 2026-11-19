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
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// How much history the GUI keeps for every sensor (in memory).
pub const HISTORY_SECS: f64 = 60.0 * 60.0;

/// `[t, value]` samples, `t` in seconds since opendevicecontrol started.
pub type Series = VecDeque<[f64; 2]>;

/// Lighting frame interval while an effect is animating.
const FRAME: Duration = Duration::from_millis(33);
/// Key input polling interval for devices that report keys.
const INPUT_TICK: Duration = Duration::from_millis(10);
/// Consecutive failed polls before a worker gives up and rescans.
const POLL_FAIL_LIMIT: u32 = 5;
/// Consecutive failed lighting frames before a worker gives up. Frames come
/// up to 30 times a second, so a short burst of errors is tolerated.
const LIGHT_FAIL_LIMIT: u32 = 30;
/// After a failed poll, retry this soon even when the poll interval is long.
const POLL_RETRY: Duration = Duration::from_secs(2);
/// Without a good sample for this long, fan curves fed by the device's own
/// sensors go to full speed.
const COOLING_FAILSAFE_AFTER: Duration = Duration::from_secs(10);
/// System temperatures older than this are not used as curve input.
const SYSTEM_SOURCE_MAX_AGE: Duration = Duration::from_secs(15);
/// How long the manager waits for all workers to release their devices.
const WORKER_STOP_TIMEOUT: Duration = Duration::from_secs(3);
/// How long [`Handle::shutdown`] waits for the manager to finish.
const SERVICE_STOP_TIMEOUT: Duration = Duration::from_secs(4);

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
    /// Keys held down right now (Home Assistant's "pressed" sensors follow
    /// this, not the event stream, so a lost event can't leave one stuck).
    pub held_keys: BTreeSet<String>,
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
    /// When `system_sensors` was last refreshed.
    pub system_updated: Option<Instant>,
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
    /// Release every device; answers whether all of them were handed back.
    Shutdown(Sender<Result<(), String>>),
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
    /// Outcome of the one shutdown every caller waits for.
    shutdown_result: Arc<OnceLock<Result<(), String>>>,
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
        f(&mut self.config.write());
        if let Err(e) = crate::config::save_shared(&self.config) {
            log::error!("saving config: {e}");
        }
        let _ = self.cmd.send(Command::ConfigChanged);
        self.notify();
    }

    pub fn set_fan_policy(&self, device: &str, channel: &str, p: FanPolicy) {
        self.update_config(|c| c.set_fan_policy(device, channel, p));
    }

    /// Stop everything: MQTT says goodbye, devices return to hardware
    /// control, energy counters are saved. Blocks up to ~5 s. Every caller,
    /// including concurrent ones, waits for the same single run and gets its
    /// outcome: `Err` when a device could not be handed back to hardware
    /// control or the service did not stop in time.
    pub fn shutdown(&self) -> Result<(), String> {
        let r = self.shutdown_result.get_or_init(|| self.run_shutdown()).clone();
        // Tell a waiting `opendevicecontrol --quit` before anyone exits.
        crate::ipc::report_shutdown(&r);
        r
    }

    fn run_shutdown(&self) -> Result<(), String> {
        self.quitting.store(true, Ordering::SeqCst);
        log::info!("shutting down");
        self.notify();
        let deadline = Instant::now() + Duration::from_secs(1);
        while !self.mqtt_done.load(Ordering::Relaxed) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let r = match self.cmd.send(Command::Shutdown(tx)) {
            Ok(()) => rx.recv_timeout(SERVICE_STOP_TIMEOUT).unwrap_or_else(|_| Err("the device service did not stop in time".into())),
            Err(_) => Err("the device service is not running".into()),
        };
        if let Err(e) = &r {
            log::error!("shutdown incomplete: {e}");
            log::logger().flush();
        }
        r
    }
}

/// A handle with no service behind it, for tests: commands land in the
/// returned receiver.
#[cfg(test)]
pub fn test_handle(config: Config) -> (Handle, Receiver<Command>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let shared = Arc::new(RwLock::new(Shared {
        started: Instant::now(),
        devices: BTreeMap::new(),
        conflicts: Vec::new(),
        mqtt: MqttStatus::Disabled,
        system_sensors: Vec::new(),
        system_updated: None,
        ram_status: None,
        sensor_status: Vec::new(),
    }));
    let h = Handle {
        config: Arc::new(RwLock::new(config)),
        shared,
        cmd: tx,
        listeners: Arc::new(Mutex::new(Vec::new())),
        key_listeners: Arc::new(Mutex::new(Vec::new())),
        quitting: Arc::new(AtomicBool::new(false)),
        mqtt_done: Arc::new(AtomicBool::new(true)),
        shutdown_result: Arc::new(OnceLock::new()),
        audio: crate::audio::Audio::default(),
    };
    (h, rx)
}

/// An empty view of a device, for tests.
#[cfg(test)]
pub fn test_view(info: DeviceInfo) -> DeviceView {
    DeviceView {
        info,
        online: true,
        sensors: Vec::new(),
        controls: Vec::new(),
        limits: Vec::new(),
        fans: Vec::new(),
        zones: Vec::new(),
        led_layout: Vec::new(),
        keymap_info: None,
        keymap: BTreeMap::new(),
        macros: None,
        keymap_status: None,
        lights: Vec::new(),
        keys: Vec::new(),
        last_key: None,
        held_keys: BTreeSet::new(),
        history: Arc::new(BTreeMap::new()),
        stats: BTreeMap::new(),
        hardware: Vec::new(),
        last_error: None,
        last_update: Some(Instant::now()),
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
        system_updated: None,
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
        shutdown_result: Arc::new(OnceLock::new()),
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
    /// Release the device and exit; answers with the release outcome.
    Stop(Sender<Result<(), String>>),
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
                let result = stop_workers(workers.iter().map(|w| (w.id.as_str(), &w.tx)), WORKER_STOP_TIMEOUT);
                state.lock().save();
                h.notify();
                // Log before signalling: the caller may exit the process
                // as soon as it receives `done`.
                match &result {
                    Ok(()) => log::info!("service stopped (devices returned to hardware control)"),
                    Err(e) => log::error!("service stopped, but not every device was handed back: {e}"),
                }
                log::logger().flush();
                let _ = done.send(result);
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
                sh.system_updated = Some(Instant::now());
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

/// Ask workers to release their devices and wait (in parallel, up to
/// `timeout` in total) for every answer. `Err` lists the devices that failed
/// to release or did not answer in time.
fn stop_workers<'a>(workers: impl Iterator<Item = (&'a str, &'a Sender<WorkerCmd>)>, timeout: Duration) -> Result<(), String> {
    let mut acks = Vec::new();
    for (id, tx) in workers {
        let (atx, arx) = crossbeam_channel::bounded(1);
        // A worker that already exited released its device itself.
        if tx.send(WorkerCmd::Stop(atx)).is_ok() {
            acks.push((id, arx));
        }
    }
    let deadline = Instant::now() + timeout;
    let mut failed = Vec::new();
    for (id, rx) in acks {
        match rx.recv_deadline(deadline) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => failed.push(format!("{id}: {e}")),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => failed.push(format!("{id}: did not stop in time")),
            // Dropped the request without answering (panicked).
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => failed.push(format!("{id}: stopped without releasing")),
        }
    }
    if failed.is_empty() { Ok(()) } else { Err(failed.join("; ")) }
}

/// Hand a device back to firmware control, logging a failure.
fn release(id: &str, dev: &mut dyn Device) -> Result<(), String> {
    dev.release().map_err(|e| {
        log::error!("{id}: could not return to hardware control: {e}");
        e.to_string()
    })
}

// ----------------------------------------------------------------- worker

struct WorkerState {
    applied_fans: HashMap<String, (FanTarget, Instant)>,
    last_sample: Option<Instant>,
    /// Consecutive failed polls / lighting frames, counted separately.
    poll_failures: u32,
    light_failures: u32,
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
            held_keys: BTreeSet::new(),
            history: Arc::new(BTreeMap::new()),
            stats: BTreeMap::new(),
            hardware: Vec::new(),
            last_error: None,
            last_update: None,
        },
    );

    // Re-apply remembered toggles (Win lock etc.).
    if dev.restore_toggles() {
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

    let mut ws = WorkerState { applied_fans: HashMap::new(), last_sample: None, poll_failures: 0, light_failures: 0 };
    let started = h.shared.read().started;
    let mut next_poll = Instant::now();
    let mut next_frame = Instant::now();
    let mut lighting = h.config.read().lighting(&id);
    let mut overrides = crate::lighting::overrides(&h.config.read(), &id, &zones);
    // Synced devices run the shared effect across the PC layout.
    let canvas_of = |zones: &[LightZone], layout: &[opendevicecontrol_core::LedShape]| {
        let cfg = h.config.read();
        if cfg.synced(&id) { crate::layout::canvas_positions(&cfg, &id, zones, layout) } else { None }
    };
    let mut canvas = canvas_of(&zones, &layout);
    // What the running effects remember between frames.
    let mut fx = crate::fx::FxState::default();
    let mut fx_overrides: Vec<crate::fx::FxState> = Vec::new();

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
                let r = release(&id, &mut *dev);
                if let Some(v) = h.shared.write().devices.get_mut(&id) {
                    v.online = false;
                }
                release_held(&h, &id);
                let _ = done.send(r);
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
                    overrides = crate::lighting::overrides(&h.config.read(), &id, &zones);
                    next_frame = Instant::now();
                }
                canvas = canvas_of(&zones, &dev.led_layout());
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
                overrides = crate::lighting::overrides(&h.config.read(), &id, &zones);
                canvas = canvas_of(&zones, &layout);
                next_frame = Instant::now();
                next_poll = next_poll.min(Instant::now() + Duration::from_millis(50));
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                let _ = release(&id, &mut *dev);
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
                        let l = c.lighting_mut(&id);
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
            // A device that takes frames slower than we render them gets
            // them at its own pace, each with the sound's peak since the last.
            let pace = dev.frame_interval().filter(|&p| p > FRAME);
            let frame = preview.unwrap_or_else(|| {
                let sound = |l: &crate::config::Lighting| l.on && l.effect.is_sound();
                let audio = (sound(&lighting) || overrides.iter().any(|(_, l)| sound(l))).then(|| match pace {
                    Some(p) => h.audio.frame_peak(p),
                    None => h.audio.frame(),
                });
                let mut f = match &canvas {
                    Some(pos) => {
                        let at = crate::lighting::Placed { pos, bars: true, centre: Some(crate::layout::CENTRE_Y) };
                        crate::lighting::render_at(&lighting, &zones, &at, t, audio.as_ref(), &mut fx)
                    }
                    None => crate::lighting::render_with(&lighting, &zones, &layout, t, audio.as_ref(), &mut fx),
                };
                crate::lighting::apply_overrides(&mut f, &overrides, t, audio.as_ref(), &mut fx_overrides);
                f
            });
            match if own { Ok(()) } else { dev.set_lights(&frame) } {
                Ok(()) => {
                    let recovered = std::mem::take(&mut ws.light_failures) > 0;
                    let changed = {
                        let mut s = h.shared.write();
                        match s.devices.get_mut(&id) {
                            Some(v) => {
                                if recovered && ws.poll_failures == 0 {
                                    v.last_error = None;
                                }
                                let changed = recovered || v.lights != frame;
                                v.lights = frame;
                                changed
                            }
                            None => false,
                        }
                    };
                    if changed {
                        h.notify();
                    }
                }
                Err(e) => {
                    if fail(&h, &id, &mut ws.light_failures, LIGHT_FAIL_LIMIT, &format!("lighting: {e}")) {
                        let _ = release(&id, &mut *dev);
                        return;
                    }
                }
            }
            next_frame = now
                + if own {
                    FRAME
                } else if crate::lighting::is_animated(&lighting) || overrides.iter().any(|(_, l)| crate::lighting::is_animated(l)) {
                    pace.unwrap_or(FRAME)
                } else {
                    Duration::from_millis(500)
                };
        }

        // ---- sensors / controls ----
        if now >= next_poll {
            let interval = dev
                .poll_interval()
                .unwrap_or_else(|| Duration::from_millis(h.config.read().general.poll_interval_ms.clamp(500, 60_000)));
            next_poll = now + interval;
            match dev.poll() {
                Ok(mut snap) => {
                    ws.poll_failures = 0;
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
                        if ws.light_failures == 0 {
                            v.last_error = None;
                        }
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
                    if !connected {
                        release_held(&h, &id);
                    }
                    h.notify();
                }
                Err(e) => {
                    if fail(&h, &id, &mut ws.poll_failures, POLL_FAIL_LIMIT, &e.to_string()) {
                        let _ = release(&id, &mut *dev);
                        return;
                    }
                    // No fresh temperatures: retry soon, and once the last
                    // good sample is too old, run curves on the device's own
                    // sensors at full speed (fan writes may still work).
                    next_poll = now + interval.min(POLL_RETRY);
                    if cooling_failsafe_due(ws.last_sample, now) {
                        let cfg = h.config.read().clone();
                        apply_fans(&h, &mut *dev, &mut ws, &cfg, &[]);
                    }
                }
            }
        }
    }
}

/// Whether fans should stop trusting the device's last temperatures.
fn cooling_failsafe_due(last_sample: Option<Instant>, now: Instant) -> bool {
    last_sample.is_none_or(|t| now.saturating_duration_since(t) >= COOLING_FAILSAFE_AFTER)
}

/// Record a failure; returns true when the worker should give up (the
/// manager then rescans, so an unplugged or reset device comes back).
fn fail(h: &Handle, id: &str, failures: &mut u32, limit: u32, err: &str) -> bool {
    *failures += 1;
    log::warn!("{id}: {err} ({}x)", *failures);
    let give_up = *failures >= limit;
    if let Some(v) = h.shared.write().devices.get_mut(id) {
        v.last_error = Some(err.to_string());
        if give_up {
            v.online = false;
        }
    }
    if give_up {
        release_held(h, id);
    }
    h.notify();
    if give_up {
        log::warn!("{id}: giving up after repeated failures; will rescan");
    }
    give_up
}

/// The device went away or offline: release whatever it had held, since
/// its own release will never arrive.
fn release_held(h: &Handle, id: &str) {
    let held = h.shared.write().devices.get_mut(id).map(|v| std::mem::take(&mut v.held_keys)).unwrap_or_default();
    for key in held {
        log::debug!("{id}: {key} released (device offline)");
        h.notify_key(&KeyEvent { device: id.into(), key, pressed: false });
    }
}

fn on_key(h: &Handle, device: &str, key: &str, pressed: bool) {
    if let Some(v) = h.shared.write().devices.get_mut(device) {
        if pressed {
            v.last_key = Some((key.to_string(), Instant::now()));
            v.held_keys.insert(key.to_string());
        } else {
            v.held_keys.remove(key);
        }
    }
    if pressed {
        log::debug!("{device}: {key} pressed");
        let action = h.config.read().key_action(device, key);
        // A "Macro" action names a macro in the library; resolve it to its
        // steps and play those. Everything else runs directly.
        if let crate::config::KeyAction::Macro { name } = &action {
            let m = h.config.read().macros.iter().find(|m| &m.name == name).cloned();
            if let Some(m) = m {
                crate::inject::run_macro(m);
            }
        } else {
            crate::inject::run(action);
        }
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
        {
            let mut c = h.config.write();
            for (k, v) in changed {
                c.toggles.insert(k, v);
            }
        }
        if let Err(e) = crate::config::save_shared(&h.config) {
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
        {
            let mut c = h.config.write();
            for (k, v) in changed {
                c.settings.insert(k, v);
            }
        }
        if let Err(e) = crate::config::save_shared(&h.config) {
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
    let find = |list: &[Sensor], sid: &str| list.iter().find(|s| s.id == sid).map(|s| s.value).filter(|v| v.is_finite());
    let Some(src) = source else { return find(local, &ch.curve_sensor) };
    let (dev, sid) = split_source(src)?;
    if dev == own {
        return find(local, sid);
    }
    let shared = h.shared.read();
    if dev == SYSTEM_SOURCE {
        // The cache is refreshed every few seconds; an old one means the
        // provider is stuck.
        if shared.system_updated.is_none_or(|t| t.elapsed() > SYSTEM_SOURCE_MAX_AGE) {
            return None;
        }
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
        if fan_needs_write(ws.applied_fans.get(&ch.id), target) {
            match dev.set_fan(&ch.id, target) {
                Ok(()) => {
                    ws.applied_fans.insert(ch.id.clone(), (target, Instant::now()));
                }
                Err(e) => log::warn!("{id}: set fan {}: {e}", ch.id),
            }
        }
    }
}

/// Whether a fan target has to be (re)sent, given what was last applied.
fn fan_needs_write(applied: Option<&(FanTarget, Instant)>, target: FanTarget) -> bool {
    let Some((prev, at)) = applied else { return true };
    let changed = match (prev, target) {
        // Small duty changes are skipped, but full speed (the fail-safe)
        // always goes through.
        (FanTarget::Duty(a), FanTarget::Duty(b)) => *a != b && (a.abs_diff(b) >= 2 || b == 100),
        (a, b) => *a != b,
    };
    // Periodically re-assert, in case another program changed it.
    changed || at.elapsed() > Duration::from_secs(30)
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
    let (ram, rest): (Vec<Worker>, Vec<Worker>) = std::mem::take(workers).into_iter().partition(is_ram_worker);
    *workers = rest;
    let ids: Vec<String> = ram.iter().map(|w| w.id.clone()).collect();
    if let Err(e) = stop_workers(ram.iter().map(|w| (w.id.as_str(), &w.tx)), WORKER_STOP_TIMEOUT) {
        log::warn!("RAM lighting: {e}");
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
mod tests {
    use super::*;
    use opendevicecontrol_core::{DeviceKind, Snapshot};

    fn handle(config: Config) -> Handle {
        let (tx, _rx) = crossbeam_channel::unbounded();
        Handle {
            config: Arc::new(RwLock::new(config)),
            shared: Arc::new(RwLock::new(Shared {
                started: Instant::now(),
                devices: BTreeMap::new(),
                conflicts: Vec::new(),
                mqtt: MqttStatus::Disabled,
                system_sensors: Vec::new(),
                system_updated: None,
                ram_status: None,
                sensor_status: Vec::new(),
            })),
            cmd: tx,
            listeners: Arc::new(Mutex::new(Vec::new())),
            key_listeners: Arc::new(Mutex::new(Vec::new())),
            quitting: Arc::new(AtomicBool::new(false)),
            mqtt_done: Arc::new(AtomicBool::new(true)),
            shutdown_result: Arc::new(OnceLock::new()),
            audio: crate::audio::Audio::default(),
        }
    }

    /// A device whose sensor reads fail while fan writes still work.
    struct FakeFan {
        info: DeviceInfo,
        written: Vec<FanTarget>,
    }

    impl FakeFan {
        fn new() -> Self {
            FakeFan {
                info: DeviceInfo {
                    id: "psu".into(),
                    kind: DeviceKind::PowerSupply,
                    manufacturer: "Test".into(),
                    model: "Fake".into(),
                    vendor_string: None,
                    product_string: None,
                    usb_vid: 0,
                    usb_pid: 0,
                    serial: None,
                    firmware: None,
                },
                written: Vec::new(),
            }
        }
    }

    impl Device for FakeFan {
        fn info(&self) -> &DeviceInfo {
            &self.info
        }
        fn poll(&mut self) -> opendevicecontrol_core::Result<Snapshot> {
            Err(opendevicecontrol_core::Error::NoReply { cmd: 0 })
        }
        fn set_control(&mut self, id: &str, _: &ControlValue) -> opendevicecontrol_core::Result<()> {
            Err(opendevicecontrol_core::Error::UnknownControl(id.into()))
        }
        fn fan_channels(&self) -> Vec<FanChannel> {
            vec![FanChannel { id: "fan".into(), name: "Fan".into(), min_duty: 30, curve_sensor: "temp".into() }]
        }
        fn set_fan(&mut self, _: &str, target: FanTarget) -> opendevicecontrol_core::Result<()> {
            self.written.push(target);
            Ok(())
        }
    }

    fn temp(id: &str, v: f64) -> Sensor {
        Sensor::new(id, id, SensorKind::Temperature, v)
    }

    fn worker_state() -> WorkerState {
        WorkerState { applied_fans: HashMap::new(), last_sample: None, poll_failures: 0, light_failures: 0 }
    }

    #[test]
    fn full_speed_skips_duty_hysteresis() {
        let now = Instant::now();
        assert!(fan_needs_write(None, FanTarget::Duty(40)));
        assert!(!fan_needs_write(Some(&(FanTarget::Duty(40), now)), FanTarget::Duty(41)));
        assert!(fan_needs_write(Some(&(FanTarget::Duty(40), now)), FanTarget::Duty(42)));
        assert!(fan_needs_write(Some(&(FanTarget::Duty(99), now)), FanTarget::Duty(100)));
        assert!(!fan_needs_write(Some(&(FanTarget::Duty(100), now)), FanTarget::Duty(100)));
        assert!(fan_needs_write(Some(&(FanTarget::Duty(100), now)), FanTarget::Hardware));
    }

    #[test]
    fn cooling_failsafe_is_bounded() {
        let now = Instant::now();
        assert!(cooling_failsafe_due(None, now));
        assert!(!cooling_failsafe_due(Some(now), now + Duration::from_secs(2)));
        assert!(cooling_failsafe_due(Some(now), now + COOLING_FAILSAFE_AFTER));
    }

    #[test]
    fn failed_reads_drive_curves_to_full_speed() {
        let mut cfg = Config::default();
        cfg.set_fan_policy("psu", "fan", FanPolicy::Curve { points: vec![(30.0, 30), (60.0, 100)], source: None });
        let h = handle(cfg.clone());
        let mut dev = FakeFan::new();
        let mut ws = worker_state();
        apply_fans(&h, &mut dev, &mut ws, &cfg, &[temp("temp", 59.0)]);
        assert_eq!(dev.written, vec![FanTarget::Duty(98)]);
        // The read fails: no sensors, so the curve goes to full speed even
        // though that is within the usual hysteresis.
        apply_fans(&h, &mut dev, &mut ws, &cfg, &[]);
        assert_eq!(dev.written.last(), Some(&FanTarget::Duty(100)));
        apply_fans(&h, &mut dev, &mut ws, &cfg, &[temp("temp", 30.0)]);
        assert_eq!(dev.written.last(), Some(&FanTarget::Duty(30)));
        // A non-finite reading is no reading.
        apply_fans(&h, &mut dev, &mut ws, &cfg, &[temp("temp", f64::NAN)]);
        assert_eq!(dev.written.last(), Some(&FanTarget::Duty(100)));
        // Recovery after a valid sample.
        apply_fans(&h, &mut dev, &mut ws, &cfg, &[temp("temp", 30.0)]);
        assert_eq!(dev.written.last(), Some(&FanTarget::Duty(30)));
    }

    #[test]
    fn stale_system_temperature_is_not_used() {
        let h = handle(Config::default());
        let ch = FakeFan::new().fan_channels().remove(0);
        let src = Some("system/cpu_temp");
        assert_eq!(curve_input(&h, "psu", &ch, src, &[]), None, "never refreshed");
        {
            let mut sh = h.shared.write();
            sh.system_sensors = vec![temp("cpu_temp", 45.0)];
            sh.system_updated = Some(Instant::now());
        }
        assert_eq!(curve_input(&h, "psu", &ch, src, &[]), Some(45.0));
        // The provider stalled: the old reading is not trusted.
        let old = Instant::now().checked_sub(SYSTEM_SOURCE_MAX_AGE + Duration::from_secs(1));
        if old.is_some() {
            h.shared.write().system_updated = old;
            assert_eq!(curve_input(&h, "psu", &ch, src, &[]), None);
        }
        {
            let mut sh = h.shared.write();
            sh.system_sensors = vec![temp("cpu_temp", f64::INFINITY)];
            sh.system_updated = Some(Instant::now());
        }
        assert_eq!(curve_input(&h, "psu", &ch, src, &[]), None);
    }

    #[test]
    fn lighting_and_polling_failures_count_separately() {
        let h = handle(Config::default());
        let mut ws = worker_state();
        for _ in 0..LIGHT_FAIL_LIMIT - 1 {
            assert!(!fail(&h, "psu", &mut ws.light_failures, LIGHT_FAIL_LIMIT, "frame"));
        }
        assert_eq!(ws.poll_failures, 0, "frame errors do not count as poll errors");
        for _ in 0..POLL_FAIL_LIMIT - 1 {
            assert!(!fail(&h, "psu", &mut ws.poll_failures, POLL_FAIL_LIMIT, "poll"));
        }
        assert!(fail(&h, "psu", &mut ws.poll_failures, POLL_FAIL_LIMIT, "poll"));
        assert!(fail(&h, "psu", &mut ws.light_failures, LIGHT_FAIL_LIMIT, "frame"));
    }

    #[test]
    fn going_offline_releases_held_keys() {
        let (h, _cmds) = test_handle(Config::default());
        let info = DeviceInfo {
            id: "kb".into(),
            kind: opendevicecontrol_core::DeviceKind::Keyboard,
            manufacturer: String::new(),
            model: "Board".into(),
            vendor_string: None,
            product_string: None,
            usb_vid: 0,
            usb_pid: 0,
            serial: None,
            firmware: None,
        };
        h.shared.write().devices.insert("kb".into(), test_view(info));
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        h.on_key(move |e| ev.lock().push((e.key.clone(), e.pressed)));

        on_key(&h, "kb", "g1", true);
        on_key(&h, "kb", "g2", true);
        on_key(&h, "kb", "g2", false);
        assert_eq!(h.shared.read().devices["kb"].held_keys.iter().collect::<Vec<_>>(), ["g1"]);
        events.lock().clear();
        release_held(&h, "kb");
        assert!(h.shared.read().devices["kb"].held_keys.is_empty());
        assert_eq!(*events.lock(), [("g1".to_string(), false)]);
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

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    use opendevicecontrol_core::{DeviceKind, Snapshot};

    /// A worker thread that answers `Stop` with `answer` (never, if `None`).
    fn fake_worker(answer: Option<Result<(), String>>) -> Sender<WorkerCmd> {
        let (tx, rx) = crossbeam_channel::unbounded();
        std::thread::spawn(move || {
            if let Ok(WorkerCmd::Stop(done)) = rx.recv() {
                match answer {
                    Some(a) => {
                        let _ = done.send(a);
                    }
                    // Stall: keep the request open past the deadline.
                    None => std::thread::sleep(Duration::from_secs(2)),
                }
            }
        });
        tx
    }

    #[test]
    fn stop_workers_reports_failures_and_stalls() {
        let ok = fake_worker(Some(Ok(())));
        let err = fake_worker(Some(Err("boom".into())));
        let stalled = fake_worker(None);
        let stalled2 = fake_worker(None);
        // A worker that already exited: its receiver is gone.
        let (exited, gone) = crossbeam_channel::unbounded::<WorkerCmd>();
        drop(gone);
        let t = Instant::now();
        let workers = [("a", &ok), ("b", &err), ("c", &stalled), ("d", &stalled2), ("e", &exited)];
        let e = stop_workers(workers.into_iter(), Duration::from_millis(300)).unwrap_err();
        // One shared deadline, not one per worker.
        assert!(t.elapsed() < Duration::from_millis(1000), "{:?}", t.elapsed());
        assert!(e.contains("b: boom"), "{e}");
        assert!(e.contains("c: did not stop in time"), "{e}");
        assert!(e.contains("d: did not stop in time"), "{e}");
        assert!(!e.contains("a:") && !e.contains("e:"), "{e}");

        let ok = fake_worker(Some(Ok(())));
        assert_eq!(stop_workers([("a", &ok)].into_iter(), Duration::from_secs(1)), Ok(()));
    }

    struct FailingRelease(DeviceInfo);

    impl Device for FailingRelease {
        fn info(&self) -> &DeviceInfo {
            &self.0
        }
        fn poll(&mut self) -> opendevicecontrol_core::Result<Snapshot> {
            Ok(Snapshot::default())
        }
        fn set_control(&mut self, id: &str, _: &ControlValue) -> opendevicecontrol_core::Result<()> {
            Err(opendevicecontrol_core::Error::UnknownControl(id.into()))
        }
        fn release(&mut self) -> opendevicecontrol_core::Result<()> {
            Err(opendevicecontrol_core::Error::NoReply { cmd: 0xf0 })
        }
    }

    #[test]
    fn release_errors_propagate() {
        let mut dev = FailingRelease(DeviceInfo {
            id: "fake".into(),
            kind: DeviceKind::PowerSupply,
            manufacturer: String::new(),
            model: "Fake".into(),
            vendor_string: None,
            product_string: None,
            usb_vid: 0,
            usb_pid: 0,
            serial: None,
            firmware: None,
        });
        let e = release("fake", &mut dev).unwrap_err();
        assert!(e.contains("did not answer"), "{e}");
    }

    /// A handle whose "manager" answers each shutdown with `answer` after a delay.
    fn fake_handle(answer: Result<(), String>) -> (Handle, Arc<std::sync::atomic::AtomicUsize>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = count.clone();
        std::thread::spawn(move || {
            while let Ok(cmd) = rx.recv() {
                if let Command::Shutdown(done) = cmd {
                    c.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(200));
                    let _ = done.send(answer.clone());
                }
            }
        });
        let h = Handle {
            config: Arc::new(RwLock::new(Config::default())),
            shared: Arc::new(RwLock::new(Shared {
                started: Instant::now(),
                devices: BTreeMap::new(),
                conflicts: Vec::new(),
                mqtt: MqttStatus::Disabled,
                system_sensors: Vec::new(),
                system_updated: None,
                ram_status: None,
                sensor_status: Vec::new(),
            })),
            cmd: tx,
            listeners: Arc::new(Mutex::new(Vec::new())),
            key_listeners: Arc::new(Mutex::new(Vec::new())),
            quitting: Arc::new(AtomicBool::new(false)),
            mqtt_done: Arc::new(AtomicBool::new(true)),
            shutdown_result: Arc::new(OnceLock::new()),
            audio: crate::audio::Audio::default(),
        };
        (h, count)
    }

    #[test]
    fn concurrent_shutdowns_share_one_result() {
        let (h, count) = fake_handle(Err("hxi: did not stop in time".into()));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let h = h.clone();
                std::thread::spawn(move || h.shutdown())
            })
            .collect();
        for t in threads {
            // Every caller waited for the run and sees its failure.
            assert_eq!(t.join().unwrap(), Err("hxi: did not stop in time".into()));
        }
        assert_eq!(h.shutdown(), Err("hxi: did not stop in time".into()));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(h.quitting.load(Ordering::SeqCst));

        let (h, _) = fake_handle(Ok(()));
        assert_eq!(h.shutdown(), Ok(()));
    }
}
