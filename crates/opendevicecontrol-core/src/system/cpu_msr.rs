//! CPU power, clocks and voltages from model-specific registers.
//!
//! Pure decoding (tested on every platform); the Windows sensor service
//! reads the registers through PawnIO's `AMDFamily17` / `IntelMSR` modules
//! on each core in turn. Formulas as in LibreHardwareMonitor's `Amd17Cpu.cs`
//! and `IntelCpu.cs` (MPL-2.0) and the vendors' manuals.

/// AMD (family 17h–1Ah).
pub mod amd {
    pub const PWR_UNIT: u32 = 0xC001_0299;
    pub const CORE_ENERGY_STAT: u32 = 0xC001_029A;
    pub const PKG_ENERGY_STAT: u32 = 0xC001_029B;
    pub const HW_PSTATE_STATUS: u32 = 0xC001_0293;
    pub const PSTATE_0: u32 = 0xC001_0064;
    /// Read-only aliases of MPERF / APERF.
    pub const MPERF_RO: u32 = 0xC000_00E7;
    pub const APERF_RO: u32 = 0xC000_00E8;

    /// Frequency of a P-state register, MHz.
    pub fn pstate_mhz(family: u32, msr: u64) -> Option<f64> {
        if family >= 0x1A {
            let fid = (msr & 0xFFF) as f64;
            (fid > 0.0).then_some(fid * 5.0)
        } else {
            let fid = (msr & 0xFF) as f64;
            let did = ((msr >> 8) & 0x3F) as f64;
            (did > 0.0 && fid > 0.0).then_some(fid / did * 200.0)
        }
    }

    /// Requested core voltage (VID) from `HW_PSTATE_STATUS`, volts.
    pub fn vid_volts(status: u64) -> f64 {
        1.55 - 0.00625 * ((status >> 14) & 0xFF) as f64
    }
}

/// Intel.
pub mod intel {
    pub const RAPL_POWER_UNIT: u32 = 0x606;
    pub const PKG_ENERGY_STATUS: u32 = 0x611;
    pub const PP0_ENERGY_STATUS: u32 = 0x639;
    pub const DRAM_ENERGY_STATUS: u32 = 0x619;
    /// The integrated graphics' energy counter.
    pub const PP1_ENERGY_STATUS: u32 = 0x641;
    pub const PERF_STATUS: u32 = 0x198;
    pub const THERM_STATUS: u32 = 0x19C;
    pub const PLATFORM_INFO: u32 = 0xCE;
    pub const MPERF: u32 = 0xE7;
    pub const APERF: u32 = 0xE8;

    /// Core voltage from `IA32_PERF_STATUS` bits 47:32 (1/8192 V).
    pub fn vid_volts(perf_status: u64) -> Option<f64> {
        let v = ((perf_status >> 32) & 0xFFFF) as f64 / 8192.0;
        (v > 0.0 && v < 3.0).then_some(v)
    }

    /// Core temperature from `IA32_THERM_STATUS` (valid bit 31, distance
    /// to TjMax in bits 22:16).
    pub fn core_temp(therm: u64, tjmax: f64) -> Option<f64> {
        (therm & (1 << 31) != 0).then_some(tjmax - ((therm >> 16) & 0x7F) as f64)
    }

    /// Base clock from `MSR_PLATFORM_INFO` (ratio in bits 15:8 × 100 MHz).
    pub fn base_mhz(platform_info: u64) -> Option<f64> {
        let r = ((platform_info >> 8) & 0xFF) as f64;
        (r > 0.0).then_some(r * 100.0)
    }
}

/// Joules per count of an energy counter, from AMD's `PWR_UNIT` or Intel's
/// `RAPL_POWER_UNIT` (energy status unit in bits 12:8).
pub fn energy_unit(pwr_unit: u64) -> f64 {
    0.5f64.powi(((pwr_unit >> 8) & 0x1F) as i32)
}

/// Difference of a 32-bit wrapping energy counter.
pub fn energy_delta(prev: u64, cur: u64) -> u64 {
    (cur as u32).wrapping_sub(prev as u32) as u64
}

/// Watts from two energy counter readings `secs` apart.
pub fn watts(prev: u64, cur: u64, unit: f64, secs: f64) -> Option<f64> {
    if secs <= 0.0 {
        return None;
    }
    let w = energy_delta(prev, cur) as f64 * unit / secs;
    (w.is_finite() && w < 2000.0).then_some(w)
}

/// One core's counters at one moment.
#[derive(Debug, Clone, Copy, Default)]
pub struct CoreSample {
    pub aperf: u64,
    pub mperf: u64,
    pub energy: Option<u64>,
}

/// Clock while running (base × ΔAPERF / ΔMPERF) and effective clock over
/// the interval, idle time included (ΔAPERF / Δt), both MHz.
pub fn core_clocks(prev: CoreSample, cur: CoreSample, base_mhz: f64, secs: f64) -> Option<(f64, f64)> {
    let da = cur.aperf.wrapping_sub(prev.aperf) as f64;
    let dm = cur.mperf.wrapping_sub(prev.mperf) as f64;
    if dm <= 0.0 || secs <= 0.0 {
        return None;
    }
    let running = base_mhz * da / dm;
    let effective = da / secs / 1e6;
    (running.is_finite() && running < 10_000.0 && effective < 10_000.0).then_some((running, effective))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amd_formulas() {
        // Zen 4 P0: FID 0xA8 (168), DID 8 → 4200 MHz.
        assert_eq!(amd::pstate_mhz(0x19, 0x8 << 8 | 0xA8), Some(4200.0));
        // Zen 5: FID 0x348 × 5 MHz.
        assert_eq!(amd::pstate_mhz(0x1A, 0x348), Some(4200.0));
        assert!((amd::vid_volts(72 << 14) - 1.1).abs() < 1e-9);
        // ESU 14: 61 µJ per count.
        assert_eq!(energy_unit(14 << 8), 1.0 / 16384.0);
    }

    #[test]
    fn counters_wrap() {
        assert_eq!(energy_delta(0xFFFF_FFF0, 0x10), 0x20);
        // 16384 counts of 1/16384 J in 0.5 s: 2 W.
        assert_eq!(watts(0, 16384, 1.0 / 16384.0, 0.5), Some(2.0));
        let a = CoreSample { aperf: 1_000, mperf: 1_000, energy: None };
        let b = CoreSample { aperf: 1_000 + 2_500_000_000, mperf: 1_000 + 2_000_000_000, energy: None };
        let (run, eff) = core_clocks(a, b, 4200.0, 1.0).unwrap();
        assert_eq!(run, 5250.0);
        assert_eq!(eff, 2500.0);
    }

    #[test]
    fn intel_formulas() {
        assert_eq!(intel::core_temp(1 << 31 | 30 << 16, 100.0), Some(70.0));
        assert_eq!(intel::core_temp(30 << 16, 100.0), None);
        assert_eq!(intel::vid_volts(8192u64 << 32), Some(1.0));
        assert_eq!(intel::base_mhz(36 << 8), Some(3600.0));
    }
}
