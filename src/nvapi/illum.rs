//! LED/RGB por las zonas de iluminación de NVAPI.
//!
//! VelocityX 1.4 usa `NvAPI_GPU_ClientIlluminationZonesGetControl` (0x3DBF5764) y `...SetControl`
//! (0x197D065E). Estructura (deducida de VelocityX.exe), versión 0x1194C = 6476 bytes:
//!
//! ```text
//! IllumParams (0x194C bytes)
//!   +0x00 u32 version = 0x1194C      +0x04 u32 flags (bit0 = bDefault)    +0x08 u32 num_zones
//!   +0x0C u8[64] reservado           +0x4C ZoneControl[32]
//! ZoneControl (200 bytes)
//!   +0x00 u32 zone_type  1 = RGB, 3 = RGBW, 4 = un color (solo brillo)
//!   +0x04 u32 ctrl_mode  0 = manual, 1 = lineal por tramos (efectos)
//!   +0x08 datos según modo (192 bytes)
//! ```

use super::raw::{GpuHandle, NvapiLib};
use super::{LedController, Rgb};
use crate::error::{HwError, HwResult};

pub const MAX_ZONES: usize = 32;
pub const PARAMS_VERSION: u32 = 0x1194C;
pub const ZONE_RGB: u32 = 1;
pub const ZONE_RGBW: u32 = 3;
pub const ZONE_SINGLE: u32 = 4;
pub const MODE_MANUAL: u32 = 0;
pub const MODE_PIECEWISE: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ZoneControl {
    pub zone_type: u32,
    pub ctrl_mode: u32,
    pub data: [u8; 192],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IllumParams {
    pub version: u32,
    pub flags: u32,
    pub num_zones: u32,
    pub rsvd: [u8; 64],
    pub zones: [ZoneControl; MAX_ZONES],
}

// Mismas comprobaciones que los `assert` del código Python.
const _: () = assert!(size_of::<ZoneControl>() == 200);
const _: () = assert!(size_of::<IllumParams>() == (PARAMS_VERSION & 0xFFFF) as usize);
const _: () = assert!(size_of::<IllumParams>() == 0x194C);

impl IllumParams {
    pub fn new(default: bool) -> Self {
        Self {
            version: PARAMS_VERSION,
            flags: u32::from(default),
            num_zones: 0,
            rsvd: [0; 64],
            zones: [ZoneControl { zone_type: 0, ctrl_mode: 0, data: [0; 192] }; MAX_ZONES],
        }
    }
}

pub fn zone_name(t: u32) -> String {
    match t {
        ZONE_RGB => "RGB".into(),
        ZONE_RGBW => "RGBW".into(),
        ZONE_SINGLE => "Un color (brillo)".into(),
        other => format!("tipo {other}"),
    }
}

/// Lo que necesita `IllumLed` del driver (real o simulado).
pub trait IllumApi {
    fn gpus(&self) -> HwResult<Vec<(GpuHandle, String)>>;
    fn get_control(&self, handle: GpuHandle, params: &mut IllumParams) -> HwResult<()>;
    fn set_control(&self, handle: GpuHandle, params: &mut IllumParams) -> HwResult<()>;
}

pub struct RealIllum(NvapiLib);

impl RealIllum {
    pub fn new() -> HwResult<Self> {
        Ok(Self(NvapiLib::new()?))
    }
}

impl IllumApi for RealIllum {
    fn gpus(&self) -> HwResult<Vec<(GpuHandle, String)>> {
        self.0.gpus()
    }
    fn get_control(&self, h: GpuHandle, p: &mut IllumParams) -> HwResult<()> {
        self.0.zones_get(h, p)
    }
    fn set_control(&self, h: GpuHandle, p: &mut IllumParams) -> HwResult<()> {
        self.0.zones_set(h, p)
    }
}

pub struct IllumLed {
    api: Box<dyn IllumApi>,
    handle: GpuHandle,
    gpu_name: String,
    num_zones: usize,
    zone_types: Vec<u32>,
    /// `true` = comportamiento idéntico al de VelocityX/versión Python (sin limpiar ni verificar).
    /// Solo lo usan las pruebas diferenciales contra el código original.
    pub legacy_behavior: bool,
}

impl IllumLed {
    pub fn new(api: Option<Box<dyn IllumApi>>, gpu_index: usize) -> HwResult<Self> {
        let api: Box<dyn IllumApi> = match api {
            Some(a) => a,
            None => Box::new(RealIllum::new()?),
        };
        let gpus = api.gpus()?;
        if gpus.is_empty() {
            return Err(HwError::new("NVAPI no encontró ninguna GPU NVIDIA."));
        }
        let (handle, gpu_name) = gpus[gpu_index.min(gpus.len() - 1)].clone();
        let mut led = Self { api, handle, gpu_name, num_zones: 0, zone_types: vec![], legacy_behavior: false };
        led.refresh()?;
        Ok(led)
    }

    pub fn refresh(&mut self) -> HwResult<()> {
        let p = self.read(false)?;
        self.num_zones = (p.num_zones as usize).min(MAX_ZONES);
        self.zone_types = (0..self.num_zones).map(|i| p.zones[i].zone_type).collect();
        Ok(())
    }

    fn read(&self, default: bool) -> HwResult<IllumParams> {
        let mut p = IllumParams::new(default);
        self.api.get_control(self.handle, &mut p)?;
        Ok(p)
    }

    fn write(&self, p: &mut IllumParams) -> HwResult<()> {
        p.version = PARAMS_VERSION;
        p.flags &= !1; // VelocityX siempre limpia bDefault antes de escribir
        self.api.set_control(self.handle, p)
    }

    /// Escribe y comprueba leyendo de vuelta. Si el driver no se quedó con el cambio (el fallo que obligaba
    /// a pulsar «Restaurar» y luego «Aplicar»), hace ese mismo ciclo automáticamente y reintenta.
    fn write_verified(&self, p: &mut IllumParams, targets: &[usize]) -> HwResult<()> {
        self.write(p)?;
        if self.legacy_behavior || self.applied(p, targets) {
            return Ok(());
        }
        let mut fresh = self.read(true)?; // valores de fábrica = «Restaurar»
        self.write(&mut fresh)?;
        self.write(p)?;
        Ok(()) // si aun así el driver normaliza los valores, no lo tratamos como error
    }

    fn applied(&self, wanted: &IllumParams, targets: &[usize]) -> bool {
        let Ok(now) = self.read(false) else { return true }; // sin lectura no se puede verificar
        targets.iter().all(|&i| {
            let (a, b) = (&now.zones[i], &wanted.zones[i]);
            a.ctrl_mode == b.ctrl_mode && a.data[..24] == b.data[..24]
        })
    }

    fn targets(p: &IllumParams, zones: Option<&[usize]>) -> HwResult<Vec<usize>> {
        if p.num_zones == 0 {
            return Err(HwError::new("Tu GPU no reporta zonas de iluminación controlables por NVAPI."));
        }
        let n = (p.num_zones as usize).min(MAX_ZONES);
        Ok(match zones {
            None => (0..n).collect(),
            Some(z) => z.iter().copied().filter(|&i| i < n).collect(),
        })
    }
}

impl LedController for IllumLed {
    fn describe(&self) -> String {
        if self.num_zones == 0 {
            return format!("{}: el driver no reporta zonas de iluminación.", self.gpu_name);
        }
        let zs = self
            .zone_types
            .iter()
            .enumerate()
            .map(|(i, t)| format!("#{i}: {}", zone_name(*t)))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}: {} zona(s) → {zs}", self.gpu_name, self.num_zones)
    }

    fn num_zones(&self) -> usize {
        self.num_zones
    }

    /// Color fijo (modo manual), igual que la función de color de VelocityX.
    fn set_static(&mut self, (r, g, b): Rgb, brightness: i32, zones: Option<&[usize]>) -> HwResult<()> {
        let br = brightness.clamp(0, 100) as u8;
        let mut p = self.read(false)?;
        let targets = Self::targets(&p, zones)?;
        for &i in &targets {
            let z = &mut p.zones[i];
            if !self.legacy_behavior {
                z.data = [0; 192]; // no arrastrar parámetros de una respiración anterior
            }
            z.ctrl_mode = MODE_MANUAL;
            match z.zone_type {
                ZONE_RGB => z.data[0..4].copy_from_slice(&[r, g, b, br]),
                ZONE_RGBW => z.data[0..5].copy_from_slice(&[r, g, b, 0, br]),
                ZONE_SINGLE => z.data[0] = br & 0x7F,
                _ => {}
            }
        }
        self.write_verified(&mut p, &targets)
    }

    fn set_off(&mut self, zones: Option<&[usize]>) -> HwResult<()> {
        self.set_static((0, 0, 0), 0, zones)
    }

    /// Respiración: va del color A al color B y vuelve (modo lineal por tramos).
    fn set_breathing(&mut self, a: Rgb, b: Rgb, period_ms: u32, zones: Option<&[usize]>) -> HwResult<()> {
        let half = ((period_ms / 2).clamp(100, 30000)) as u16;
        let mut p = self.read(false)?;
        let targets = Self::targets(&p, zones)?;
        for &i in &targets {
            let z = &mut p.zones[i];
            if z.zone_type != ZONE_RGB {
                continue; // los efectos solo están documentados para zonas RGB
            }
            z.data = [0; 192];
            z.ctrl_mode = MODE_PIECEWISE;
            let lvl = |c: Rgb| if c != (0, 0, 0) { 100 } else { 0 };
            z.data[0..4].copy_from_slice(&[a.0, a.1, a.2, lvl(a)]);
            z.data[4..8].copy_from_slice(&[b.0, b.1, b.2, lvl(b)]);
            // "<IBxHHHHHH" en data[8..]: tipo de ciclo=2, grupos=1, pad, 6 × u16
            let d = &mut z.data[8..];
            d[0..4].copy_from_slice(&2u32.to_le_bytes());
            d[4] = 1;
            d[5] = 0;
            for (k, v) in [half, half, 100, 100, 0, 0].into_iter().enumerate() {
                d[6 + k * 2..8 + k * 2].copy_from_slice(&v.to_le_bytes());
            }
        }
        self.write_verified(&mut p, &targets)
    }

    /// Pide al driver los valores de fábrica (bDefault=1) y los aplica.
    fn restore_default(&mut self) -> HwResult<()> {
        let mut p = self.read(true)?;
        self.write(&mut p)
    }
}
