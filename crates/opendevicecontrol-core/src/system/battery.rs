//! Laptop batteries on Windows: the battery class driver's IOCTLs (the
//! ones Windows' own battery meter uses). No administrator rights needed.
//! Sensors as on Linux (`hwmon::batteries`): charge level, charging, rate,
//! voltage, wear level, plus temperature and cycle count when reported.

use super::external::{ExtHardware, ExtSensor};
use crate::model::SensorKind;

/// `BATTERY_STATUS` and `BATTERY_INFORMATION` fields that matter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Reading {
    /// `BATTERY_CHARGING` (4), `BATTERY_DISCHARGING` (2)…
    pub power_state: u32,
    /// mWh, or `u32::MAX` if unknown.
    pub capacity: u32,
    /// mV.
    pub voltage: u32,
    /// mW, positive while charging.
    pub rate: i32,
    pub full: u32,
    pub design: u32,
    /// Tenths of a kelvin, 0 if not reported.
    pub temperature: u32,
}

const UNKNOWN: u32 = 0xFFFF_FFFF;
const CHARGING: u32 = 0x4;

/// Sensors of one battery (`key` prefixes the ids).
pub fn sensors(key: &str, r: &Reading) -> Vec<ExtSensor> {
    let id = |s: &str| format!("{key}_{s}");
    let mut out = Vec::new();
    if r.capacity != UNKNOWN && r.full > 0 && r.full != UNKNOWN {
        let mut s = ExtSensor::new(id("level"), "Charge level", SensorKind::Battery, (f64::from(r.capacity) / f64::from(r.full) * 100.0).clamp(0.0, 100.0).round());
        s.default_on = true;
        out.push(s);
    }
    let mut c = ExtSensor::new(id("charging"), "Charging", SensorKind::Charging, f64::from(u8::from(r.power_state & CHARGING != 0)));
    c.default_on = true;
    out.push(c);
    // BATTERY_UNKNOWN_RATE is 0x80000000.
    if r.rate != i32::MIN {
        out.push(ExtSensor::new(id("power"), "Charge / discharge rate", SensorKind::Power, f64::from(r.rate) / 1000.0));
    }
    if r.voltage != UNKNOWN && r.voltage > 0 {
        out.push(ExtSensor::new(id("voltage"), "Voltage", SensorKind::Voltage, f64::from(r.voltage) / 1000.0));
    }
    if r.design > 0 && r.design != UNKNOWN && r.full != UNKNOWN {
        let mut s = ExtSensor::new(id("health"), "Wear level (capacity left)", SensorKind::Load, (f64::from(r.full) / f64::from(r.design) * 100.0).round());
        s.diagnostic = true;
        out.push(s);
    }
    if r.temperature > 0 {
        let t = f64::from(r.temperature) / 10.0 - 273.15;
        if t > 0.0 && t < 100.0 {
            out.push(ExtSensor::new(id("temp"), "Temperature", SensorKind::Temperature, (t * 10.0).round() / 10.0));
        }
    }
    out
}

/// Every battery, one node each.
#[cfg(windows)]
pub fn read() -> Vec<ExtHardware> {
    win::read()
}

#[cfg(not(windows))]
pub fn read() -> Vec<ExtHardware> {
    Vec::new()
}

#[cfg(windows)]
mod win {
    use super::*;
    use crate::model::HardwareKind;
    use windows_sys::core::GUID;
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
        DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
    };
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::IO::DeviceIoControl;

    const GUID_DEVICE_BATTERY: GUID = GUID::from_u128(0x72631e54_78a4_11d0_bcf7_00aa00b7b32a);
    const IOCTL_BATTERY_QUERY_TAG: u32 = 0x0029_4040;
    const IOCTL_BATTERY_QUERY_INFORMATION: u32 = 0x0029_4044;
    const IOCTL_BATTERY_QUERY_STATUS: u32 = 0x0029_404C;
    const INFO_BATTERY: u32 = 0;
    const INFO_TEMPERATURE: u32 = 2;
    const INFO_DEVICE_NAME: u32 = 4;
    const INFO_MANUFACTURER: u32 = 6;

    fn ioctl(h: windows_sys::Win32::Foundation::HANDLE, code: u32, input: &[u8], out: &mut [u8]) -> Option<usize> {
        let mut n = 0u32;
        // SAFETY: buffers valid for their lengths.
        let ok = unsafe {
            DeviceIoControl(h, code, input.as_ptr().cast(), input.len() as u32, out.as_mut_ptr().cast(), out.len() as u32, &mut n, std::ptr::null_mut())
        };
        (ok != 0).then_some(n as usize)
    }

    fn u32_at(b: &[u8], at: usize) -> u32 {
        b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes(s.try_into().unwrap_or_default()))
    }

    fn query(h: windows_sys::Win32::Foundation::HANDLE, tag: u32, level: u32, out: &mut [u8]) -> Option<usize> {
        let mut q = [0u8; 12];
        q[0..4].copy_from_slice(&tag.to_le_bytes());
        q[4..8].copy_from_slice(&level.to_le_bytes());
        ioctl(h, IOCTL_BATTERY_QUERY_INFORMATION, &q, out)
    }

    fn text(h: windows_sys::Win32::Foundation::HANDLE, tag: u32, level: u32) -> Option<String> {
        let mut out = [0u8; 512];
        let n = query(h, tag, level, &mut out)?;
        let w: Vec<u16> = out[..n].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|&c| c != 0).collect();
        let s = String::from_utf16_lossy(&w).trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    pub fn read() -> Vec<ExtHardware> {
        let mut out = Vec::new();
        // SAFETY: SetupAPI enumeration with buffers sized as it asks; every
        // handle is closed.
        unsafe {
            let set = SetupDiGetClassDevsW(&GUID_DEVICE_BATTERY, std::ptr::null(), std::ptr::null_mut(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE);
            if set as isize == -1 {
                return out;
            }
            for i in 0..8u32 {
                let mut iface: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
                iface.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
                if SetupDiEnumDeviceInterfaces(set, std::ptr::null(), &GUID_DEVICE_BATTERY, i, &mut iface) == 0 {
                    break;
                }
                let mut need = 0u32;
                SetupDiGetDeviceInterfaceDetailW(set, &iface, std::ptr::null_mut(), 0, &mut need, std::ptr::null_mut());
                if need == 0 {
                    continue;
                }
                let mut buf = vec![0u64; (need as usize).div_ceil(8)];
                let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
                (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
                if SetupDiGetDeviceInterfaceDetailW(set, &iface, detail, need, std::ptr::null_mut(), std::ptr::null_mut()) == 0 {
                    continue;
                }
                let path = std::ptr::addr_of!((*detail).DevicePath) as *const u16;
                let h = CreateFileW(
                    path,
                    GENERIC_READ | GENERIC_WRITE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    std::ptr::null_mut(),
                );
                if h == INVALID_HANDLE_VALUE {
                    continue;
                }
                let mut tag_buf = [0u8; 4];
                let wait = 0u32.to_le_bytes();
                let tag = ioctl(h, IOCTL_BATTERY_QUERY_TAG, &wait, &mut tag_buf).map(|_| u32::from_le_bytes(tag_buf)).filter(|t| *t != 0);
                if let Some(tag) = tag {
                    let mut r = Reading::default();
                    let mut info = [0u8; 36];
                    if query(h, tag, INFO_BATTERY, &mut info).is_some() {
                        r.design = u32_at(&info, 12);
                        r.full = u32_at(&info, 16);
                    }
                    let mut t = [0u8; 4];
                    if query(h, tag, INFO_TEMPERATURE, &mut t).is_some() {
                        r.temperature = u32::from_le_bytes(t);
                    }
                    // BATTERY_WAIT_STATUS (tag, no timeout) → BATTERY_STATUS.
                    let mut ws = [0u8; 20];
                    ws[0..4].copy_from_slice(&tag.to_le_bytes());
                    let mut st = [0u8; 16];
                    if ioctl(h, IOCTL_BATTERY_QUERY_STATUS, &ws, &mut st).is_some() {
                        r.power_state = u32_at(&st, 0);
                        r.capacity = u32_at(&st, 4);
                        r.voltage = u32_at(&st, 8);
                        r.rate = u32_at(&st, 12) as i32;
                        let key = format!("battery_{}", i);
                        let model = text(h, tag, INFO_DEVICE_NAME);
                        let maker = text(h, tag, INFO_MANUFACTURER);
                        let name = match (maker, model) {
                            (Some(m), Some(d)) => format!("{m} {d}"),
                            (None, Some(d)) => d,
                            _ => format!("Battery {}", i + 1),
                        };
                        let mut s = sensors(&key, &r);
                        s.retain(|x| x.value.is_finite());
                        out.push(ExtHardware { key, name, kind: HardwareKind::Battery, parent: None, sensors: s });
                    }
                }
                CloseHandle(h);
            }
            SetupDiDestroyDeviceInfoList(set);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes() {
        let r = Reading { power_state: 4, capacity: 30_000, voltage: 12_300, rate: 15_000, full: 50_000, design: 60_000, temperature: 3031 };
        let s = sensors("battery_0", &r);
        let v = |id: &str| s.iter().find(|x| x.id == id).map(|x| x.value);
        assert_eq!(v("battery_0_level"), Some(60.0));
        assert_eq!(v("battery_0_charging"), Some(1.0));
        assert_eq!(v("battery_0_power"), Some(15.0));
        assert_eq!(v("battery_0_voltage"), Some(12.3));
        assert_eq!(v("battery_0_health"), Some(83.0));
        assert_eq!(v("battery_0_temp"), Some(30.0));
    }
}
