//! Windows I²C on NVIDIA graphics cards through NvAPI (`nvapi64.dll`,
//! installed with every NVIDIA driver). No extra driver and no
//! administrator rights needed.
//!
//! NvAPI exports a single function, `nvapi_QueryInterface`, that hands out
//! the others by id; the ids and `NV_I2C_INFO_V3` follow NVIDIA's public
//! NvAPI SDK headers. RGB controllers on graphics cards sit on the GPU's
//! internal I²C port 1 (not a display's DDC port).

use super::SmBus;
use crate::{Error, Result};
use std::ffi::c_void;
use std::sync::OnceLock;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

type QueryInterface = unsafe extern "C" fn(u32) -> *mut c_void;
type Initialize = unsafe extern "C" fn() -> i32;
type EnumPhysicalGpus = unsafe extern "C" fn(*mut [*mut c_void; MAX_GPUS], *mut u32) -> i32;
type GetPciIdentifiers = unsafe extern "C" fn(*mut c_void, *mut u32, *mut u32, *mut u32, *mut u32) -> i32;
type GetBusId = unsafe extern "C" fn(*mut c_void, *mut u32) -> i32;
type I2cEx = unsafe extern "C" fn(*mut c_void, *mut I2cInfoV3, *mut u32) -> i32;

const MAX_GPUS: usize = 64;
const ID_INITIALIZE: u32 = 0x0150_E828;
const ID_ENUM_PHYSICAL_GPUS: u32 = 0xE5AC_921F;
const ID_GET_PCI_IDENTIFIERS: u32 = 0x2DDF_B66E;
const ID_GET_BUS_ID: u32 = 0x1BE0_B8E5;
const ID_I2C_READ_EX: u32 = 0x4D7B_0709;
const ID_I2C_WRITE_EX: u32 = 0x283A_C65A;
/// `NVAPI_I2C_SPEED_DEPRECATED`: use `speed_khz` instead.
const SPEED_DEPRECATED: u32 = 0xFFFF;
/// The GPU's internal I²C port that carries RGB controllers.
const RGB_PORT: u8 = 1;

/// `NV_I2C_INFO_V3`.
#[repr(C)]
struct I2cInfoV3 {
    version: u32,
    display_mask: u32,
    is_ddc_port: u8,
    /// 8-bit address: the 7-bit address shifted left once.
    dev_address: u8,
    reg_address: *mut u8,
    reg_address_size: u32,
    data: *mut u8,
    size: u32,
    speed: u32,
    speed_khz: u32,
    port_id: u8,
    is_port_id_set: u32,
}

const I2C_INFO_V3_VER: u32 = std::mem::size_of::<I2cInfoV3>() as u32 | (3 << 16);

struct Api {
    enum_gpus: EnumPhysicalGpus,
    pci_ids: GetPciIdentifiers,
    bus_id: Option<GetBusId>,
    read: I2cEx,
    write: I2cEx,
}

fn api() -> std::result::Result<&'static Api, String> {
    static API: OnceLock<std::result::Result<Api, String>> = OnceLock::new();
    API.get_or_init(|| {
        let dll: Vec<u16> = "nvapi64.dll".encode_utf16().chain([0]).collect();
        // SAFETY: loading the driver's DLL from System32 and transmuting the
        // functions it hands out to their documented signatures. The module
        // is never unloaded, so the pointers stay valid.
        unsafe {
            let lib = LoadLibraryExW(dll.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
            if lib.is_null() {
                return Err("nvapi64.dll not found (no NVIDIA driver)".into());
            }
            let qi = GetProcAddress(lib, c"nvapi_QueryInterface".as_ptr().cast()).ok_or("nvapi64.dll lacks nvapi_QueryInterface")?;
            let qi: QueryInterface = std::mem::transmute(qi);
            let get = |id: u32, name: &str| {
                let p = qi(id);
                if p.is_null() {
                    Err(format!("NvAPI lacks {name}"))
                } else {
                    Ok(p)
                }
            };
            let init: Initialize = std::mem::transmute(get(ID_INITIALIZE, "NvAPI_Initialize")?);
            let r = init();
            if r != 0 {
                return Err(format!("NvAPI_Initialize failed ({r})"));
            }
            Ok(Api {
                enum_gpus: std::mem::transmute::<*mut c_void, EnumPhysicalGpus>(get(ID_ENUM_PHYSICAL_GPUS, "NvAPI_EnumPhysicalGPUs")?),
                pci_ids: std::mem::transmute::<*mut c_void, GetPciIdentifiers>(get(ID_GET_PCI_IDENTIFIERS, "NvAPI_GPU_GetPCIIdentifiers")?),
                bus_id: get(ID_GET_BUS_ID, "NvAPI_GPU_GetBusId").ok().map(|p| std::mem::transmute::<*mut c_void, GetBusId>(p)),
                read: std::mem::transmute::<*mut c_void, I2cEx>(get(ID_I2C_READ_EX, "NvAPI_I2CReadEx")?),
                write: std::mem::transmute::<*mut c_void, I2cEx>(get(ID_I2C_WRITE_EX, "NvAPI_I2CWriteEx")?),
            })
        }
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// One NVIDIA GPU and its PCI identity.
pub struct NvGpu {
    handle: *mut c_void,
    pub vendor: u16,
    pub device: u16,
    pub sub_vendor: u16,
    pub sub_device: u16,
    /// PCI bus number, when NvAPI reports it.
    pub bus: Option<u32>,
}

// SAFETY: NvAPI's GPU handles are process-wide and NvAPI serialises I²C
// transfers itself; each bus is used from one thread at a time.
unsafe impl Send for NvGpu {}

/// Every NVIDIA GPU NvAPI sees.
pub fn gpus() -> Result<Vec<NvGpu>> {
    let api = api().map_err(Error::Bus)?;
    let mut handles = [std::ptr::null_mut(); MAX_GPUS];
    let mut n = 0u32;
    // SAFETY: NvAPI fills at most MAX_GPUS handles and the count.
    let r = unsafe { (api.enum_gpus)(&mut handles, &mut n) };
    if r != 0 {
        return Err(Error::Bus(format!("NvAPI_EnumPhysicalGPUs failed ({r})")));
    }
    let mut out = Vec::new();
    for &handle in handles.iter().take((n as usize).min(MAX_GPUS)) {
        let (mut dev, mut sub, mut rev, mut ext) = (0u32, 0u32, 0u32, 0u32);
        // SAFETY: a handle from EnumPhysicalGPUs and four out-parameters.
        if unsafe { (api.pci_ids)(handle, &mut dev, &mut sub, &mut rev, &mut ext) } != 0 {
            continue;
        }
        let mut bus = 0u32;
        // SAFETY: as above, one out-parameter.
        let bus = api.bus_id.filter(|f| unsafe { f(handle, &mut bus) } == 0).map(|_| bus);
        out.push(NvGpu { handle, vendor: dev as u16, device: (dev >> 16) as u16, sub_vendor: sub as u16, sub_device: (sub >> 16) as u16, bus });
    }
    Ok(out)
}

/// The RGB I²C port of one GPU.
pub struct NvBus {
    gpu: NvGpu,
    name: String,
}

impl NvBus {
    pub fn open(gpu: NvGpu) -> Self {
        let name = format!("NVIDIA GPU {:04x}:{:04x} I2C port {RGB_PORT}", gpu.vendor, gpu.device);
        NvBus { gpu, name }
    }

    /// One transfer: `reg` (if any) then `data` bytes written or read.
    fn xfer(&mut self, addr: u8, write: bool, reg: Option<u8>, data: &mut [u8]) -> Result<()> {
        let api = api().map_err(Error::Bus)?;
        let mut reg_byte = reg.unwrap_or(0);
        let mut info = I2cInfoV3 {
            version: I2C_INFO_V3_VER,
            display_mask: 0,
            is_ddc_port: 0,
            dev_address: addr << 1,
            reg_address: if reg.is_some() { &mut reg_byte } else { std::ptr::null_mut() },
            reg_address_size: reg.is_some() as u32,
            data: data.as_mut_ptr(),
            size: data.len() as u32,
            speed: SPEED_DEPRECATED,
            speed_khz: 0,
            port_id: RGB_PORT,
            is_port_id_set: 1,
        };
        let mut unknown = 0u32;
        let f = if write { api.write } else { api.read };
        // SAFETY: `info` points at buffers that outlive the call.
        let r = unsafe { f(self.gpu.handle, &mut info, &mut unknown) };
        if r != 0 {
            return Err(Error::Bus(format!("NvAPI I2C {} {addr:#04x}/{:#04x} failed ({r})", if write { "write" } else { "read" }, reg_byte)));
        }
        Ok(())
    }
}

impl SmBus for NvBus {
    fn name(&self) -> &str {
        &self.name
    }

    fn read_byte_data(&mut self, addr: u8, reg: u8) -> Result<u8> {
        let mut b = [0u8; 1];
        self.xfer(addr, false, Some(reg), &mut b)?;
        Ok(b[0])
    }

    fn read_word_data(&mut self, addr: u8, reg: u8) -> Result<u16> {
        let mut b = [0u8; 2];
        self.xfer(addr, false, Some(reg), &mut b)?;
        Ok(u16::from_le_bytes(b))
    }

    fn write_byte_data(&mut self, addr: u8, reg: u8, value: u8) -> Result<()> {
        self.xfer(addr, true, Some(reg), &mut [value])
    }

    fn write_word_data(&mut self, addr: u8, reg: u8, value: u16) -> Result<()> {
        self.xfer(addr, true, Some(reg), &mut value.to_le_bytes())
    }

    fn write_block_data(&mut self, addr: u8, reg: u8, data: &[u8]) -> Result<()> {
        if data.is_empty() || data.len() > super::BLOCK_MAX {
            return Err(Error::Bus(format!("invalid block length {}", data.len())));
        }
        // SMBus block write: the byte count goes first.
        let mut buf = Vec::with_capacity(data.len() + 1);
        buf.push(data.len() as u8);
        buf.extend_from_slice(data);
        self.xfer(addr, true, Some(reg), &mut buf)
    }
}
