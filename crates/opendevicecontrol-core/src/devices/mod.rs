//! The generic device interface and discovery across all vendors.

// Drivers live under `crate::vendors`; these re-exports keep the
// established `devices::<driver>` paths working.
pub use crate::vendors::corsair::{bragi, cooling, hxi, nxp, ram, void};

use crate::model::*;
use crate::Result;
use std::collections::HashMap;
use std::ffi::CString;

/// Control id (a toggle) that switches between OpenDeviceControl's lighting and the
/// device's own built-in effects, for devices that have both.
pub const SOFTWARE_LIGHTING: &str = "software_lighting";
/// Ids of the controls for the device's built-in lighting effects start
/// with this (front-ends show them with the lighting, not as settings).
pub const HW_LIGHTING_PREFIX: &str = "hw_";

/// Common interface implemented by every supported device.
pub trait Device: Send {
    fn info(&self) -> &DeviceInfo;

    /// Read all sensors and the current value of every control.
    fn poll(&mut self) -> Result<Snapshot>;

    /// Hardware nodes the sensors of the last poll belong to (see
    /// [`Sensor::hardware`]); empty for devices that are one piece of hardware.
    fn hardware(&self) -> Vec<Hardware> {
        Vec::new()
    }

    /// Static limits (over-current, over-voltage, over-temperature…).
    fn limits(&self) -> &[Limit] {
        &[]
    }

    fn set_control(&mut self, id: &str, value: &ControlValue) -> Result<()>;

    /// Ids of controls whose value the front-end should store and re-apply
    /// when the device is opened again (settings the hardware forgets, such
    /// as a pump mode or the LED count of a channel).
    fn remembered_controls(&self) -> Vec<String> {
        Vec::new()
    }

    fn fan_channels(&self) -> Vec<FanChannel> {
        Vec::new()
    }

    fn set_fan(&mut self, _channel: &str, _target: FanTarget) -> Result<()> {
        Ok(())
    }

    /// Lighting zones, in the order `set_lights` expects their LEDs.
    fn light_zones(&self) -> Vec<LightZone> {
        Vec::new()
    }

    /// Set every LED: the concatenation of all zones' LEDs, in zone order.
    /// Called many times per second by the effects engine.
    fn set_lights(&mut self, _colors: &[Rgb]) -> Result<()> {
        Ok(())
    }

    /// Physical arrangement of the LEDs (e.g. a keyboard's keys), for
    /// front-ends that draw the device. Empty: no known geometry.
    fn led_layout(&self) -> Vec<LedShape> {
        Vec::new()
    }

    /// While the device runs its own (built-in) lighting rather than
    /// OpenDeviceControl's: an approximation of what it shows at `t` seconds, one
    /// colour per LED in zone order, for previews. `None`: OpenDeviceControl drives the
    /// LEDs (or no preview is possible).
    fn preview_lights(&mut self, _t: f64) -> Option<Vec<Rgb>> {
        None
    }

    /// A key pressed in the front-end while it shows this device's preview,
    /// for built-in effects that react to key presses. `key` is a lower-case
    /// key name: letters, digits, `f1`–`f12`, `space`, `enter`, `escape`,
    /// `tab`, `backspace`, `left`, `right`, `up`, `down`, `home`, `end`,
    /// `pageup`, `pagedown`, `insert`, `delete`, `minus`, `equals`,
    /// `comma`, `period`, `slash`, `semicolon`, `quote`, `backtick`,
    /// `leftbracket`, `rightbracket`, `backslash`.
    fn preview_key(&mut self, _key: &str) {}

    /// Device-side key remapping and macros, if supported.
    fn keymap_info(&self) -> Option<crate::keymap::KeymapInfo> {
        None
    }

    /// What each key does in `layer` (indexed like the lighting zone, see
    /// [`crate::keymap`]). Slow on wireless links: call from a worker.
    fn read_keymap(&mut self, _layer: u8) -> Result<Vec<crate::keymap::KeyBinding>> {
        Err(crate::Error::NotSupported(""))
    }

    /// Store `keys` (indexed like [`Device::read_keymap`]) as `layer`.
    /// Writes the device's flash: call only on an explicit user action.
    fn write_keymap(&mut self, _layer: u8, _keys: &[crate::keymap::KeyBinding]) -> Result<()> {
        Err(crate::Error::NotSupported(""))
    }

    fn read_macros(&mut self) -> Result<Vec<crate::keymap::Macro>> {
        Err(crate::Error::NotSupported(""))
    }

    /// Store all macros (slot = index). Writes flash.
    fn write_macros(&mut self, _macros: &[crate::keymap::Macro]) -> Result<()> {
        Err(crate::Error::NotSupported(""))
    }

    /// Keys/buttons this device reports to opendevicecontrol.
    fn input_keys(&self) -> Vec<InputKey> {
        Vec::new()
    }

    /// Non-blocking: key events since the last call.
    fn poll_input(&mut self) -> Vec<InputEvent> {
        Vec::new()
    }

    /// How often `poll` (sensors and controls) should run; `None` uses the
    /// global poll interval.
    fn poll_interval(&self) -> Option<std::time::Duration> {
        None
    }

    /// False while a wireless device is out of reach (e.g. a headset that is
    /// switched off while its receiver stays plugged in). The device stays
    /// open and keeps being polled; front-ends show it as offline instead of
    /// treating it as failed, and it comes back by itself.
    fn connected(&self) -> bool {
        true
    }

    /// Called before the device is dropped: return the device to a safe,
    /// firmware-controlled state.
    fn release(&mut self) {}
}

/// Assigns stable ids: the model slug, suffixed `-2`, `-3`… for duplicates.
#[derive(Default)]
pub(crate) struct Ids(HashMap<&'static str, usize>);

impl Ids {
    pub(crate) fn next(&mut self, slug: &'static str) -> String {
        let n = self.0.entry(slug).or_default();
        *n += 1;
        if *n == 1 { slug.to_string() } else { format!("{slug}-{n}") }
    }
}

/// Enumerate and open every supported device, from every vendor.
///
/// Returns each device with the HID path that identifies it (its primary
/// interface). Devices whose primary path is in `skip_paths` are already
/// held by the caller and are not reopened; failures are logged and skipped.
pub fn discover(skip_paths: &[CString]) -> Result<Vec<(CString, Box<dyn Device>)>> {
    let api = hidapi::HidApi::new()?;
    let mut list: Vec<&hidapi::DeviceInfo> = api.device_list().collect();
    // Stable order so duplicate ids are assigned consistently.
    list.sort_by(|a, b| a.path().cmp(b.path()));
    let mut seen = std::collections::HashSet::new();
    list.retain(|d| seen.insert(d.path().to_owned()));

    let mut ids = Ids::default();
    let mut out: Vec<(CString, Box<dyn Device>)> = Vec::new();
    crate::vendors::corsair::discover(&api, &list, &mut ids, skip_paths, &mut out);
    crate::vendors::aula::discover(&api, &list, &mut ids, skip_paths, &mut out);
    for (_, d) in &out {
        crate::system::tree::own(d.info());
    }

    // The computer itself (system stats, presence). Always present.
    let this_pc = CString::new(crate::system::PATH).expect("no NUL");
    if !skip_paths.contains(&this_pc) {
        out.push((this_pc, Box::new(crate::system::ThisPc::open())));
    }
    Ok(out)
}