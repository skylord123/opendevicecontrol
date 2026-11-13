//! Drive health on Windows, for the sensor service: the NVMe health log
//! (`IOCTL_STORAGE_QUERY_PROPERTY`, protocol-specific property) and ATA
//! SMART attributes (`SMART_RCV_DRIVE_DATA`) of every physical drive. These
//! need administrator rights (the service runs as LocalSystem), but no
//! driver.

use super::drive_health;
use super::external::{ExtHardware, ExtTree};
use crate::model::HardwareKind;
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows_sys::Win32::System::IO::DeviceIoControl;

const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
const SMART_RCV_DRIVE_DATA: u32 = 0x0007_C088;
const STORAGE_DEVICE_PROPERTY: u32 = 0;
const STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY: u32 = 50;
const PROTOCOL_TYPE_NVME: u32 = 3;
const NVME_DATA_TYPE_LOG_PAGE: u32 = 2;
const NVME_LOG_HEALTH: u32 = 2;
const BUS_NVME: u32 = 17;
const BUS_SATA: u32 = 11;
const BUS_ATA: u32 = 3;

struct Drive(HANDLE);

impl Drop for Drive {
    fn drop(&mut self) {
        // SAFETY: handle from CreateFileW, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

impl Drive {
    fn open(n: u32) -> Option<Drive> {
        let path: Vec<u16> = format!(r"\\.\PhysicalDrive{n}").encode_utf16().chain([0]).collect();
        // SAFETY: plain Win32 call.
        let h = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        (h != INVALID_HANDLE_VALUE && !h.is_null()).then_some(Drive(h))
    }

    fn ioctl(&self, code: u32, input: &[u8], output: &mut [u8]) -> Option<usize> {
        let mut n = 0u32;
        // SAFETY: buffers are valid for their lengths.
        let ok = unsafe {
            DeviceIoControl(
                self.0,
                code,
                input.as_ptr().cast(),
                input.len() as u32,
                output.as_mut_ptr().cast(),
                output.len() as u32,
                &mut n,
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(n as usize)
    }

    /// (name, bus type) from the storage device descriptor.
    fn describe(&self) -> Option<(String, u32)> {
        let mut q = [0u8; 12];
        q[0..4].copy_from_slice(&STORAGE_DEVICE_PROPERTY.to_le_bytes());
        let mut out = vec![0u8; 1024];
        let n = self.ioctl(IOCTL_STORAGE_QUERY_PROPERTY, &q, &mut out)?;
        let out = &out[..n];
        let u32_at = |at: usize| out.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap_or_default()));
        let text = |off: u32| -> String {
            let off = off as usize;
            if off == 0 || off >= out.len() {
                return String::new();
            }
            let end = out[off..].iter().position(|&c| c == 0).map_or(out.len(), |p| off + p);
            String::from_utf8_lossy(&out[off..end]).trim().to_string()
        };
        let vendor = text(u32_at(12)?);
        let product = text(u32_at(16)?);
        let bus = u32_at(28)?;
        let name = match vendor.as_str() {
            "" | "ATA" | "NVMe" => product,
            v if product.starts_with(v) => product,
            v => format!("{v} {product}"),
        };
        Some((name, bus))
    }

    /// NVMe health log page (512 bytes).
    fn nvme_health(&self) -> Option<Vec<u8>> {
        // STORAGE_PROPERTY_QUERY header (8) + STORAGE_PROTOCOL_SPECIFIC_DATA
        // (40) + the log.
        const HEADER: usize = 8;
        const PROTO: usize = 40;
        let mut buf = vec![0u8; HEADER + PROTO + 512];
        buf[0..4].copy_from_slice(&STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY.to_le_bytes());
        let p = HEADER;
        let put = |b: &mut [u8], at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut buf, p, PROTOCOL_TYPE_NVME);
        put(&mut buf, p + 4, NVME_DATA_TYPE_LOG_PAGE);
        put(&mut buf, p + 8, NVME_LOG_HEALTH);
        put(&mut buf, p + 16, PROTO as u32); // data offset, from the protocol data
        put(&mut buf, p + 20, 512); // data length
        let mut out = vec![0u8; buf.len()];
        self.ioctl(IOCTL_STORAGE_QUERY_PROPERTY, &buf, &mut out)?;
        // STORAGE_PROTOCOL_DATA_DESCRIPTOR: version, size, then the
        // protocol data with its offset.
        let offset = u32::from_le_bytes(out[8 + 16..8 + 20].try_into().ok()?) as usize;
        out.get(8 + offset..8 + offset + 512).map(<[u8]>::to_vec)
    }

    /// ATA SMART attribute data (512 bytes).
    fn ata_smart(&self, drive: u8) -> Option<Vec<u8>> {
        // SENDCMDINPARAMS, packed: buffer size, IDEREGS, drive number,
        // reserved.
        let mut input = [0u8; 33];
        input[0..4].copy_from_slice(&512u32.to_le_bytes());
        input[4..12].copy_from_slice(&[0xD0, 1, 1, 0x4F, 0xC2, 0xA0, 0xB0, 0]);
        input[12] = drive;
        // SENDCMDOUTPARAMS: buffer size (4) + DRIVERSTATUS (12) + data.
        let mut out = vec![0u8; 16 + 512];
        self.ioctl(SMART_RCV_DRIVE_DATA, &input, &mut out)?;
        Some(out[16..].to_vec())
    }
}

/// Every physical drive with health data, one node each.
pub fn read() -> ExtTree {
    let mut hardware = Vec::new();
    for n in 0..32u32 {
        let Some(d) = Drive::open(n) else { continue };
        let Some((name, bus)) = d.describe() else { continue };
        let key = format!("disk{n}");
        let sensors = match bus {
            BUS_NVME => d.nvme_health().map(|log| drive_health::nvme_health(&key, &log)).unwrap_or_default(),
            BUS_SATA | BUS_ATA => {
                d.ata_smart(n as u8).map(|data| drive_health::ata_health(&key, &drive_health::ata_attributes(&data))).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        if !sensors.is_empty() {
            hardware.push(ExtHardware { key, name, kind: HardwareKind::Storage, parent: None, sensors });
        }
    }
    ExtTree { source: "OpenDeviceControl sensor service", hardware }
}
