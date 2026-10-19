//! Backend real: NVML (`nvml-wrapper`) y, cuando NVML no expone offsets de reloj, NVAPI.

use super::*;
use crate::nvapi::oc::{self, DOM_CORE, DOM_MEM, OcNvapi};
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use nvml_wrapper::enums::device::GpuLockedClocksSetting;
use nvml_wrapper::error::NvmlError;
use nvml_wrapper::{Device, Nvml};
use std::ffi::c_void;

fn explain(e: &NvmlError) -> String {
    match e {
        NvmlError::NoPermission => "Permiso denegado: ejecuta el programa como Administrador.".into(),
        NvmlError::NotSupported => "Tu GPU o tu driver no soportan esta función.".into(),
        NvmlError::InvalidArg => "Valor fuera del rango permitido por el driver.".into(),
        NvmlError::GpuLost => "La GPU dejó de responder (¿driver reiniciándose?).".into(),
        NvmlError::FailedToLoadSymbol(_) => {
            "Tu driver de NVIDIA es demasiado antiguo para esta función. Actualiza el driver.".into()
        }
        other => format!("Error NVML: {other}"),
    }
}

fn nv<T>(r: Result<T, NvmlError>) -> HwResult<T> {
    r.map_err(|e| HwError::new(explain(&e)))
}

/// `nvmlDevice{Get}{Gpc,Mem}ClkMinMaxVfOffset` no está en `nvml-wrapper`: se llama directamente.
fn vf_range(device: &Device, symbol: &[u8]) -> Option<(i32, i32)> {
    type F = unsafe extern "C" fn(*mut c_void, *mut i32, *mut i32) -> u32;
    let name = if cfg!(windows) { "nvml.dll" } else { "libnvidia-ml.so.1" };
    // SAFETY: NVML ya está cargada por nvml-wrapper; la firma coincide con nvml.h.
    unsafe {
        let lib = libloading::Library::new(name).ok()?;
        let f = lib.get::<F>(symbol).ok()?;
        let (mut lo, mut hi) = (0i32, 0i32);
        let ret = f(device.handle().cast(), &mut lo, &mut hi);
        (ret == 0).then_some((lo, hi))
    }
}

thread_local! {
    static READER_NVML: std::cell::RefCell<Option<Nvml>> = const { std::cell::RefCell::new(None) };
}

fn read_telemetry(nvml: &Nvml, index: u32) -> Telemetry {
    let mut t = Telemetry::default();
    let Ok(d) = nvml.device_by_index(index) else { return t };
    t.temp = d.temperature(TemperatureSensor::Gpu).unwrap_or(0) as f64;
    t.core_clock = d.clock_info(Clock::Graphics).unwrap_or(0);
    t.mem_clock = d.clock_info(Clock::Memory).unwrap_or(0);
    t.fan_pct = d.fan_speed(0).unwrap_or(0);
    t.power_w = d.power_usage().unwrap_or(0) as f64 / 1000.0;
    t.util_gpu = d.utilization_rates().map(|u| u.gpu).unwrap_or(0);
    if let Ok(m) = d.memory_info() {
        t.vram_used_mb = m.used / (1 << 20);
        t.vram_total_mb = m.total / (1 << 20);
    }
    t.throttle = throttle_text(d.current_throttle_reasons().map(|r| r.bits()).unwrap_or(0));
    t
}

pub struct NvmlBackend {
    nvml: Nvml,
    index: u32,
    name: String,
    driver: String,
    limits: Limits,
    oc: Option<OcNvapi>,
    oc_source: Option<&'static str>,
    oc_error: Option<String>,
}

impl NvmlBackend {
    pub fn new(index: u32) -> HwResult<Self> {
        let init_err = |e: &NvmlError| {
            HwError::new(format!("No se pudo iniciar NVML. ¿Está instalado el driver de NVIDIA?\n{}", explain(e)))
        };
        let nvml = Nvml::init().map_err(|e| init_err(&e))?;
        let name = {
            let d = nvml.device_by_index(index).map_err(|e| init_err(&e))?;
            d.name().map_err(|e| init_err(&e))?
        };
        let driver = nvml.sys_driver_version().unwrap_or_else(|_| "?".into());
        let mut be = Self { nvml, index, name, driver, limits: Limits::default(), oc: None, oc_source: None, oc_error: None };
        be.limits = be.probe_limits();
        be.oc_source = be.limits.oc_supported.then_some("nvml");
        be.probe_nvapi_oc();
        Ok(be)
    }

    fn dev(&self) -> HwResult<Device<'_>> {
        nv(self.nvml.device_by_index(self.index))
    }

    fn nvapi(&mut self) -> HwResult<&OcNvapi> {
        if self.oc.is_none() {
            self.oc = Some(OcNvapi::new(oc::open_real_api()?));
        }
        Ok(self.oc.as_ref().unwrap())
    }

    /// Si NVML no expone offsets de reloj, intenta con NVAPI.
    fn probe_nvapi_oc(&mut self) {
        if self.limits.oc_supported {
            return;
        }
        let ranges = (|| -> HwResult<_> {
            let oc = self.nvapi()?;
            Ok((oc.get_range(DOM_CORE)?, oc.get_range(DOM_MEM)?))
        })();
        let (core, mem) = match ranges {
            Ok(r) => r,
            Err(e) => {
                self.oc_error = Some(e.0);
                return;
            }
        };
        if core.is_none() && mem.is_none() {
            self.oc_error = Some("El driver no lista relojes editables en el P-state 0.".into());
            return;
        }
        let l = &mut self.limits;
        self.oc_source = Some("nvapi");
        l.oc_supported = true;
        (l.core_off_min, l.core_off_max) = core
            .map(|(lo, hi)| (lo.max(SOFT_CORE_OFFSET.0), hi.min(SOFT_CORE_OFFSET.1)))
            .unwrap_or((0, 0));
        (l.mem_off_min, l.mem_off_max) = mem
            .map(|(lo, hi)| (lo.max(SOFT_MEM_OFFSET.0), hi.min(SOFT_MEM_OFFSET.1)))
            .unwrap_or((0, 0));
    }

    fn probe_limits(&self) -> Limits {
        let mut l = Limits::default();
        let Ok(d) = self.dev() else { return l };
        if let Ok(pc) = d.power_management_limit_constraints() {
            l.power_min_w = pc.min_limit as f64 / 1000.0;
            l.power_max_w = pc.max_limit as f64 / 1000.0;
        }
        l.power_default_w = d.power_management_limit_default().map(|v| v as f64 / 1000.0).unwrap_or(l.power_max_w);
        if d.gpc_clock_vf_offset().is_ok() {
            l.oc_supported = true;
            if let Some((lo, hi)) = vf_range(&d, b"nvmlDeviceGetGpcClkMinMaxVfOffset\0") {
                l.core_off_min = lo.max(SOFT_CORE_OFFSET.0);
                l.core_off_max = hi.min(SOFT_CORE_OFFSET.1);
            }
            if let Some((lo, hi)) = vf_range(&d, b"nvmlDeviceGetMemClkMinMaxVfOffset\0") {
                l.mem_off_min = lo.max(SOFT_MEM_OFFSET.0);
                l.mem_off_max = hi.min(SOFT_MEM_OFFSET.1);
            }
        }
        l.max_core_clock = d.max_clock_info(Clock::Graphics).map(|v| v as i32).unwrap_or(l.max_core_clock);
        l.max_mem_clock = d.max_clock_info(Clock::Memory).map(|v| v as i32).unwrap_or(l.max_mem_clock);
        l.num_fans = d.num_fans().unwrap_or(0);
        l
    }

    fn fan_indices(&self) -> std::ops::Range<u32> {
        0..self.limits.num_fans.max(1)
    }
}

impl GpuBackend for NvmlBackend {
    fn name(&self) -> &str {
        &self.name
    }
    fn driver(&self) -> &str {
        &self.driver
    }
    fn limits(&self) -> &Limits {
        &self.limits
    }
    fn oc_source(&self) -> Option<&str> {
        self.oc_source
    }
    fn oc_error(&self) -> Option<&str> {
        self.oc_error.as_deref()
    }

    fn read(&mut self) -> Telemetry {
        read_telemetry(&self.nvml, self.index)
    }

    fn telemetry_source(&self) -> Option<Box<dyn FnMut() -> Telemetry + Send>> {
        let index = self.index;
        Some(Box::new(move || {
            // Cada hilo lector abre su propia instancia de NVML (NVML cuenta las inicializaciones).
            READER_NVML.with(|cell| {
                let mut slot = cell.borrow_mut();
                if slot.is_none() {
                    *slot = Nvml::init().ok();
                }
                match slot.as_ref() {
                    Some(n) if n.device_by_index(index).is_ok() => read_telemetry(n, index),
                    Some(_) => {
                        // Tras suspender/reanudar o un reinicio del driver el manejador puede quedar inválido:
                        // se descarta y se vuelve a inicializar NVML en la siguiente lectura.
                        *slot = None;
                        Telemetry::default()
                    }
                    None => Telemetry::default(),
                }
            })
        }))
    }

    fn core_offset(&mut self) -> i32 {
        if self.oc_source == Some("nvapi") {
            return self.oc.as_ref().and_then(|o| o.get_offset(DOM_CORE).ok().flatten()).unwrap_or(0);
        }
        self.dev().ok().and_then(|d| d.gpc_clock_vf_offset().ok()).unwrap_or(0)
    }

    fn mem_offset(&mut self) -> i32 {
        if self.oc_source == Some("nvapi") {
            return self.oc.as_ref().and_then(|o| o.get_offset(DOM_MEM).ok().flatten()).unwrap_or(0);
        }
        self.dev().ok().and_then(|d| d.mem_clock_vf_offset().ok()).unwrap_or(0)
    }

    fn set_power_limit(&mut self, watts: f64) -> HwResult<()> {
        let l = &self.limits;
        let watts = l.power_min_w.max(l.power_max_w.min(watts));
        let (default_w, mw) = (l.power_default_w, (watts * 1000.0).round_ties_even() as u32);
        let e_nvml = match self.dev().and_then(|mut d| nv(d.set_power_management_limit(mw))) {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        // plan B: NVAPI, como VelocityX
        let pct = watts / default_w.max(1.0) * 100.0;
        match self.nvapi().and_then(|o| o.set_power_pct(pct)) {
            Ok(_) => Ok(()),
            Err(e_api) => Err(HwError::new(format!("{e_nvml} | NVAPI: {e_api}"))),
        }
    }

    fn set_core_offset(&mut self, mhz: i32) -> HwResult<()> {
        let l = &self.limits;
        let v = l.core_off_min.max(l.core_off_max.min(mhz));
        if self.oc_source == Some("nvapi") {
            if (l.core_off_min, l.core_off_max) != (0, 0) {
                self.oc.as_ref().expect("nvapi oc").set_offset(DOM_CORE, v)?;
            }
            return Ok(());
        }
        nv(self.dev()?.set_gpc_clock_vf_offset(v))
    }

    fn set_mem_offset(&mut self, mhz: i32) -> HwResult<()> {
        let l = &self.limits;
        let v = l.mem_off_min.max(l.mem_off_max.min(mhz));
        if self.oc_source == Some("nvapi") {
            if (l.mem_off_min, l.mem_off_max) != (0, 0) {
                self.oc.as_ref().expect("nvapi oc").set_offset(DOM_MEM, v)?;
            }
            return Ok(());
        }
        nv(self.dev()?.set_mem_clock_vf_offset(v))
    }

    fn lock_clocks(&mut self, min_mhz: u32, max_mhz: u32) -> HwResult<()> {
        let mut d = self.dev()?;
        nv(d.set_gpu_locked_clocks(GpuLockedClocksSetting::Numeric { min_clock_mhz: min_mhz, max_clock_mhz: max_mhz }))
    }

    fn unlock_clocks(&mut self) -> HwResult<()> {
        nv(self.dev()?.reset_gpu_locked_clocks())
    }

    fn set_fan(&mut self, pct: i32) -> HwResult<()> {
        let pct = pct.clamp(MIN_FAN_PCT, 100) as u32;
        let idx = self.fan_indices();
        let mut d = self.dev()?;
        for i in idx {
            nv(d.set_fan_speed(i, pct))?;
        }
        Ok(())
    }

    fn fan_auto(&mut self) -> HwResult<()> {
        let idx = self.fan_indices();
        let mut d = self.dev()?;
        for i in idx {
            nv(d.set_default_fan_speed(i))?;
        }
        Ok(())
    }
}
