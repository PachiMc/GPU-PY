//! Lógica independiente de la interfaz: perfiles, ajustes, aplicación de perfiles, detección de hardware.

use crate::backend::GpuBackend;
use crate::error::{HwError, HwResult};
use crate::nvapi::fake::FakeNvapi;
use crate::nvapi::i2c::{self, FakeBus, PalitLed};
use crate::nvapi::illum::IllumLed;
use crate::nvapi::{LedController, parse_color};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const APP_NAME: &str = "GPU PY";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const DEFAULT_CURVE: [(i32, i32); 5] = [(30, 30), (50, 40), (65, 60), (75, 80), (85, 100)];
/// Por encima de esta temperatura el ventilador se fuerza al 100 %.
pub const SAFETY_TEMP: f64 = 88.0;

// --------------------------------------------------------------------------------------------
// Carpeta de datos (%APPDATA%\GPU PY — igual que la versión Python, así se conservan los perfiles)
// --------------------------------------------------------------------------------------------
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("GPU_PY_DATA_DIR")
        .map(|d| PathBuf::from(d).join("_base"))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default();
            PathBuf::from(home).join(".config")
        });
    let path = base.join(APP_NAME);
    let _ = std::fs::create_dir_all(&path);
    // migración desde la versión anterior
    let old = base.join("VelocityPy");
    if old.join("profiles.json").exists() && !path.join("profiles.json").exists() {
        let _ = std::fs::copy(old.join("profiles.json"), path.join("profiles.json"));
    }
    path
}

// --------------------------------------------------------------------------------------------
// Ajustes y perfiles (formato JSON compatible con la versión Python)
// --------------------------------------------------------------------------------------------
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub start_minimized: bool,
    pub startup_profile: Option<String>,
    pub tray_enabled: bool,
    /// Tras suspender/hibernar el driver restablece OC, ventiladores y LED: volver a aplicar lo último aplicado.
    pub reapply_on_resume: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { start_minimized: true, startup_profile: None, tray_enabled: true, reapply_on_resume: true }
    }
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read_to_string(data_dir().join("settings.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(data_dir().join("settings.json"), text)
    }
}

/// Estado del LED. `mode`: keep | static | breathing | off | default | cycle | strobe
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LedState {
    pub mode: String,
    pub color: String,
    pub color2: String,
    pub brightness: i32,
    pub period: i32,
    pub zones: Option<Vec<usize>>,
    pub speed: i32,
    pub reverse: bool,
}

impl Default for LedState {
    fn default() -> Self {
        Self {
            mode: "keep".into(),
            color: "#ff2d55".into(),
            color2: "#000000".into(),
            brightness: 100,
            period: 3000,
            zones: None,
            speed: 50,
            reverse: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub power_w: f64,
    pub core_off: i32,
    pub mem_off: i32,
    pub lock_on: bool,
    pub lock_mhz: i32,
    /// auto | manual | curve  (en memoria también "keep" para "no tocar los ventiladores")
    pub fan_mode: String,
    pub fan_manual: i32,
    pub curve: Vec<(i32, i32)>,
    pub led: Option<LedState>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            power_w: 220.0,
            core_off: 0,
            mem_off: 0,
            lock_on: false,
            lock_mhz: 2100,
            fan_mode: "auto".into(),
            fan_manual: 50,
            curve: DEFAULT_CURVE.to_vec(),
            led: Some(LedState::default()),
        }
    }
}

pub fn default_profile(be: &dyn GpuBackend) -> Profile {
    let l = be.limits();
    Profile { power_w: l.power_default_w, lock_mhz: l.max_core_clock, ..Profile::default() }
}

pub struct ProfileStore {
    path: PathBuf,
    pub data: BTreeMap<String, Profile>,
    corrupt: bool,
}

impl ProfileStore {
    pub fn load() -> Self {
        Self::at(data_dir().join("profiles.json"))
    }

    pub fn at(path: PathBuf) -> Self {
        let (data, corrupt) = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(d) => (d, false),
                Err(_) => (BTreeMap::new(), true),
            },
            Err(_) => (BTreeMap::new(), false),
        };
        Self { path, data, corrupt }
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        if self.corrupt {
            // No destruyas un archivo que no pudimos leer: guarda una copia antes de sobrescribirlo.
            let _ = std::fs::copy(&self.path, self.path.with_extension("json.bak"));
            self.corrupt = false;
        }
        let text = serde_json::to_string_pretty(&self.data).map_err(std::io::Error::other)?;
        std::fs::write(&self.path, text)
    }

    /// Nombres ordenados.
    pub fn names(&self) -> Vec<String> {
        self.data.keys().cloned().collect()
    }
}

// --------------------------------------------------------------------------------------------
// Aplicar ajustes
// --------------------------------------------------------------------------------------------
pub fn apply_led_state(led: &mut dyn LedController, st: &LedState) -> HwResult<()> {
    let zones = st.zones.as_deref();
    match st.mode.as_str() {
        "keep" => Ok(()),
        "off" => led.set_off(zones),
        "default" => led.restore_default(),
        mode @ ("cycle" | "strobe") => {
            if !led.supports_effects() {
                return Err(HwError::new("Este controlador no admite ese efecto."));
            }
            let (speed, reverse) = (st.speed.max(0) as u32, st.reverse);
            if mode == "cycle" {
                led.set_cycle(speed, reverse)
            } else {
                led.set_strobe(parse_color(&st.color)?, speed, reverse)
            }
        }
        "breathing" => {
            led.set_breathing(parse_color(&st.color)?, parse_color(&st.color2)?, st.period.max(0) as u32, zones)
        }
        _ => led.set_static(parse_color(&st.color)?, st.brightness, zones),
    }
}

/// `Option<Box<dyn LedController>>` → `Option<&mut dyn LedController>` (ayuda con los tiempos de vida).
pub fn led_mut(led: &mut Option<Box<dyn LedController>>) -> Option<&mut dyn LedController> {
    match led {
        Some(b) => Some(b.as_mut()),
        None => None,
    }
}

/// Aplica un perfil directamente (sin GUI). Devuelve la lista de errores.
pub fn apply_profile_to_backend(
    be: &mut dyn GpuBackend,
    p: &Profile,
    led: Option<&mut dyn LedController>,
) -> Vec<String> {
    let mut errs = Vec::new();
    let mut run = |label: &str, r: HwResult<()>, quiet: bool| {
        if let (Err(e), false) = (r, quiet) {
            errs.push(format!("{label}: {e}"));
        }
    };
    run("Límite de potencia", be.set_power_limit(p.power_w), false);
    run("Offset del núcleo", be.set_core_offset(p.core_off), false);
    run("Offset de la memoria", be.set_mem_offset(p.mem_off), false);
    if p.lock_on {
        run("Bloqueo de reloj", be.lock_clocks(210, p.lock_mhz.max(0) as u32), false);
    } else {
        run("Bloqueo de reloj", be.unlock_clocks(), true);
    }
    match p.fan_mode.as_str() {
        "manual" => run("Ventilador", be.set_fan(p.fan_manual), false),
        "auto" => run("Ventilador", be.fan_auto(), true),
        _ => {}
    }
    if let (Some(led), Some(st)) = (led, p.led.as_ref()) {
        if let Err(e) = apply_led_state(led, st) {
            errs.push(format!("LED: {e}"));
        }
    }
    errs
}

/// Interpolación lineal de la curva de ventiladores.
pub fn interpolate(curve: &[(i32, i32)], temp: f64) -> i32 {
    let mut pts = curve.to_vec();
    pts.sort_unstable();
    let (first, last) = (pts[0], pts[pts.len() - 1]);
    if temp <= first.0 as f64 {
        return first.1;
    }
    if temp >= last.0 as f64 {
        return last.1;
    }
    for w in pts.windows(2) {
        let ((t0, p0), (t1, p1)) = (w[0], w[1]);
        if t0 as f64 <= temp && temp <= t1 as f64 {
            // Python `round()` redondea los .5 al par
            let v = p0 as f64 + (p1 - p0) as f64 * (temp - t0 as f64) / (t1 - t0).max(1) as f64;
            return v.round_ties_even() as i32;
        }
    }
    last.1
}

// --------------------------------------------------------------------------------------------
// Detección de hardware y diagnósticos
// --------------------------------------------------------------------------------------------

/// Prueba primero las zonas de NVAPI y, si no hay, el controlador I2C PALIT.
pub fn make_led(demo: bool) -> (Option<Box<dyn LedController>>, Option<String>) {
    if demo {
        let led = IllumLed::new(Some(Box::new(FakeNvapi::standard())), 0).ok();
        return (led.map(|l| Box::new(l) as Box<dyn LedController>), None);
    }
    let mut problems: Vec<String> = Vec::new();
    match IllumLed::new(None, 0) {
        Ok(led) if led.num_zones() > 0 => return (Some(Box::new(led)), None),
        Ok(_) => problems.push("NVAPI: el driver no reporta zonas de iluminación.".into()),
        Err(e) => problems.push(format!("NVAPI: {e}")),
    }
    match i2c::open_real_bus() {
        Ok(bus) => {
            let led = PalitLed::new(bus);
            if led.protocol.is_some() {
                return (Some(Box::new(led)), None);
            }
            problems.push("I2C: no se detectó un controlador LED PALIT (dispositivos 0x92 / 0x10).".into());
        }
        Err(e) => problems.push(format!("I2C: {e}")),
    }
    (None, Some(problems.join(" ")))
}

pub fn i2c_scan_text(demo: bool) -> HwResult<String> {
    let bus: Box<dyn i2c::I2cBus> = if demo { Box::new(FakeBus::standard()) } else { i2c::open_real_bus()? };
    Ok(i2c::scan_report(&PalitLed::new(bus)))
}

pub fn oc_info_text(be: &mut dyn GpuBackend) -> String {
    let l = be.limits().clone();
    let mut out = vec![
        format!("GPU: {} (driver {})", be.name(), be.driver()),
        format!(
            "Overclock de reloj: {} (vía {})",
            if l.oc_supported { "SÍ" } else { "NO" },
            be.oc_source().unwrap_or("ninguna")
        ),
    ];
    if l.oc_supported {
        out.push(format!(
            "  Núcleo : offset actual {:+} MHz, rango {:+} … {:+}",
            be.core_offset(),
            l.core_off_min,
            l.core_off_max
        ));
        out.push(format!(
            "  Memoria: offset actual {:+} MHz, rango {:+} … {:+}",
            be.mem_offset(),
            l.mem_off_min,
            l.mem_off_max
        ));
    } else if let Some(e) = be.oc_error() {
        out.push(format!("  Motivo: {e}"));
    }
    out.push(format!(
        "Potencia: {:.0} … {:.0} W (por defecto {:.0} W)",
        l.power_min_w, l.power_max_w, l.power_default_w
    ));
    out.join("\n")
}
