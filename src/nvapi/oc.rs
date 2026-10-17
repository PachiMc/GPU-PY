//! Overclock de reloj y límite de potencia vía NVAPI (la misma vía que VelocityX).
//!
//! NVML no expone offsets de reloj en muchos drivers de Windows, pero NVAPI sí.
//! Ids de `nvapi_QueryInterface` (confirmados dentro de VelocityX.exe):
//!
//! | función                                  | id         |
//! |------------------------------------------|------------|
//! | NvAPI_GPU_GetPstates20                   | 0x6FF81213 |
//! | NvAPI_GPU_SetPstates20                   | 0x0F4DAE6B |
//! | NvAPI_GPU_ClientPowerPoliciesGetInfo     | 0x34206D86 |
//! | NvAPI_GPU_ClientPowerPoliciesGetStatus   | 0x70916171 |
//! | NvAPI_GPU_ClientPowerPoliciesSetStatus   | 0xAD95F5ED |
//!
//! Pstates20 (deducido del código de VelocityX): lectura con versión 0x31CF8 (v3, 0x1CF8 bytes) o,
//! si el driver responde -9, 0x11C94 (v1, 0x1C94 bytes).
//!
//! ```text
//! +0x00 version | +0x04 flags (bit0 = editable) | +0x08 numPstates | +0x0C numClocks | +0x10 numBaseVoltages
//! +0x14 pstates[16] de 0x1C8 bytes: +0x00 pstateId | +0x04 flags | +0x08 clocks[8] de 0x2C bytes:
//!     +0x00 domainId (0 = núcleo, 4 = memoria) | +0x04 tipo | +0x08 flags (bit0 = editable)
//!     +0x0C delta kHz (s32) | +0x10 delta mínimo kHz | +0x14 delta máximo kHz
//! escritura (v1): numPstates=1, numClocks=1, pstate[0].id=0, clock[0].domainId=dominio, delta = MHz*1000
//! ```
//!
//! Power policies (porcentaje × 1000): Info v2 0x208C8 (0x8C8 bytes) o v1 0x100B8 (0xB8);
//! Status v2 0x20558 (0x558 bytes).

#[cfg(windows)]
use super::raw::NvapiLib;
use super::raw::STATUS_INCOMPATIBLE_VERSION;
use crate::error::{HwError, HwResult};

pub const ID_PSTATES20_GET: u32 = 0x6FF8_1213;
pub const ID_PSTATES20_SET: u32 = 0x0F4D_AE6B;
pub const ID_PWR_INFO: u32 = 0x3420_6D86;
pub const ID_PWR_GET: u32 = 0x7091_6171;
pub const ID_PWR_SET: u32 = 0xAD95_F5ED;

pub const DOM_CORE: u32 = 0;
pub const DOM_MEM: u32 = 4;

pub const PS_V3: u32 = 0x31CF8;
pub const PS_V3_SIZE: usize = 0x1CF8;
pub const PS_V1: u32 = 0x11C94;
pub const PS_V1_SIZE: usize = 0x1C94;
pub const PSTATE_STRIDE: usize = 0x1C8;
pub const CLOCK_STRIDE: usize = 0x2C;
pub const PSTATE_BASE: usize = 0x14;

pub const PWR_INFO_V2: u32 = 0x208C8;
pub const PWR_INFO_V2_SIZE: usize = 0x8C8;
pub const PWR_INFO_V2_STRIDE: usize = 0x230;
pub const PWR_INFO_V1: u32 = 0x100B8;
pub const PWR_INFO_V1_SIZE: usize = 0xB8;
pub const PWR_INFO_V1_STRIDE: usize = 0x2C;
pub const PWR_STATUS_V2: u32 = 0x20558;
pub const PWR_STATUS_SIZE: usize = 0x558;
pub const PWR_STATUS_STRIDE: usize = 0x154;

// --- ayudantes de buffer (little-endian, como struct.pack("<I")) ------------------------------
pub fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}
pub fn s32_at(b: &[u8], off: usize) -> i32 {
    i32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}
pub fn put_u32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
pub fn put_s32(b: &mut [u8], off: usize, v: i32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
/// Buffer relleno de ceros con la versión en los primeros 4 bytes.
pub fn new_buf(size: usize, version: u32) -> Vec<u8> {
    let mut b = vec![0u8; size];
    put_u32(&mut b, 0, version);
    b
}

/// Acceso al driver (real o simulado). Las funciones de potencia pueden no existir en el driver.
pub trait OcApi {
    fn ps_get(&self, buf: &mut [u8]) -> i32;
    fn ps_set(&self, buf: &mut [u8]) -> i32;
    fn pw_info(&self, buf: &mut [u8]) -> HwResult<i32>;
    fn pw_get(&self, buf: &mut [u8]) -> HwResult<i32>;
    fn pw_set(&self, buf: &mut [u8]) -> HwResult<i32>;
    fn check(&self, status: i32, what: &str) -> HwResult<()>;
}

// --------------------------------------------------------------------------------------------
// Acceso real a nvapi64.dll (solo Windows)
// --------------------------------------------------------------------------------------------
#[cfg(windows)]
pub struct NvapiOcApi {
    lib: NvapiLib,
    handle: usize,
    ps_get: super::raw::FnBuf,
    ps_set: super::raw::FnBuf,
    pw_info: Option<super::raw::FnBuf>,
    pw_get: Option<super::raw::FnBuf>,
    pw_set: Option<super::raw::FnBuf>,
}

#[cfg(windows)]
impl NvapiOcApi {
    pub fn new() -> HwResult<Self> {
        let lib = NvapiLib::new()?;
        let gpus = lib.gpus()?;
        let handle = gpus.first().ok_or_else(|| HwError::new("NVAPI no encontró ninguna GPU NVIDIA."))?.0;
        Ok(Self {
            ps_get: lib.function(ID_PSTATES20_GET, false)?.unwrap(),
            ps_set: lib.function(ID_PSTATES20_SET, false)?.unwrap(),
            pw_info: lib.function(ID_PWR_INFO, true)?,
            pw_get: lib.function(ID_PWR_GET, true)?,
            pw_set: lib.function(ID_PWR_SET, true)?,
            handle,
            lib,
        })
    }

    fn pw(&self, f: Option<super::raw::FnBuf>, buf: &mut [u8]) -> HwResult<i32> {
        let f = f.ok_or_else(|| HwError::new("Este driver no expone el control de potencia de NVAPI."))?;
        // SAFETY: `buf` tiene el tamaño de la estructura versionada que espera el driver.
        Ok(unsafe { f(self.handle as *mut _, buf.as_mut_ptr().cast()) })
    }
}

#[cfg(windows)]
impl OcApi for NvapiOcApi {
    fn ps_get(&self, buf: &mut [u8]) -> i32 {
        // SAFETY: igual que `pw`.
        unsafe { (self.ps_get)(self.handle as *mut _, buf.as_mut_ptr().cast()) }
    }
    fn ps_set(&self, buf: &mut [u8]) -> i32 {
        // SAFETY: igual que `pw`.
        unsafe { (self.ps_set)(self.handle as *mut _, buf.as_mut_ptr().cast()) }
    }
    fn pw_info(&self, buf: &mut [u8]) -> HwResult<i32> {
        self.pw(self.pw_info, buf)
    }
    fn pw_get(&self, buf: &mut [u8]) -> HwResult<i32> {
        self.pw(self.pw_get, buf)
    }
    fn pw_set(&self, buf: &mut [u8]) -> HwResult<i32> {
        self.pw(self.pw_set, buf)
    }
    fn check(&self, status: i32, what: &str) -> HwResult<()> {
        self.lib.check(status, what)
    }
}

/// Abre la API real de overclock (error limpio fuera de Windows).
pub fn open_real_api() -> HwResult<Box<dyn OcApi>> {
    #[cfg(windows)]
    {
        Ok(Box::new(NvapiOcApi::new()?))
    }
    #[cfg(not(windows))]
    {
        Err(HwError::new("NVAPI solo está disponible en Windows."))
    }
}

// --------------------------------------------------------------------------------------------
// Lógica de alto nivel
// --------------------------------------------------------------------------------------------
pub struct OcNvapi {
    api: Box<dyn OcApi>,
}

impl OcNvapi {
    pub fn new(api: Box<dyn OcApi>) -> Self {
        Self { api }
    }

    // -- Pstates20 ---------------------------------------------------------------------------
    fn read_pstates(&self) -> HwResult<Vec<u8>> {
        for (ver, size) in [(PS_V3, PS_V3_SIZE), (PS_V1, PS_V1_SIZE)] {
            let mut buf = new_buf(size, ver);
            let st = self.api.ps_get(&mut buf);
            if st == STATUS_INCOMPATIBLE_VERSION {
                continue;
            }
            self.api.check(st, "NvAPI_GPU_GetPstates20")?;
            return Ok(buf);
        }
        Err(HwError::new("El driver rechazó las versiones de Pstates20 conocidas."))
    }

    /// Posición del reloj `domain` dentro del P-state 0 (como hace VelocityX).
    fn clock_offset(buf: &[u8], domain: u32) -> HwResult<Option<usize>> {
        if u32_at(buf, 4) & 1 == 0 {
            return Err(HwError::new("El driver marca los P-states como no editables (overclock bloqueado)."));
        }
        let n_p = (u32_at(buf, 8) as usize).min(16);
        let n_c = (u32_at(buf, 0xC) as usize).min(8);
        for p in 0..n_p {
            let po = PSTATE_BASE + p * PSTATE_STRIDE;
            if u32_at(buf, po) != 0 {
                continue;
            }
            for c in 0..n_c {
                let co = po + 8 + c * CLOCK_STRIDE;
                if u32_at(buf, co) == domain {
                    return Ok(Some(co));
                }
            }
        }
        Ok(None)
    }

    /// Offset actual en MHz, o `None` si el dominio no existe.
    pub fn get_offset(&self, domain: u32) -> HwResult<Option<i32>> {
        let buf = self.read_pstates()?;
        Ok(Self::clock_offset(&buf, domain)?.map(|co| s32_at(&buf, co + 0xC) / 1000))
    }

    /// (mínimo, máximo) permitido en MHz, o `None` si el dominio no existe.
    pub fn get_range(&self, domain: u32) -> HwResult<Option<(i32, i32)>> {
        let buf = self.read_pstates()?;
        Ok(Self::clock_offset(&buf, domain)?.map(|co| (s32_at(&buf, co + 0x10) / 1000, s32_at(&buf, co + 0x14) / 1000)))
    }

    pub fn set_offset(&self, domain: u32, mhz: i32) -> HwResult<i32> {
        let mut buf = new_buf(PS_V3_SIZE, PS_V1); // VelocityX usa v1 y reserva 0x1CF8 bytes
        put_u32(&mut buf, 0x08, 1); // numPstates
        put_u32(&mut buf, 0x0C, 1); // numClocks
        put_u32(&mut buf, 0x10, 0); // numBaseVoltages
        put_u32(&mut buf, PSTATE_BASE, 0); // pstate 0
        put_u32(&mut buf, 0x1C, domain); // clock[0].domainId
        put_s32(&mut buf, 0x28, mhz * 1000); // clock[0].delta (kHz)
        let st = self.api.ps_set(&mut buf);
        self.api.check(st, "NvAPI_GPU_SetPstates20")?;
        let got = self.get_offset(domain)?;
        if let Some(g) = got {
            if g != mhz {
                return Err(HwError::new(format!(
                    "El driver aceptó el cambio pero quedó en {g} MHz (pedido: {mhz} MHz)."
                )));
            }
        }
        Ok(got.unwrap_or(mhz))
    }

    // -- Potencia ----------------------------------------------------------------------------
    fn power_info(&self) -> HwResult<(Vec<u8>, usize)> {
        for (ver, size, stride) in [
            (PWR_INFO_V2, PWR_INFO_V2_SIZE, PWR_INFO_V2_STRIDE),
            (PWR_INFO_V1, PWR_INFO_V1_SIZE, PWR_INFO_V1_STRIDE),
        ] {
            let mut buf = new_buf(size, ver);
            let st = self.api.pw_info(&mut buf)?;
            if st == STATUS_INCOMPATIBLE_VERSION {
                continue;
            }
            self.api.check(st, "NvAPI_GPU_ClientPowerPoliciesGetInfo")?;
            if buf[4] == 0 {
                return Err(HwError::new("El driver no reporta políticas de potencia."));
            }
            return Ok((buf, stride));
        }
        Err(HwError::new("El driver rechazó las versiones de power policies conocidas."))
    }

    /// (mínimo, por defecto, máximo) en % de la potencia base, política 0.
    pub fn power_range(&self) -> HwResult<(u32, u32, u32)> {
        let (buf, stride) = self.power_info()?;
        let n = buf[5] as usize;
        for i in 0..n.min(4) {
            let e = 8 + i * stride;
            if u32_at(&buf, e) == 0 && u32_at(&buf, e + 8) == 0 {
                let mut vals = [0x10, 0x1C, 0x28].map(|o| u32_at(&buf, e + o) / 1000);
                vals.sort_unstable();
                return Ok((vals[0], vals[1], vals[2]));
            }
        }
        Err(HwError::new("No se encontró la política de potencia total de la GPU."))
    }

    fn status_buf(ids: &[u32]) -> Vec<u8> {
        let mut buf = new_buf(PWR_STATUS_SIZE, PWR_STATUS_V2);
        buf[4] = ids.len() as u8;
        for (i, pid) in ids.iter().enumerate() {
            put_u32(&mut buf, 8 + i * PWR_STATUS_STRIDE, *pid);
        }
        buf
    }

    pub fn get_power_pct(&self) -> HwResult<u32> {
        let (info, stride) = self.power_info()?;
        let n = (info[5] as usize).min(4);
        let ids: Vec<u32> = (0..n).map(|i| u32_at(&info, 8 + i * stride)).collect();
        let mut buf = Self::status_buf(&ids);
        let st = self.api.pw_get(&mut buf)?;
        self.api.check(st, "NvAPI_GPU_ClientPowerPoliciesGetStatus")?;
        for i in 0..n {
            let e = 8 + i * PWR_STATUS_STRIDE;
            if u32_at(&buf, e) == 0 {
                return Ok(u32_at(&buf, e + 0xC) / 1000);
            }
        }
        Err(HwError::new("No se pudo leer la potencia actual."))
    }

    pub fn set_power_pct(&self, pct: f64) -> HwResult<u32> {
        let (lo, _, hi) = self.power_range()?;
        // Python: int(round(max(lo, min(hi, pct)))) → redondeo "al par" en los .5
        let pct = (lo as f64).max((hi as f64).min(pct)).round_ties_even() as u32;
        let mut buf = new_buf(PWR_STATUS_SIZE, PWR_STATUS_V2);
        buf[4] = 1; // una entrada
        put_u32(&mut buf, 8, 0); // policyId 0
        let flags = u32_at(&buf, 0x10) & !1;
        put_u32(&mut buf, 0x10, flags); // flags bit0 = 0
        put_u32(&mut buf, 0x14, pct * 1000); // potencia
        let st = self.api.pw_set(&mut buf)?;
        self.api.check(st, "NvAPI_GPU_ClientPowerPoliciesSetStatus")?;
        Ok(pct)
    }
}
