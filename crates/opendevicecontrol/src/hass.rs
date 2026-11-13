//! Home Assistant integration via MQTT discovery.
//!
//! Every device becomes an HA device; every sensor an HA `sensor`; every
//! choice control an HA `select`; fan channels get a mode `select` and a
//! fixed-duty `number`. State is published as one retained JSON document per
//! device, so HA (and the other OS when dual-booting) sees the last values.

use crate::config::{Effect, FanPolicy, HexColor, Mqtt};
use crate::service::{Command, DeviceView, Handle, KeyEvent, MqttStatus};
use crossbeam_channel::{select, Receiver};
use opendevicecontrol_core::{ControlKind, ControlValue, SensorKind};
use rumqttc::{Client, Event, LastWill, MqttOptions, Packet, QoS};
use serde_json::{json, Map, Value};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Object id of a sensor's entity: its id, or for a duration published as
/// the moment it counts from, that moment's name (`last_input`).
fn object_id(s: &opendevicecontrol_core::Sensor) -> String {
    match &s.since {
        Some(since) => crate::config::sanitize_id(&since.name.to_lowercase()),
        // HA's `lock` binary sensors are on when *unlocked*: the inverted
        // entity gets its own id, so it isn't confused with the old one.
        None if is_lock(s) => format!("{}_lock", s.id),
        None => s.id.clone(),
    }
}

/// How long an advertised entity's sensor (or control, key…) may be
/// missing from its online device before its Home Assistant entity is
/// removed: long enough for hotplug, a driver reset or a monitoring tool
/// restarting.
const MISSING_GRACE: Duration = Duration::from_secs(10 * 60);

/// Whether `topic` is a retained message the previous name `name` left: a
/// discovery config of this computer (`<prefix>/<component>/<name>_<node>_…/<obj>/config`)
/// or anything under its old base topic (`<name>/<node>/…`). Other
/// computers on the same broker keep theirs.
fn legacy_topic(topic: &str, name: &str, prefix: &str, node: &str) -> bool {
    if topic.starts_with(&format!("{name}/{node}/")) {
        return true;
    }
    let Some(rest) = topic.strip_prefix(&format!("{prefix}/")) else { return false };
    let parts: Vec<&str> = rest.split('/').collect();
    matches!(parts.as_slice(), [_, id, _, "config"] if id.starts_with(&format!("{name}_{node}_")))
}

/// The energy counter in a retained state document, if it holds a valid one.
fn energy_of(payload: &[u8]) -> Option<f64> {
    let v: Value = serde_json::from_slice(payload).ok()?;
    v.get("energy")?.as_f64().filter(|k| k.is_finite() && *k >= 0.0)
}

/// Device ids no longer used (merged into another device): their retained
/// discovery configs and state are removed when they show up.
fn retired_ids() -> &'static [&'static str] {
    opendevicecontrol_core::vendors::aula::keyboard::RETIRED_IDS
}

/// Whether `topic` is a retained message of a retired device of this
/// computer: a discovery config or anything under its own topic.
fn retired_topic(topic: &str, prefix: &str, base: &str, node: &str) -> bool {
    retired_ids().iter().any(|id| {
        if topic.starts_with(&format!("{base}/{id}/")) {
            return true;
        }
        let Some(rest) = topic.strip_prefix(&format!("{prefix}/")) else { return false };
        let parts: Vec<&str> = rest.split('/').collect();
        matches!(parts.as_slice(), [_, dev, _, "config"] if *dev == format!("opendevicecontrol_{node}_{}", id.replace('-', "_")))
    })
}

fn is_lock(s: &opendevicecontrol_core::Sensor) -> bool {
    s.kind.is_binary() && s.class.as_deref() == Some("lock")
}

/// `{{ value_json['id'] }}`: bracket access works for any id (one starting
/// with a digit, or named like a dict method).
fn value_of(id: &str) -> String {
    format!("value_json['{id}']")
}

/// Icon for a control, from what it does.
fn control_icon(c: &opendevicecontrol_core::Control) -> &'static str {
    let id = c.id.to_lowercase();
    let has = |k: &str| id.contains(k);
    if has("ocp") {
        "mdi:flash-alert"
    } else if has("pump") {
        "mdi:pump"
    } else if has("dpi") {
        "mdi:mouse-move-vertical"
    } else if has("poll") {
        "mdi:timer-outline"
    } else if has("sidetone") {
        "mdi:headset"
    } else if has("mic") {
        "mdi:microphone"
    } else if has("sound") {
        "mdi:volume-high"
    } else if has("led") {
        "mdi:led-strip-variant"
    } else if has("lock") {
        "mdi:keyboard-off"
    } else if has("timeout") {
        "mdi:timer-cog-outline"
    } else {
        match c.kind {
            ControlKind::Choice { .. } => "mdi:form-select",
            ControlKind::Toggle => "mdi:toggle-switch-outline",
            ControlKind::Number { .. } => "mdi:tune",
        }
    }
}

/// ISO 8601 UTC ("2026-11-11T19:28:21+00:00") from Unix seconds.
fn iso8601(unix: i64) -> String {
    let (days, secs) = (unix.div_euclid(86_400), unix.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}+00:00", secs / 3600, secs / 60 % 60, secs % 60)
}

pub fn start(h: Handle) {
    let (tick_tx, tick_rx) = crossbeam_channel::bounded::<()>(1);
    h.on_update(move || {
        let _ = tick_tx.try_send(());
    });
    // Key presses are published immediately (not rate limited), as HA events.
    let (key_tx, key_rx) = crossbeam_channel::bounded::<KeyEvent>(256);
    h.on_key(move |e| {
        let _ = key_tx.try_send(e.clone());
    });
    std::thread::Builder::new()
        .name("opendevicecontrol-mqtt".into())
        .spawn(move || loop {
            let cfg = h.config.read().mqtt.clone();
            if !cfg.enabled || cfg.host.trim().is_empty() {
                set_status(&h, MqttStatus::Disabled);
                h.mqtt_done.store(true, Ordering::Relaxed);
                // Wait for configuration change.
                while h.config.read().mqtt == cfg {
                    let _ = tick_rx.recv_timeout(Duration::from_secs(1));
                }
                continue;
            }
            h.mqtt_done.store(false, Ordering::Relaxed);
            // Events queued while MQTT was off or reconnecting are stale;
            // held state is re-read from the devices instead.
            while key_rx.try_recv().is_ok() {}
            Session::new(&h, cfg).run(&tick_rx, &key_rx);
        })
        .expect("spawn mqtt");
}

fn set_status(h: &Handle, s: MqttStatus) {
    let changed = {
        let mut sh = h.shared.write();
        let changed = sh.mqtt != s;
        sh.mqtt = s;
        changed
    };
    if changed {
        h.notify();
    }
}

struct Topics {
    node: String,
    base: String,
    prefix: String,
}

impl Topics {
    fn bridge_availability(&self) -> String {
        format!("{}/status", self.base)
    }
    fn availability(&self, dev: &str) -> String {
        format!("{}/{dev}/availability", self.base)
    }
    fn state(&self, dev: &str) -> String {
        format!("{}/{dev}/state", self.base)
    }
    fn command(&self, dev: &str, ctrl: &str) -> String {
        format!("{}/{dev}/set/{ctrl}", self.base)
    }
    fn light_state(&self, dev: &str) -> String {
        format!("{}/{dev}/light", self.base)
    }
    fn key_event(&self, dev: &str, key: &str) -> String {
        format!("{}/{dev}/key/{key}", self.base)
    }
    fn key_state(&self, dev: &str, key: &str) -> String {
        format!("{}/{dev}/key/{key}/held", self.base)
    }
    fn uid(&self, dev: &str, obj: &str) -> String {
        format!("opendevicecontrol_{}_{}_{}", self.node, dev.replace('-', "_"), obj)
    }
    fn discovery(&self, component: &str, dev: &str, obj: &str) -> String {
        format!("{}/{component}/opendevicecontrol_{}_{}/{obj}/config", self.prefix, self.node, dev.replace('-', "_"))
    }
}

enum Incoming {
    Connected,
    Disconnected(String),
    Publish { topic: String, payload: Vec<u8>, retain: bool },
}

/// Discovery configs advertised for one device: topic → payload as last
/// accepted for sending, `None` when it has to be sent again (after a
/// reconnect or a Home Assistant restart).
type Advertised = HashMap<String, Option<String>>;

struct Session<'a> {
    h: &'a Handle,
    cfg: Mqtt,
    t: Topics,
    client: Client,
    events: Receiver<Incoming>,
    stop: Arc<AtomicBool>,
    connected: bool,
    /// Per device, the discovery configs this run advertised. Only these
    /// are ever removed when their entity goes away: the other OS of a
    /// dual-boot machine shares the node id and may advertise more.
    advertised: HashMap<String, Advertised>,
    /// Advertised entities whose sensor/control is missing from its
    /// (online) device, and since when.
    missing: HashMap<String, Instant>,
    /// Discovery topics of sensors not sent, already cleared this session
    /// (removes entities of sensors the user switched off, even from an
    /// earlier run).
    cleared: HashSet<String>,
    /// State topics whose retained energy value we already merged.
    seeded: HashSet<String>,
    /// Held state ("pressed" sensors) per key state topic, as published.
    held: HashMap<String, bool>,
    /// Messages the client refused (queue full) since the last report.
    dropped: Cell<u32>,
    /// A legacy cleanup message could not be sent: try again next time.
    legacy_failed: bool,
    /// Last light state published per device.
    last_light: std::collections::HashMap<String, Value>,
    /// On/off sensor values (locked, in use, charging…) as last published;
    /// a change triggers an immediate publish instead of waiting for the
    /// regular interval.
    last_binary: std::collections::HashMap<String, Vec<(String, bool)>>,
    publish_after: Instant,
    last_publish: Instant,
    /// Removing the previous name's discovery entries and retained topics:
    /// (previous name, until when).
    legacy: Option<(String, Instant)>,
}

impl<'a> Session<'a> {
    fn topics(cfg: &Mqtt) -> Topics {
        let node = crate::config::sanitize_id(&cfg.node_id);
        Topics {
            base: format!("{}/{}", cfg.base_topic.trim_end_matches('/'), node),
            prefix: cfg.discovery_prefix.trim_end_matches('/').to_string(),
            node,
        }
    }

    fn new(h: &'a Handle, cfg: Mqtt) -> Self {
        let t = Self::topics(&cfg);
        let mut opts = MqttOptions::new(format!("opendevicecontrol_{}", t.node), cfg.host.trim(), cfg.port);
        opts.set_keep_alive(Duration::from_secs(30));
        opts.set_max_packet_size(256 * 1024, 256 * 1024);
        if !cfg.username.is_empty() {
            opts.set_credentials(cfg.username.clone(), cfg.password.clone());
        }
        opts.set_last_will(LastWill::new(t.bridge_availability(), "offline", QoS::AtLeastOnce, true));
        let (client, mut connection) = Client::new(opts, 2048);

        let (ev_tx, events) = crossbeam_channel::unbounded();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        std::thread::Builder::new()
            .name("opendevicecontrol-mqtt-io".into())
            .spawn(move || {
                for ev in connection.iter() {
                    if stop2.load(Ordering::Relaxed) {
                        break;
                    }
                    let msg = match ev {
                        Ok(Event::Incoming(Packet::ConnAck(_))) => Incoming::Connected,
                        Ok(Event::Incoming(Packet::Publish(p))) => {
                            Incoming::Publish { topic: p.topic.clone(), payload: p.payload.to_vec(), retain: p.retain }
                        }
                        Ok(_) => continue,
                        Err(e) => {
                            let _ = ev_tx.send(Incoming::Disconnected(e.to_string()));
                            // rumqttc reconnects on the next iteration.
                            std::thread::sleep(Duration::from_secs(5));
                            continue;
                        }
                    };
                    if ev_tx.send(msg).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn mqtt io");

        set_status(h, MqttStatus::Connecting);
        Self::with_client(h, cfg, client, events, stop)
    }

    fn with_client(h: &'a Handle, cfg: Mqtt, client: Client, events: Receiver<Incoming>, stop: Arc<AtomicBool>) -> Self {
        Self {
            h,
            t: Self::topics(&cfg),
            cfg,
            client,
            events,
            stop,
            connected: false,
            advertised: HashMap::new(),
            missing: HashMap::new(),
            cleared: HashSet::new(),
            seeded: HashSet::new(),
            held: HashMap::new(),
            dropped: Cell::new(0),
            legacy_failed: false,
            last_light: std::collections::HashMap::new(),
            last_binary: std::collections::HashMap::new(),
            publish_after: Instant::now(),
            last_publish: Instant::now() - Duration::from_secs(3600),
            legacy: None,
        }
    }

    fn run(mut self, tick: &Receiver<()>, keys: &Receiver<KeyEvent>) {
        loop {
            select! {
                recv(self.events) -> ev => match ev {
                    Ok(Incoming::Connected) => self.on_connect(),
                    Ok(Incoming::Disconnected(e)) => {
                        self.connected = false;
                        log::warn!("MQTT: {e}");
                        set_status(self.h, MqttStatus::Error(e));
                    }
                    Ok(Incoming::Publish { topic, payload, retain }) => self.on_message(&topic, &payload, retain),
                    Err(_) => {}
                },
                recv(keys) -> k => if let (Ok(k), true) = (k, self.connected) {
                    // Sent immediately (not rate limited) so held-time
                    // automations in HA are accurate. The held state
                    // follows below, from the device's own record.
                    let event = if k.pressed { "press" } else { "release" };
                    self.pubj(self.t.key_event(&k.device, &k.key), &json!({"event_type": event}), false);
                },
                recv(tick) -> _ => {},
                default(Duration::from_secs(1)) => {},
            }
            if self.h.quitting.load(Ordering::Relaxed) {
                // Clean exit: mark entities unavailable instead of stale.
                self.shutdown();
                self.h.mqtt_done.store(true, Ordering::Relaxed);
                loop {
                    std::thread::park();
                }
            }
            if self.h.config.read().mqtt != self.cfg {
                log::info!("MQTT settings changed, reconnecting");
                self.shutdown();
                return;
            }
            if self.connected {
                self.sync_held();
            }
            if self.connected && Instant::now() >= self.publish_after {
                let urgent = self.binary_changed();
                self.publish_states(urgent);
                self.publish_lights(false);
            }
        }
    }

    fn shutdown(&mut self) {
        if self.connected {
            let _ = self.client.try_publish(self.t.bridge_availability(), QoS::AtLeastOnce, true, "offline");
            std::thread::sleep(Duration::from_millis(200));
        }
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.client.try_disconnect();
    }

    /// Queue a publish; false when the client refused it (queue full or
    /// closed), so the caller keeps it pending and tries again.
    fn send(&self, topic: &str, payload: impl Into<Vec<u8>>, retain: bool) -> bool {
        let ok = self.client.try_publish(topic, QoS::AtLeastOnce, retain, payload).is_ok();
        if !ok {
            self.dropped.set(self.dropped.get() + 1);
        }
        ok
    }

    fn pubj(&self, topic: String, v: &Value, retain: bool) -> bool {
        self.send(&topic, v.to_string(), retain)
    }

    /// Send every advertised discovery config and key state again.
    fn resend_all(&mut self) {
        for adv in self.advertised.values_mut() {
            adv.values_mut().for_each(|p| *p = None);
        }
        self.held.clear();
        self.last_light.clear();
    }

    /// Publish the held state of every key whose state differs from what
    /// was last published. Taken from the devices' own record, so a lost
    /// event, a link change or the device going offline can't leave a key
    /// stuck "pressed".
    fn sync_held(&mut self) {
        let t = &self.t;
        let want: Vec<(String, bool)> = {
            let shared = self.h.shared.read();
            shared
                .devices
                .values()
                .flat_map(|v| {
                    v.keys.iter().map(move |k| (t.key_state(&v.info.id, &k.id), v.online && v.held_keys.contains(&k.id)))
                })
                .collect()
        };
        for (topic, held) in want {
            if self.held.get(&topic) != Some(&held) && self.send(&topic, if held { "ON" } else { "OFF" }, true) {
                self.held.insert(topic, held);
            }
        }
    }

    fn on_connect(&mut self) {
        log::info!("MQTT connected to {}:{}", self.cfg.host, self.cfg.port);
        self.connected = true;
        set_status(self.h, MqttStatus::Connected);
        // Re-send discovery, key and light states once after (re)connecting.
        self.resend_all();
        let _ = self.client.try_subscribe(format!("{}/+/set/+", self.t.base), QoS::AtLeastOnce);
        let _ = self.client.try_subscribe(format!("{}/status", self.t.prefix), QoS::AtLeastOnce);
        // Our own retained state: lets us resume the energy counter written by
        // the other OS on a dual-boot machine.
        let _ = self.client.try_subscribe(format!("{}/+/state", self.t.base), QoS::AtLeastOnce);
        let _ = self.client.try_publish(self.t.bridge_availability(), QoS::AtLeastOnce, true, "online");
        // Retired devices (e.g. the Aula cable's own id): their retained
        // entries arrive now and are cleared in `on_message`.
        for id in retired_ids() {
            let uid = format!("opendevicecontrol_{}_{}", self.t.node, id.replace('-', "_"));
            let _ = self.client.try_subscribe(format!("{}/+/{uid}/+/config", self.t.prefix), QoS::AtLeastOnce);
            let _ = self.client.try_subscribe(format!("{}/{id}/#", self.t.base), QoS::AtLeastOnce);
        }
        // Give retained messages time to arrive before the first state publish.
        self.publish_after = Instant::now() + Duration::from_secs(2);
        // After a rename, remove the old name's entities and retained topics
        // before announcing the new ones, so Home Assistant frees their
        // entity ids (device + entity name) and the new ones get the same.
        if let Some(name) = self.cfg.legacy_cleanup.clone() {
            log::info!("MQTT: removing Home Assistant entries left by {name}");
            let a = self.client.try_subscribe(format!("{}/+/+/+/config", self.t.prefix), QoS::AtLeastOnce);
            let b = self.client.try_subscribe(format!("{name}/{}/#", self.t.node), QoS::AtLeastOnce);
            self.legacy_failed = a.is_err() || b.is_err();
            let until = Instant::now() + Duration::from_secs(4);
            self.legacy = Some((name, until));
            self.publish_after = until + Duration::from_secs(2);
        }
    }

    /// During the cleanup window: clear a retained message of the previous
    /// name. Returns true if `topic` was one.
    fn clear_legacy(&mut self, topic: &str, payload: &[u8]) -> bool {
        let Some((name, _)) = &self.legacy else { return false };
        if payload.is_empty() || !legacy_topic(topic, name, &self.t.prefix, &self.t.node) {
            return false;
        }
        // The old state may carry the energy counter the other OS of a
        // dual-boot machine kept: resume from it before it is deleted.
        let old_dev = topic
            .strip_prefix(&format!("{name}/{}/", self.t.node))
            .and_then(|r| r.strip_suffix("/state"))
            .filter(|d| !d.contains('/'))
            .map(|d| crate::config::merged_id(d).unwrap_or(d).to_string());
        if let Some(dev) = old_dev {
            self.seed_energy(topic, &dev, payload);
        }
        if !self.send(topic, "", true) {
            self.legacy_failed = true;
        }
        true
    }

    /// Resume a device's energy counter from a retained state document
    /// (ours, or the other OS's). Each topic is used once it held a valid
    /// value; the service keeps whichever counter is larger.
    fn seed_energy(&mut self, topic: &str, dev: &str, payload: &[u8]) {
        if self.seeded.contains(topic) {
            return;
        }
        if let Some(kwh) = energy_of(payload) {
            self.seeded.insert(topic.to_string());
            let _ = self.h.cmd.send(Command::SeedEnergy { device: dev.to_string(), kwh });
        }
    }

    /// End the cleanup window once it has passed.
    fn finish_legacy(&mut self) {
        let Some((name, until)) = &self.legacy else { return };
        if Instant::now() < *until {
            return;
        }
        let _ = self.client.try_unsubscribe(format!("{}/+/+/+/config", self.t.prefix));
        let _ = self.client.try_unsubscribe(format!("{name}/{}/#", self.t.node));
        if std::mem::take(&mut self.legacy_failed) {
            // Not everything could be queued: keep the task for the next
            // connection instead of marking it done.
            log::warn!("MQTT: some entries left by {name} could not be removed; retrying on the next connection");
            self.legacy = None;
            return;
        }
        log::info!("MQTT: entries left by {name} removed");
        self.legacy = None;
        self.cfg.legacy_cleanup = None;
        self.h.update_config(|c| c.mqtt.legacy_cleanup = None);
    }

    fn on_message(&mut self, topic: &str, payload: &[u8], retain: bool) {
        if self.clear_legacy(topic, payload) {
            return;
        }
        if retired_topic(topic, &self.t.prefix, &self.t.base, &self.t.node) {
            if !payload.is_empty() {
                log::info!("MQTT: removing {topic} (device merged into another)");
                let _ = self.client.try_publish(topic.to_string(), QoS::AtLeastOnce, true, "");
            }
            return;
        }
        let text = String::from_utf8_lossy(payload).trim().to_string();
        if topic == format!("{}/status", self.t.prefix) {
            if text == "online" {
                log::info!("Home Assistant restarted, re-sending discovery");
                self.resend_all();
                self.publish_after = Instant::now() + Duration::from_secs(1);
            }
            return;
        }
        let Some(rest) = topic.strip_prefix(&format!("{}/", self.t.base)) else { return };
        let parts: Vec<&str> = rest.split('/').collect();
        match parts.as_slice() {
            [dev, "state"] => self.seed_energy(topic, dev, payload),
            // A retained command is an old one the broker replays on every
            // (re)connect, not something asked for now.
            [dev, "set", ctrl] if retain => log::info!("ignoring retained HA command {dev}/{ctrl}"),
            [dev, "set", ctrl] => self.on_command(dev, ctrl, &text),
            _ => {}
        }
    }

    fn on_command(&self, dev: &str, ctrl: &str, text: &str) {
        if !self.h.config.read().general.allow_remote_control {
            log::warn!("ignoring HA command {dev}/{ctrl}: remote control disabled");
            return;
        }
        let view = self.h.shared.read().devices.get(dev).cloned();
        let Some(view) = view else { return };
        log::info!("HA command {dev}/{ctrl} = {text}");
        if ctrl == "light" {
            // JSON schema light: {"state":"ON","brightness":0-100,"color":{r,g,b},"effect":".."}
            let Ok(v) = serde_json::from_str::<Value>(text) else { return };
            self.h.update_config(|c| {
                let l = c.lighting.entry(dev.to_string()).or_default();
                if let Some(s) = v.get("state").and_then(Value::as_str) {
                    l.on = s.eq_ignore_ascii_case("ON");
                }
                if let Some(b) = v.get("brightness").and_then(Value::as_f64) {
                    l.brightness = b.round().clamp(0.0, 100.0) as u8;
                }
                if let Some(e) = v.get("effect").and_then(Value::as_str).and_then(|e| l.effect.switch_to(e)) {
                    l.effect = e;
                }
                if let Some(col) = v.get("color") {
                    let ch = |k: &str| col.get(k).and_then(Value::as_u64).unwrap_or(0).min(255) as u8;
                    l.effect.set_color(HexColor(opendevicecontrol_core::Rgb::new(ch("r"), ch("g"), ch("b"))));
                }
            });
            return;
        }
        for ch in &view.fans {
            if ctrl == format!("{}_mode", ch.id) {
                // Back to the mode's last settings (curve, input, duty).
                let p = self.h.config.read().fan_policy_for_mode(dev, &ch.id, text, 50);
                if let Some(p) = p {
                    self.h.set_fan_policy(dev, &ch.id, p);
                }
                return;
            }
            if ctrl == format!("{}_fixed_duty", ch.id) {
                if let Ok(d) = text.parse::<f64>() {
                    let duty = (d.round() as i64).clamp(ch.min_duty as i64, 100) as u8;
                    self.h.set_fan_policy(dev, &ch.id, FanPolicy::Fixed { duty });
                }
                return;
            }
        }
        if let Some(c) = view.controls.iter().find(|c| c.id == ctrl) {
            let value = match &c.kind {
                ControlKind::Toggle => ControlValue::Bool(text.eq_ignore_ascii_case("ON")),
                ControlKind::Choice { .. } => ControlValue::Choice(text.to_string()),
                ControlKind::Number { .. } => match text.parse() {
                    Ok(v) => ControlValue::Number(v),
                    Err(_) => return,
                },
            };
            let _ = self.h.cmd.send(Command::SetControl { device: dev.to_string(), id: ctrl.to_string(), value });
        }
    }

    fn device_block(&self, v: &DeviceView) -> Value {
        // This PC is named after the computer, so several PCs on one broker
        // don't all become "This PC". The app version is in `origin`;
        // `sw_version` is the device's own firmware.
        let name = match (&v.info.kind, &v.info.serial) {
            (opendevicecontrol_core::DeviceKind::Computer, Some(host)) if !host.is_empty() => host.clone(),
            _ => v.info.display_name(),
        };
        let mut d = json!({
            "identifiers": [format!("opendevicecontrol_{}_{}", self.t.node, v.info.id.replace('-', "_"))],
            "name": name,
            "model": v.info.model,
        });
        if let Some(fw) = v.info.firmware.as_deref().filter(|f| !f.is_empty()) {
            d["sw_version"] = json!(fw);
        }
        if !v.info.manufacturer.is_empty() {
            d["manufacturer"] = json!(v.info.manufacturer);
        }
        if v.info.kind == opendevicecontrol_core::DeviceKind::Computer {
            // This PC: show the OS as the model and the hardware alongside.
            if let Some(os) = &v.info.vendor_string {
                d["model"] = json!(os);
            }
            if let Some(hw) = &v.info.product_string {
                d["hw_version"] = json!(hw);
            }
        } else if let Some(s) = &v.info.serial {
            d["serial_number"] = json!(s);
        }
        d
    }

    fn common(&self, v: &DeviceView, obj: &str, name: &str) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("name".into(), json!(name));
        m.insert("unique_id".into(), json!(self.t.uid(&v.info.id, obj)));
        m.insert("state_topic".into(), json!(self.t.state(&v.info.id)));
        m.insert(
            "availability".into(),
            json!([
                {"topic": self.t.bridge_availability()},
                {"topic": self.t.availability(&v.info.id)}
            ]),
        );
        m.insert("availability_mode".into(), json!("all"));
        m.insert("device".into(), self.device_block(v));
        m.insert(
            "origin".into(),
            json!({"name": "OpenDeviceControl", "sw_version": env!("CARGO_PKG_VERSION"), "support_url": env!("CARGO_PKG_REPOSITORY")}),
        );
        m
    }

    /// Discovery config of one sensor: (topic, payload).
    fn sensor_config(&self, v: &DeviceView, s: &opendevicecontrol_core::Sensor) -> (String, Value) {
        let dev = &v.info.id;
        // Detail sensors of This PC's tree are named after their hardware
        // ("CPU Package" is ambiguous; "AMD Ryzen 7 7800X3D Package" isn't).
        let name = match s.hardware.as_deref().and_then(|h| v.node(h)) {
            Some(h) if s.extra && !s.name.to_lowercase().contains(&h.name.to_lowercase()) => format!("{} {}", h.name, s.name),
            _ => s.name.clone(),
        };
        if let Some(since) = &s.since {
            // The moment a duration counts from: changes only when it
            // happens again, so Home Assistant records nothing in between.
            let obj = object_id(s);
            let mut m = self.common(v, &obj, &since.name);
            m.insert("value_template".into(), json!(format!("{{{{ {} }}}}", value_of(&obj))));
            m.insert("device_class".into(), json!("timestamp"));
            if let Some(i) = &s.icon {
                m.insert("icon".into(), json!(i));
            }
            if s.diagnostic {
                m.insert("entity_category".into(), json!("diagnostic"));
            }
            return (self.t.discovery("sensor", dev, &obj), Value::Object(m));
        }
        {
            let s = &opendevicecontrol_core::Sensor { name, ..s.clone() };
            let obj = object_id(s);
            let mut m = self.common(v, &obj, &s.name);
            if s.kind.is_binary() {
                // `lock`: on means unlocked.
                let (on, off) = if is_lock(s) { ("OFF", "ON") } else { ("ON", "OFF") };
                m.insert(
                    "value_template".into(),
                    json!(format!("{{{{ '{on}' if {} | float(0) > 0 else '{off}' }}}}", value_of(&s.id))),
                );
                let class = s.class.as_deref().or(if s.kind == SensorKind::Charging { Some("battery_charging") } else { None });
                if let Some(c) = class {
                    m.insert("device_class".into(), json!(c));
                }
                if let Some(i) = &s.icon {
                    m.insert("icon".into(), json!(i));
                }
                m.insert("payload_on".into(), json!("ON"));
                m.insert("payload_off".into(), json!("OFF"));
                if s.diagnostic {
                    m.insert("entity_category".into(), json!("diagnostic"));
                }
                return (self.t.discovery("binary_sensor", dev, &obj), Value::Object(m));
            }
            m.insert("value_template".into(), json!(format!("{{{{ {} }}}}", value_of(&s.id))));
            let (class, state_class, icon): (Option<&str>, &str, Option<&str>) = match s.kind {
                SensorKind::Voltage => (Some("voltage"), "measurement", None),
                SensorKind::Current => (Some("current"), "measurement", None),
                SensorKind::Power => (Some("power"), "measurement", None),
                SensorKind::Energy => (Some("energy"), "total_increasing", None),
                SensorKind::Temperature => (Some("temperature"), "measurement", None),
                SensorKind::FanSpeed => (None, "measurement", Some("mdi:fan")),
                SensorKind::Duty => (None, "measurement", Some("mdi:fan-chevron-up")),
                SensorKind::Efficiency => (None, "measurement", Some("mdi:gauge")),
                SensorKind::Battery => (Some("battery"), "measurement", None),
                SensorKind::Load => (None, "measurement", Some("mdi:gauge")),
                SensorKind::DataSize => (Some("data_size"), "measurement", None),
                SensorKind::DataRate => (Some("data_rate"), "measurement", None),
                SensorKind::Frequency => (Some("frequency"), "measurement", None),
                SensorKind::Flow => (Some("volume_flow_rate"), "measurement", None),
                // Published as binary sensors above; never reach here.
                SensorKind::Charging | SensorKind::State => (None, "measurement", None),
                SensorKind::Duration => (
                    Some("duration"),
                    if s.id == "total_uptime" { "total_increasing" } else { "measurement" },
                    None,
                ),
            };
            if let Some(c) = s.class.as_deref().or(class) {
                m.insert("device_class".into(), json!(c));
            }
            if let Some(i) = s.icon.as_deref().or(icon) {
                m.insert("icon".into(), json!(i));
            }
            m.insert("state_class".into(), json!(state_class));
            // HA's spelling of revolutions per minute.
            let unit = if s.kind == SensorKind::FanSpeed { "rpm" } else { s.kind.unit() };
            m.insert("unit_of_measurement".into(), json!(unit));
            m.insert("suggested_display_precision".into(), json!(s.kind.precision()));
            if s.diagnostic {
                m.insert("entity_category".into(), json!("diagnostic"));
            }
            (self.t.discovery("sensor", dev, &s.id), Value::Object(m))
        }
    }

    /// Bring a device's Home Assistant entities in line with what it has
    /// now and what the user chose to send: advertise new or changed ones,
    /// remove ones switched off right away and ones whose sensor, control
    /// or key went away after [`MISSING_GRACE`] (never while the device is
    /// offline). Bookkeeping only advances for messages the client
    /// accepted, so anything refused is tried again next time. At most
    /// `budget` removals per call (the MQTT queue is bounded and a PC has
    /// hundreds of sensors).
    fn sync_device(&mut self, v: &DeviceView, cfg: &crate::config::Config, budget: &mut usize) {
        let dev = v.info.id.clone();
        let mut want: Vec<(String, Value)> = self.device_configs(v);
        let mut unwanted = HashSet::new();
        for s in &v.sensors {
            let comp = if s.kind.is_binary() { "binary_sensor" } else { "sensor" };
            let obj = object_id(s);
            if obj != s.id && *budget > 0 {
                // Earlier versions published it under its own id (as a
                // duration, or as a lock with the opposite meaning).
                let old = self.t.discovery(comp, &dev, &s.id);
                if !self.cleared.contains(&old) {
                    *budget -= 1;
                    if self.send(&old, "", true) {
                        self.cleared.insert(old);
                    }
                }
            }
            if cfg.sends(&dev, s) {
                want.push(self.sensor_config(v, s));
            } else {
                unwanted.insert(self.t.discovery(comp, &dev, &obj));
            }
        }

        let mut adv = self.advertised.remove(&dev).unwrap_or_default();
        let wanted: HashSet<String> = want.iter().map(|(t, _)| t.clone()).collect();
        for (topic, config) in want {
            self.missing.remove(&topic);
            let config = config.to_string();
            if adv.get(&topic).is_some_and(|p| p.as_ref() == Some(&config)) {
                continue;
            }
            if self.send(&topic, config.clone(), true) {
                self.cleared.remove(&topic);
                adv.insert(topic, Some(config));
            }
        }
        let now = Instant::now();
        let stale: Vec<String> = adv.keys().filter(|t| !wanted.contains(*t)).cloned().collect();
        for topic in stale {
            let remove = if unwanted.contains(&topic) {
                true
            } else if !v.online {
                // Hardware that is away keeps its entities.
                self.missing.remove(&topic);
                false
            } else {
                now.duration_since(*self.missing.entry(topic.clone()).or_insert(now)) >= MISSING_GRACE
            };
            if remove && *budget > 0 {
                *budget -= 1;
                if self.send(&topic, "", true) {
                    adv.remove(&topic);
                    self.missing.remove(&topic);
                    self.cleared.insert(topic);
                }
            }
        }
        // Sensors switched off before this run: clear what an earlier run
        // may have left.
        for topic in unwanted {
            if !adv.contains_key(&topic) && !self.cleared.contains(&topic) && *budget > 0 {
                *budget -= 1;
                if self.send(&topic, "", true) {
                    self.cleared.insert(topic);
                }
            }
        }
        self.advertised.insert(dev, adv);
    }

    /// A device this run advertised is gone from the service (e.g. RAM
    /// support switched off): remove its entities and retained topics.
    /// Returns true once everything is cleared.
    fn forget_device(&mut self, dev: &str, budget: &mut usize) -> bool {
        let Some(mut adv) = self.advertised.remove(dev) else { return true };
        let topics: Vec<String> = adv.keys().cloned().collect();
        for topic in topics {
            if *budget == 0 {
                break;
            }
            *budget -= 1;
            if self.send(&topic, "", true) {
                adv.remove(&topic);
                self.missing.remove(&topic);
            }
        }
        let keys: Vec<String> = self.held.keys().filter(|t| t.starts_with(&format!("{}/{dev}/key/", self.t.base))).cloned().collect();
        for topic in keys {
            if self.send(&topic, "", true) {
                self.held.remove(&topic);
            }
        }
        if adv.is_empty() && !self.held.keys().any(|t| t.starts_with(&format!("{}/{dev}/key/", self.t.base))) {
            let topics = [self.t.availability(dev), self.t.state(dev), self.t.light_state(dev)];
            if topics.iter().map(|t| self.send(t, "", true)).fold(true, |a, b| a & b) {
                log::info!("MQTT: removed {dev} from Home Assistant (no longer present)");
                self.last_light.remove(dev);
                return true;
            }
        }
        self.advertised.insert(dev.to_string(), adv);
        false
    }

    /// Discovery configs of a device's controls, lighting, keys and fans:
    /// (topic, payload).
    fn device_configs(&self, v: &DeviceView) -> Vec<(String, Value)> {
        let dev = &v.info.id;
        let mut out = Vec::new();
        for c in &v.controls {
            match &c.kind {
                ControlKind::Choice { options } => {
                    let mut m = self.common(v, &c.id, &c.name);
                    m.insert("command_topic".into(), json!(self.t.command(dev, &c.id)));
                    m.insert("options".into(), json!(options));
                    m.insert("value_template".into(), json!(format!("{{{{ {} }}}}", value_of(&c.id))));
                    m.insert("entity_category".into(), json!("config"));
                    m.insert("icon".into(), json!(control_icon(c)));
                    out.push((self.t.discovery("select", dev, &c.id), Value::Object(m)));
                }
                ControlKind::Toggle => {
                    let mut m = self.common(v, &c.id, &c.name);
                    m.insert("command_topic".into(), json!(self.t.command(dev, &c.id)));
                    m.insert("value_template".into(), json!(format!("{{{{ {} }}}}", value_of(&c.id))));
                    m.insert("payload_on".into(), json!("ON"));
                    m.insert("payload_off".into(), json!("OFF"));
                    m.insert("entity_category".into(), json!("config"));
                    m.insert("icon".into(), json!(control_icon(c)));
                    out.push((self.t.discovery("switch", dev, &c.id), Value::Object(m)));
                }
                ControlKind::Number { min, max, step, unit } => {
                    let mut m = self.common(v, &c.id, &c.name);
                    m.insert("command_topic".into(), json!(self.t.command(dev, &c.id)));
                    m.insert("value_template".into(), json!(format!("{{{{ {} }}}}", value_of(&c.id))));
                    m.insert("min".into(), json!(min));
                    m.insert("max".into(), json!(max));
                    m.insert("step".into(), json!(step));
                    m.insert("mode".into(), json!("slider"));
                    if !unit.is_empty() {
                        m.insert("unit_of_measurement".into(), json!(unit));
                    }
                    if matches!(unit.as_str(), "s" | "min" | "h" | "ms") {
                        m.insert("device_class".into(), json!("duration"));
                    }
                    m.insert("entity_category".into(), json!("config"));
                    m.insert("icon".into(), json!(control_icon(c)));
                    out.push((self.t.discovery("number", dev, &c.id), Value::Object(m)));
                }
            }
        }

        // Lighting: one HA light per device (JSON schema).
        if !v.zones.is_empty() {
            let mut m = self.common(v, "lighting", "Lighting");
            m.remove("state_topic");
            m.insert("schema".into(), json!("json"));
            m.insert("state_topic".into(), json!(self.t.light_state(dev)));
            m.insert("command_topic".into(), json!(self.t.command(dev, "light")));
            m.insert("brightness".into(), json!(true));
            m.insert("brightness_scale".into(), json!(100));
            m.insert("supported_color_modes".into(), json!(["rgb"]));
            m.insert("effect".into(), json!(true));
            m.insert("effect_list".into(), json!(Effect::NAMES));
            m.insert("icon".into(), json!("mdi:led-strip-variant"));
            out.push((self.t.discovery("light", dev, "lighting"), Value::Object(m)));
        }

        // Keys, two entities each:
        // * binary_sensor "<key> pressed": on while held, off when released,
        //   for "held for N seconds" automations;
        // * event "<key> key": fires `press` / `release`, for instant triggers.
        for k in &v.keys {
            let obj = format!("key_{}", k.id);
            let mut m = self.common(v, &obj, &format!("{} key", k.name));
            m.insert("state_topic".into(), json!(self.t.key_event(dev, &k.id)));
            m.insert("event_types".into(), json!(["press", "release"]));
            m.insert("device_class".into(), json!("button"));
            m.insert("icon".into(), json!("mdi:gesture-tap-button"));
            out.push((self.t.discovery("event", dev, &obj), Value::Object(m)));

            let obj = format!("key_{}_pressed", k.id);
            let mut m = self.common(v, &obj, &format!("{} pressed", k.name));
            m.insert("state_topic".into(), json!(self.t.key_state(dev, &k.id)));
            m.insert("payload_on".into(), json!("ON"));
            m.insert("payload_off".into(), json!("OFF"));
            m.insert("icon".into(), json!("mdi:keyboard"));
            out.push((self.t.discovery("binary_sensor", dev, &obj), Value::Object(m)));
        }

        for ch in &v.fans {
            let mode_id = format!("{}_mode", ch.id);
            let mut m = self.common(v, &mode_id, &format!("{} mode", ch.name));
            m.insert("command_topic".into(), json!(self.t.command(dev, &mode_id)));
            m.insert("options".into(), json!(FanPolicy::MODES));
            m.insert("value_template".into(), json!(format!("{{{{ {} }}}}", value_of(&mode_id))));
            m.insert("icon".into(), json!("mdi:fan-auto"));
            out.push((self.t.discovery("select", dev, &mode_id), Value::Object(m)));

            let duty_id = format!("{}_fixed_duty", ch.id);
            let mut m = self.common(v, &duty_id, &format!("{} fixed duty", ch.name));
            m.insert("command_topic".into(), json!(self.t.command(dev, &duty_id)));
            m.insert("value_template".into(), json!(format!("{{{{ {} }}}}", value_of(&duty_id))));
            m.insert("min".into(), json!(ch.min_duty));
            m.insert("max".into(), json!(100));
            m.insert("step".into(), json!(1));
            m.insert("mode".into(), json!("slider"));
            m.insert("unit_of_measurement".into(), json!("%"));
            m.insert("icon".into(), json!("mdi:fan"));
            out.push((self.t.discovery("number", dev, &duty_id), Value::Object(m)));
        }
        out
    }

    /// True when any device's on/off sensor (locked, in use…) changed since
    /// the last publish, so automations react within a second.
    fn binary_changed(&mut self) -> bool {
        let shared = self.h.shared.read();
        let mut changed = false;
        for v in shared.devices.values() {
            let now: Vec<(String, bool)> =
                v.sensors.iter().filter(|s| s.kind.is_binary()).map(|s| (s.id.clone(), s.value > 0.0)).collect();
            if now.is_empty() {
                continue;
            }
            if self.last_binary.get(&v.info.id) != Some(&now) {
                changed |= self.last_binary.contains_key(&v.info.id);
                self.last_binary.insert(v.info.id.clone(), now);
            }
        }
        changed
    }

    fn publish_states(&mut self, force: bool) {
        self.finish_legacy();
        if self.legacy.is_some() {
            return;
        }
        let interval = Duration::from_secs(self.cfg.publish_interval_s.max(1));
        if !force && self.last_publish.elapsed() < interval {
            return;
        }
        self.last_publish = Instant::now();
        let devices: Vec<DeviceView> = self.h.shared.read().devices.values().cloned().collect();
        let cfg = self.h.config.read().clone();
        let mut budget = 200;
        let gone: Vec<String> =
            self.advertised.keys().filter(|d| !devices.iter().any(|v| &v.info.id == *d)).cloned().collect();
        for dev in gone {
            self.forget_device(&dev, &mut budget);
        }
        for v in &devices {
            let dev = v.info.id.clone();
            if v.last_update.is_none() {
                continue; // not read yet
            }
            self.sync_device(v, &cfg, &mut budget);
            let _ = self.client.try_publish(
                self.t.availability(&dev),
                QoS::AtLeastOnce,
                true,
                if v.online { "online" } else { "offline" },
            );
            if !v.online {
                continue;
            }
            let mut st = Map::new();
            for s in v.sensors.iter().filter(|s| cfg.sends(&dev, s)) {
                if let Some(since) = &s.since {
                    st.insert(object_id(s), json!(iso8601(since.at)));
                    continue;
                }
                let p = 10f64.powi(s.kind.precision() as i32 + 1);
                st.insert(s.id.clone(), json!((s.value * p).round() / p));
            }
            for c in &v.controls {
                let val = match &c.value {
                    ControlValue::Bool(b) => json!(if *b { "ON" } else { "OFF" }),
                    ControlValue::Choice(s) => json!(s),
                    ControlValue::Number(n) => json!(n),
                };
                st.insert(c.id.clone(), val);
            }
            for ch in &v.fans {
                let p = cfg.fan_policy(&dev, &ch.id);
                st.insert(format!("{}_mode", ch.id), json!(p.mode_name()));
                let duty = match p {
                    FanPolicy::Fixed { duty } => duty as f64,
                    _ => v.value(&format!("{}_duty", ch.id)).unwrap_or(0.0).round(),
                };
                st.insert(format!("{}_fixed_duty", ch.id), json!(duty));
            }
            self.pubj(self.t.state(&dev), &Value::Object(st), true);
        }
        self.publish_lights(false);
        let dropped = self.dropped.replace(0);
        if dropped > 0 {
            log::warn!("MQTT: {dropped} messages not queued (queue full); retrying");
        }
    }

    /// Publish each device's light state when it changed (or `force`), so
    /// HA reflects changes from the GUI or brightness key right away.
    fn publish_lights(&mut self, force: bool) {
        let devices: Vec<(String, bool)> =
            self.h.shared.read().devices.values().map(|v| (v.info.id.clone(), !v.zones.is_empty())).collect();
        let cfg = self.h.config.read().clone();
        for (dev, has_lights) in devices {
            // Only once its light entity is advertised on this connection.
            let light = self.t.discovery("light", &dev, "lighting");
            if !has_lights || !self.advertised.get(&dev).and_then(|a| a.get(&light)).is_some_and(Option::is_some) {
                continue;
            }
            let l = cfg.lighting(&dev);
            let c = l.effect.primary_color().0;
            let st = json!({
                "state": if l.on { "ON" } else { "OFF" },
                "brightness": l.brightness,
                "color_mode": "rgb",
                "color": {"r": c.r, "g": c.g, "b": c.b},
                "effect": l.effect.name(),
            });
            if (force || self.last_light.get(&dev) != Some(&st)) && self.pubj(self.t.light_state(&dev), &st, true) {
                self.last_light.insert(dev, st);
            }
        }
    }
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00+00:00");
        assert_eq!(iso8601(1_794_450_501), "2026-11-12T02:28:21+00:00");
        assert_eq!(iso8601(951_782_400), "2000-02-29T00:00:00+00:00");
        let s = opendevicecontrol_core::Sensor::new("idle_time", "Idle time", SensorKind::Duration, 5.0).since(0, "Last input");
        assert_eq!(object_id(&s), "last_input");
        // Cleanup after a rename touches only the old name's own retained topics.
        assert!(legacy_topic("homeassistant/sensor/rigmeld_my_pc_this_pc/cpu_load/config", "rigmeld", "homeassistant", "my_pc"));
        assert!(legacy_topic("rigmeld/my_pc/this_pc/state", "rigmeld", "homeassistant", "my_pc"));
        assert!(!legacy_topic("rigmeld/other_pc/this_pc/state", "rigmeld", "homeassistant", "my_pc"));
        assert!(!legacy_topic("rigmeld/my_pcx/this_pc/state", "rigmeld", "homeassistant", "my_pc"));
        assert!(!legacy_topic("homeassistant/sensor/rigmeld_other_pc_this_pc/cpu_load/config", "rigmeld", "homeassistant", "my_pc"));
        assert!(!legacy_topic("homeassistant/sensor/opendevicecontrol_my_pc_this_pc/cpu_load/config", "rigmeld", "homeassistant", "my_pc"));
        assert!(!legacy_topic("homeassistant/sensor/zigbee_thing/temp/config", "rigmeld", "homeassistant", "my_pc"));
        // Retired device ids: only their own topics.
        let r = |t: &str| retired_topic(t, "homeassistant", "opendevicecontrol", "my_pc");
        assert!(r("homeassistant/sensor/opendevicecontrol_my_pc_aula_f99_usb/battery/config"));
        assert!(r("opendevicecontrol/aula-f99-usb/state"));
        assert!(!r("homeassistant/sensor/opendevicecontrol_my_pc_aula_f99/battery/config"));
        assert!(!r("opendevicecontrol/aula-f99/state"));
        let l = opendevicecontrol_core::Sensor::new("locked", "Locked", SensorKind::State, 1.0).class("lock");
        assert_eq!(object_id(&l), "locked_lock");
        assert_eq!(value_of("12v"), "value_json['12v']");
    }

    use crate::config::Config;
    use crate::service::{test_handle, test_view};
    use opendevicecontrol_core::{Control, DeviceInfo, DeviceKind, InputKey, LightZone, Sensor};

    const BASE: &str = "opendevicecontrol/my_pc";

    /// A session on a client whose queue holds `cap` messages and is read
    /// by the test instead of a broker.
    fn session(h: &Handle, cap: usize) -> (Session<'_>, flume::Receiver<rumqttc::Request>) {
        let (tx, rx) = flume::bounded(cap);
        let cfg = Mqtt { node_id: "my_pc".into(), ..Mqtt::default() };
        let (_ev_tx, events) = crossbeam_channel::unbounded();
        let mut s = Session::with_client(h, cfg, Client::from_sender(tx), events, Arc::new(AtomicBool::new(false)));
        s.connected = true;
        (s, rx)
    }

    /// Queued publishes: (topic, payload).
    fn drain(rx: &flume::Receiver<rumqttc::Request>) -> Vec<(String, String)> {
        rx.try_iter()
            .filter_map(|r| match r {
                rumqttc::Request::Publish(p) => Some((p.topic, String::from_utf8_lossy(&p.payload).into_owned())),
                _ => None,
            })
            .collect()
    }

    fn device(id: &str) -> DeviceView {
        let info = DeviceInfo {
            id: id.into(),
            kind: DeviceKind::Keyboard,
            manufacturer: "Test".into(),
            model: "Board".into(),
            vendor_string: None,
            product_string: None,
            usb_vid: 0,
            usb_pid: 0,
            serial: None,
            firmware: None,
        };
        let mut v = test_view(info);
        v.sensors = vec![
            Sensor::new("temp", "Temperature", SensorKind::Temperature, 40.0),
            Sensor::new("power", "Power", SensorKind::Power, 10.0),
            Sensor::new("energy", "Energy", SensorKind::Energy, 1.0),
        ];
        v.controls = vec![Control {
            id: "sound".into(),
            name: "Sound".into(),
            kind: ControlKind::Toggle,
            value: ControlValue::Bool(false),
            help: None,
        }];
        v.zones = vec![LightZone { id: "keys".into(), name: "Keys".into(), leds: 1 }];
        v.keys = vec![InputKey { id: "g1".into(), name: "G1".into(), assignable: true }];
        v
    }

    fn config_topic(component: &str, obj: &str) -> String {
        format!("homeassistant/{component}/opendevicecontrol_my_pc_kb/{obj}/config")
    }

    fn cleared(sent: &[(String, String)], topic: &str) -> bool {
        sent.iter().any(|(t, p)| t == topic && p.is_empty())
    }

    #[test]
    fn refused_publishes_are_retried_until_everything_converges() {
        let (h, _cmds) = test_handle(Config::default());
        h.shared.write().devices.insert("kb".into(), device("kb"));
        // Far less room than one cycle needs (the real queue holds 2048).
        let (mut s, rx) = session(&h, 3);
        let mut seen = HashSet::new();
        for _ in 0..20 {
            s.publish_states(true);
            seen.extend(drain(&rx).into_iter().map(|(t, _)| t));
        }
        for t in [
            config_topic("sensor", "temp"),
            config_topic("sensor", "power"),
            config_topic("sensor", "energy"),
            config_topic("switch", "sound"),
            config_topic("light", "lighting"),
            config_topic("event", "key_g1"),
            config_topic("binary_sensor", "key_g1_pressed"),
            format!("{BASE}/kb/light"),
            format!("{BASE}/kb/state"),
        ] {
            assert!(seen.contains(&t), "{t} never sent");
        }
        // Nothing is re-sent once accepted.
        s.publish_states(true);
        assert!(drain(&rx).iter().all(|(t, _)| !t.ends_with("/config")));
    }

    #[test]
    fn entities_follow_the_device() {
        let (h, _cmds) = test_handle(Config::default());
        h.shared.write().devices.insert("kb".into(), device("kb"));
        let (mut s, rx) = session(&h, 4096);
        s.publish_states(true);
        drain(&rx);

        // A control that appears later is advertised.
        h.shared.write().devices.get_mut("kb").unwrap().controls.push(Control {
            id: "mic".into(),
            name: "Mic".into(),
            kind: ControlKind::Toggle,
            value: ControlValue::Bool(true),
            help: None,
        });
        s.publish_states(true);
        assert!(drain(&rx).iter().any(|(t, p)| t == &config_topic("switch", "mic") && !p.is_empty()));

        // A sensor that goes away stays during the grace period…
        h.shared.write().devices.get_mut("kb").unwrap().sensors.retain(|x| x.id != "power");
        s.publish_states(true);
        assert!(!cleared(&drain(&rx), &config_topic("sensor", "power")));
        // …is kept while the device is offline…
        h.shared.write().devices.get_mut("kb").unwrap().online = false;
        s.publish_states(true);
        assert!(!cleared(&drain(&rx), &config_topic("sensor", "power")));
        assert!(!s.missing.contains_key(&config_topic("sensor", "power")));
        // …and is removed once missing that long from an online device.
        h.shared.write().devices.get_mut("kb").unwrap().online = true;
        s.publish_states(true);
        drain(&rx);
        let Some(long_ago) = Instant::now().checked_sub(MISSING_GRACE + Duration::from_secs(1)) else { return };
        s.missing.insert(config_topic("sensor", "power"), long_ago);
        s.publish_states(true);
        assert!(cleared(&drain(&rx), &config_topic("sensor", "power")));

        // A sensor the user switches off goes right away.
        h.config.write().ha_sensors.insert("kb/temp".into(), false);
        s.publish_states(true);
        assert!(cleared(&drain(&rx), &config_topic("sensor", "temp")));

        // A device removed from the service loses its entities and topics,
        // and nothing it never advertised is touched.
        h.shared.write().devices.remove("kb");
        s.publish_states(true);
        let sent = drain(&rx);
        for t in [config_topic("sensor", "energy"), config_topic("switch", "sound"), format!("{BASE}/kb/availability")] {
            assert!(cleared(&sent, &t), "{t} not cleared");
        }
        assert!(!cleared(&sent, &config_topic("sensor", "power")), "cleared twice");
        assert!(sent.iter().all(|(t, p)| p.is_empty() && (t.contains("_kb/") || t.starts_with(&format!("{BASE}/kb/")))));
        assert!(s.advertised.is_empty());
    }

    #[test]
    fn energy_seeding_and_legacy_cleanup() {
        let (h, cmds) = test_handle(Config::default());
        let (mut s, rx) = session(&h, 64);
        let seeds = || -> Vec<f64> {
            cmds.try_iter()
                .filter_map(|c| match c {
                    Command::SeedEnergy { kwh, .. } => Some(kwh),
                    _ => None,
                })
                .collect()
        };
        let state = format!("{BASE}/psu/state");
        s.on_message(&state, b"not json", true);
        s.on_message(&state, br#"{"power": 3}"#, true);
        s.on_message(&state, br#"{"energy": -1}"#, true);
        assert!(seeds().is_empty());
        s.on_message(&state, br#"{"energy": 12.5}"#, true);
        s.on_message(&state, br#"{"energy": 13.0}"#, false);
        assert_eq!(seeds(), [12.5]);

        // Cleanup of a previous name: this computer's topics only, and the
        // old energy counter is resumed before it is deleted.
        s.legacy = Some(("rigmeld".into(), Instant::now() + Duration::from_secs(60)));
        s.on_message("rigmeld/other_pc/psu/state", br#"{"energy": 99.0}"#, true);
        s.on_message("homeassistant/sensor/rigmeld_other_pc_psu/power/config", b"{}", true);
        assert!(drain(&rx).is_empty());
        assert!(seeds().is_empty());
        s.on_message("rigmeld/my_pc/psu/state", br#"{"energy": 20.0}"#, true);
        s.on_message("homeassistant/sensor/rigmeld_my_pc_psu/power/config", b"{}", true);
        assert_eq!(seeds(), [20.0]);
        let sent = drain(&rx);
        assert!(cleared(&sent, "rigmeld/my_pc/psu/state"));
        assert!(cleared(&sent, "homeassistant/sensor/rigmeld_my_pc_psu/power/config"));

        // A cleanup that could not be queued is not marked done.
        s.legacy_failed = true;
        s.cfg.legacy_cleanup = Some("rigmeld".into());
        s.legacy = Some(("rigmeld".into(), Instant::now()));
        s.finish_legacy();
        assert!(s.legacy.is_none());
        assert_eq!(s.cfg.legacy_cleanup.as_deref(), Some("rigmeld"));
    }

    #[test]
    fn retained_commands_are_ignored() {
        let (h, cmds) = test_handle(Config::default());
        h.shared.write().devices.insert("kb".into(), device("kb"));
        let (mut s, _rx) = session(&h, 64);
        let topic = format!("{BASE}/kb/set/sound");
        s.on_message(&topic, b"ON", true);
        assert!(cmds.try_iter().all(|c| !matches!(c, Command::SetControl { .. })));
        s.on_message(&topic, b"ON", false);
        assert!(cmds.try_iter().any(|c| matches!(c, Command::SetControl { id, .. } if id == "sound")));
    }

    #[test]
    fn held_keys_follow_the_device() {
        let (h, _cmds) = test_handle(Config::default());
        h.shared.write().devices.insert("kb".into(), device("kb"));
        let held = format!("{BASE}/kb/key/g1/held");
        let (mut s, rx) = session(&h, 1);
        // Released at first.
        s.sync_held();
        assert_eq!(drain(&rx), [(held.clone(), "OFF".to_string())]);
        // Pressed, but the queue is full: published once there is room.
        h.shared.write().devices.get_mut("kb").unwrap().held_keys.insert("g1".into());
        assert!(s.send("filler", "", false));
        s.sync_held();
        drain(&rx);
        s.sync_held();
        assert_eq!(drain(&rx), [(held.clone(), "ON".to_string())]);
        // A reconnect during the hold re-sends it as held, not released.
        s.resend_all();
        s.sync_held();
        assert_eq!(drain(&rx), [(held.clone(), "ON".to_string())]);
        // The device dropping off releases it even without a release event.
        h.shared.write().devices.get_mut("kb").unwrap().online = false;
        s.sync_held();
        assert_eq!(drain(&rx), [(held, "OFF".to_string())]);
    }
}
