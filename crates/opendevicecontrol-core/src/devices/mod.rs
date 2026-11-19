//! The generic device interface and discovery across all vendors.

// Drivers live under `crate::vendors`; these re-exports keep the
// established `devices::<driver>` paths working.
pub use crate::vendors::corsair::{bragi, cooling, hxi, nxp, ram, void};

use crate::model::*;
use crate::Result;
use std::collections::HashSet;
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

    /// The least time between `set_lights` frames the device keeps up
    /// with; `None` takes frames as fast as they are rendered. Animated
    /// lighting is rendered at this pace, just before each frame is sent.
    fn frame_interval(&self) -> Option<std::time::Duration> {
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
    /// firmware-controlled state. Implementations attempt every step and
    /// return the first error, so callers know the handoff is incomplete.
    fn release(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Assigns stable ids: the model slug, suffixed `-2`, `-3`… for duplicates.
///
/// Ids already held by running workers are reserved and never handed out
/// again, so a new device can't take the id of one that is still open
/// (whatever order the scan sees them in). Drivers allocate only for devices
/// they are about to open, not for held ones.
#[derive(Default)]
pub(crate) struct Ids {
    reserved: HashSet<String>,
    given: HashSet<String>,
}

impl Ids {
    pub(crate) fn reserving<I: IntoIterator<Item = String>>(held: I) -> Self {
        Ids { reserved: held.into_iter().collect(), given: HashSet::new() }
    }

    pub(crate) fn next(&mut self, slug: &'static str) -> String {
        let id = (1..)
            .map(|n: usize| if n == 1 { slug.to_string() } else { format!("{slug}-{n}") })
            .find(|id| !self.reserved.contains(id) && !self.given.contains(id))
            .expect("unbounded");
        self.given.insert(id.clone());
        id
    }
}

/// Enumerate and open every supported device, from every vendor.
///
/// `held` lists the devices the caller already runs, as (primary path, id):
/// those paths are not reopened and their ids are not reused. Returns each
/// new device with the HID path that identifies it (its primary interface);
/// failures are logged and skipped.
pub fn discover(held: &[(CString, String)]) -> Result<Vec<(CString, Box<dyn Device>)>> {
    let skip_paths: Vec<CString> = held.iter().map(|(p, _)| p.clone()).collect();
    let skip_paths = skip_paths.as_slice();
    let api = hidapi::HidApi::new()?;
    let mut list: Vec<&hidapi::DeviceInfo> = api.device_list().collect();
    // Stable order so duplicate ids are assigned consistently.
    list.sort_by(|a, b| a.path().cmp(b.path()));
    // Drop entries listed twice, but keep every top-level collection: on
    // Linux all collections of an interface share one hidraw path, and the
    // drivers match on the collection's usage page.
    let mut seen = std::collections::HashSet::new();
    list.retain(|d| seen.insert((d.path().to_owned(), d.usage_page(), d.usage())));

    let mut ids = Ids::reserving(held.iter().map(|(_, id)| id.clone()));
    let mut out: Vec<(CString, Box<dyn Device>)> = Vec::new();
    crate::vendors::corsair::discover(&api, &list, &mut ids, skip_paths, &mut out);
    crate::vendors::aula::discover(&api, &list, &mut ids, skip_paths, &mut out);
    crate::vendors::speedlink::discover(&api, &list, &mut ids, skip_paths, &mut out);
    crate::vendors::asrock::discover(&api, &list, &mut ids, skip_paths, &mut out);
    crate::vendors::asus::discover(&mut ids, skip_paths, &mut out);
    crate::vendors::system76::discover(&mut ids, skip_paths, &mut out);
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

#[cfg(test)]
mod ids_tests {
    use super::Ids;

    #[test]
    fn first_scan_numbers_from_the_bare_slug() {
        let mut ids = Ids::default();
        assert_eq!(ids.next("k70"), "k70");
        assert_eq!(ids.next("k70"), "k70-2");
        assert_eq!(ids.next("hx1000i"), "hx1000i");
    }

    #[test]
    fn new_device_never_takes_a_held_id() {
        // A running "k70" is held; a second unit sorts before it on rescan.
        let mut ids = Ids::reserving(["k70".to_string()]);
        assert_eq!(ids.next("k70"), "k70-2");
    }

    #[test]
    fn replugged_first_unit_gets_its_id_back() {
        // Two units ran as "k70" and "k70-2"; the first was unplugged and
        // comes back while "k70-2" is still held.
        let mut ids = Ids::reserving(["k70-2".to_string()]);
        assert_eq!(ids.next("k70"), "k70");
        assert_eq!(ids.next("k70"), "k70-3");
    }
}
