//! Acceso a la GPU: trait común, telemetría y límites. Implementaciones: NVML (real) y demo.

mod demo;
mod nvml;

pub use demo::DemoBackend;
pub use nvml::NvmlBackend;

use crate::error::{HwError, HwResult};

/// Topes "blandos" de seguridad para los sliders (se intersectan con los del driver).
pub const SOFT_CORE_OFFSET: (i32, i32) = (-300, 250);
pub const SOFT_MEM_OFFSET: (i32, i32) = (-1000, 1000);
pub const MIN_FAN_PCT: i32 = 30;

pub const THROTTLE_REASONS: [(u64, &str); 5] = [
    (0x04, "Límite de potencia"),
    (0x08, "Frenado por hardware"),
    (0x20, "Límite térmico (SW)"),
    (0x40, "Límite térmico (HW)"),
    (0x80, "Freno de energía"),
];

pub fn throttle_text(mask: u64) -> String {
    THROTTLE_REASONS
        .iter()
        .filter(|(bit, _)| mask & bit != 0)
        .map(|(_, text)| *text)
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Telemetry {
    pub temp: f64,
    pub core_clock: u32,
    pub mem_clock: u32,
    pub fan_pct: u32,
    pub power_w: f64,
    pub util_gpu: u32,
    pub vram_used_mb: u64,
    pub vram_total_mb: u64,
    pub throttle: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Limits {
    pub power_min_w: f64,
    pub power_max_w: f64,
    pub power_default_w: f64,
    pub core_off_min: i32,
    pub core_off_max: i32,
    pub mem_off_min: i32,
    pub mem_off_max: i32,
    pub max_core_clock: i32,
    pub max_mem_clock: i32,
    pub num_fans: u32,
    pub oc_supported: bool,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            power_min_w: 100.0,
            power_max_w: 220.0,
            power_default_w: 220.0,
            core_off_min: SOFT_CORE_OFFSET.0,
            core_off_max: SOFT_CORE_OFFSET.1,
            mem_off_min: SOFT_MEM_OFFSET.0,
            mem_off_max: SOFT_MEM_OFFSET.1,
            max_core_clock: 2100,
            max_mem_clock: 7000,
            num_fans: 0,
            oc_supported: false,
        }
    }
}

/// Lo que la aplicación necesita de una GPU. Se usa siempre desde el hilo de la interfaz.
pub trait GpuBackend {
    fn name(&self) -> &str;
    fn driver(&self) -> &str;
    fn is_demo(&self) -> bool {
        false
    }
    fn limits(&self) -> &Limits;
    /// "nvml", "nvapi", "demo" o `None` si no hay overclock de reloj.
    fn oc_source(&self) -> Option<&str>;
    fn oc_error(&self) -> Option<&str>;

    fn read(&mut self) -> Telemetry;
    /// Lector independiente que puede ejecutarse en otro hilo (NVML). `None` = leer en el hilo de la interfaz.
    fn telemetry_source(&self) -> Option<Box<dyn FnMut() -> Telemetry + Send>> {
        None
    }
    fn core_offset(&mut self) -> i32;
    fn mem_offset(&mut self) -> i32;

    // Escritura (requiere Administrador)
    fn set_power_limit(&mut self, watts: f64) -> HwResult<()>;
    fn set_core_offset(&mut self, mhz: i32) -> HwResult<()>;
    fn set_mem_offset(&mut self, mhz: i32) -> HwResult<()>;
    fn lock_clocks(&mut self, min_mhz: u32, max_mhz: u32) -> HwResult<()>;
    fn unlock_clocks(&mut self) -> HwResult<()>;
    fn set_fan(&mut self, pct: i32) -> HwResult<()>;
    fn fan_auto(&mut self) -> HwResult<()>;

    /// Devuelve los errores (vacío = todo bien).
    fn reset_all(&mut self) -> Vec<String> {
        let d = self.limits().power_default_w;
        let mut errs = Vec::new();
        let mut push = |label: &str, r: HwResult<()>| {
            if let Err(e) = r {
                errs.push(format!("{label}: {e}"));
            }
        };
        push("Potencia", self.set_power_limit(d));
        push("Núcleo", self.set_core_offset(0));
        push("Memoria", self.set_mem_offset(0));
        push("Bloqueo de reloj", self.unlock_clocks());
        push("Ventilador", self.fan_auto());
        errs
    }
}

/// ¿Corremos con privilegios de Administrador / root?
pub fn is_admin() -> bool {
    #[cfg(windows)]
    {
        // SAFETY: IsUserAnAdmin no tiene precondiciones.
        unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
    }
    #[cfg(not(windows))]
    {
        // Sin dependencias extra: /proc/self/status → "Uid:\t<real>\t<efectivo>..."
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines().find(|l| l.starts_with("Uid:")).map(|l| l.split_whitespace().nth(2) == Some("0"))
            })
            .unwrap_or(false)
    }
}
