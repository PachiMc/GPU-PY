//! Carga de `nvapi64.dll` y resolución de funciones con `nvapi_QueryInterface`.
//!
//! Los ids de función son los confirmados dentro de VelocityX.exe (ver los módulos hermanos).
//! Fuera de Windows `NvapiLib::new()` devuelve un error y nada más se ejecuta.

use crate::error::{HwError, HwResult};

pub type GpuHandle = usize;

pub const ID_INITIALIZE: u32 = 0x0150_E828;
pub const ID_ENUM_GPUS: u32 = 0xE5AC_921F;
pub const ID_GPU_NAME: u32 = 0xCEEE_8E9F;
pub const ID_ERROR_MSG: u32 = 0x6C2D_048C;

pub const STATUS_INCOMPATIBLE_VERSION: i32 = -9;

const STATUS_TEXT: &[(i32, &str)] = &[
    (-1, "Error genérico de NVAPI."),
    (-3, "El driver no implementa esta función (tarjeta o driver no compatibles)."),
    (-4, "NVAPI no está inicializada."),
    (-5, "Argumento inválido."),
    (-6, "No se encontró ninguna GPU NVIDIA."),
    (-9, "Versión de estructura incompatible con el driver."),
    (-104, "Función no soportada por esta GPU."),
];

/// Convierte un código de estado en un error legible (`driver_msg` = texto que dio NVAPI, si lo dio).
pub fn status_error(status: i32, what: &str, driver_msg: Option<String>) -> HwError {
    let msg = STATUS_TEXT
        .iter()
        .find(|(s, _)| *s == status)
        .map(|(_, t)| (*t).to_string())
        .or(driver_msg)
        .unwrap_or_else(|| "error desconocido".to_string());
    HwError::new(format!("{what} falló ({status}): {msg}"))
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::nvapi::{i2c::I2cInfoV3, illum::IllumParams};
    use libloading::Library;
    use std::ffi::{c_char, c_void};
    use std::mem::{size_of, transmute_copy};

    type QueryInterface = unsafe extern "C" fn(u32) -> *const c_void;
    type FnInit = unsafe extern "C" fn() -> i32;
    type FnEnum = unsafe extern "C" fn(*mut [*mut c_void; 64], *mut u32) -> i32;
    type FnName = unsafe extern "C" fn(*mut c_void, *mut [c_char; 64]) -> i32;
    type FnErr = unsafe extern "C" fn(i32, *mut [c_char; 64]) -> i32;
    type FnZones = unsafe extern "C" fn(*mut c_void, *mut IllumParams) -> i32;
    pub type FnBuf = unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32;
    pub type FnI2c = unsafe extern "C" fn(*mut c_void, *mut I2cInfoV3, *mut u64) -> i32;

    pub const ID_ZONES_GET: u32 = 0x3DBF_5764;
    pub const ID_ZONES_SET: u32 = 0x197D_065E;

    pub struct NvapiLib {
        _lib: Library, // mantiene la DLL cargada mientras existan los punteros
        qi: QueryInterface,
        enum_gpus: FnEnum,
        gpu_name: FnName,
        err_msg: Option<FnErr>,
        zones_get: FnZones,
        zones_set: FnZones,
    }

    fn cstr(buf: &[c_char]) -> String {
        let bytes: Vec<u8> = buf.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Resuelve una función por id. `F` debe ser un `unsafe extern "C" fn(...)` con la firma correcta.
    fn resolve<F: Copy>(qi: QueryInterface, id: u32, optional: bool) -> HwResult<Option<F>> {
        assert_eq!(size_of::<F>(), size_of::<*const c_void>());
        // SAFETY: nvapi_QueryInterface solo busca el id en una tabla del driver.
        let ptr = unsafe { qi(id) };
        if ptr.is_null() {
            if optional {
                return Ok(None);
            }
            return Err(HwError::new(format!(
                "El driver no expone la función NVAPI 0x{id:08X}. Actualiza el driver de NVIDIA."
            )));
        }
        // SAFETY: el puntero es una función del driver con la firma que declara quien llama.
        Ok(Some(unsafe { transmute_copy::<*const c_void, F>(&ptr) }))
    }

    impl NvapiLib {
        pub fn new() -> HwResult<Self> {
            let dll = if size_of::<*const c_void>() == 8 { "nvapi64.dll" } else { "nvapi.dll" };
            let fail = || HwError::new(format!("No se pudo cargar {dll}. ¿Está instalado el driver de NVIDIA?"));
            // SAFETY: cargamos una DLL del driver de NVIDIA; no ejecuta nada hasta llamar a sus funciones.
            let lib = unsafe { Library::new(dll) }.map_err(|_| fail())?;
            // SAFETY: la firma de nvapi_QueryInterface es `void* (*)(uint32_t)`.
            let qi: QueryInterface = unsafe { *lib.get::<QueryInterface>(b"nvapi_QueryInterface\0").map_err(|_| fail())? };

            let init: FnInit = resolve(qi, ID_INITIALIZE, false)?.unwrap();
            let me = Self {
                qi,
                enum_gpus: resolve(qi, ID_ENUM_GPUS, false)?.unwrap(),
                gpu_name: resolve(qi, ID_GPU_NAME, false)?.unwrap(),
                err_msg: resolve(qi, ID_ERROR_MSG, true)?,
                zones_get: resolve(qi, ID_ZONES_GET, false)?.unwrap(),
                zones_set: resolve(qi, ID_ZONES_SET, false)?.unwrap(),
                _lib: lib,
            };
            // SAFETY: NvAPI_Initialize no recibe argumentos.
            me.check(unsafe { init() }, "NvAPI_Initialize")?;
            Ok(me)
        }

        /// Función extra por id (la usan `oc` e `i2c` para sus propias funciones).
        pub fn function<F: Copy>(&self, id: u32, optional: bool) -> HwResult<Option<F>> {
            resolve(self.qi, id, optional)
        }

        pub fn check(&self, status: i32, what: &str) -> HwResult<()> {
            if status == 0 {
                return Ok(());
            }
            let mut msg = None;
            if let Some(f) = self.err_msg {
                let mut buf = [0 as c_char; 64];
                // SAFETY: buf es un NvAPI_ShortString (64 bytes).
                if unsafe { f(status, &mut buf) } == 0 {
                    msg = Some(cstr(&buf));
                }
            }
            Err(status_error(status, what, msg))
        }

        pub fn gpus(&self) -> HwResult<Vec<(GpuHandle, String)>> {
            let mut handles = [std::ptr::null_mut::<c_void>(); 64];
            let mut count = 0u32;
            // SAFETY: handles tiene espacio para 64 (NVAPI_MAX_PHYSICAL_GPUS).
            self.check(unsafe { (self.enum_gpus)(&mut handles, &mut count) }, "NvAPI_EnumPhysicalGPUs")?;
            Ok((0..count.min(64) as usize)
                .map(|i| {
                    let mut buf = [0 as c_char; 64];
                    // SAFETY: handle devuelto por NVAPI, buf de 64 bytes.
                    let ok = unsafe { (self.gpu_name)(handles[i], &mut buf) } == 0;
                    (handles[i] as usize, if ok { cstr(&buf) } else { "GPU NVIDIA".into() })
                })
                .collect())
        }

        pub fn zones_get(&self, h: GpuHandle, p: &mut IllumParams) -> HwResult<()> {
            // SAFETY: IllumParams tiene el layout exacto que espera el driver (ver illum.rs).
            let st = unsafe { (self.zones_get)(h as *mut c_void, p) };
            self.check(st, "ClientIlluminationZonesGetControl")
        }

        pub fn zones_set(&self, h: GpuHandle, p: &mut IllumParams) -> HwResult<()> {
            // SAFETY: igual que zones_get.
            let st = unsafe { (self.zones_set)(h as *mut c_void, p) };
            self.check(st, "ClientIlluminationZonesSetControl")
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    use crate::nvapi::illum::IllumParams;

    pub struct NvapiLib;

    impl NvapiLib {
        pub fn new() -> HwResult<Self> {
            Err(HwError::new("NVAPI solo está disponible en Windows."))
        }
        pub fn function<F: Copy>(&self, _id: u32, _optional: bool) -> HwResult<Option<F>> {
            Err(HwError::new("NVAPI solo está disponible en Windows."))
        }
        pub fn check(&self, status: i32, what: &str) -> HwResult<()> {
            if status == 0 { Ok(()) } else { Err(status_error(status, what, None)) }
        }
        pub fn gpus(&self) -> HwResult<Vec<(GpuHandle, String)>> {
            Err(HwError::new("NVAPI solo está disponible en Windows."))
        }
        pub fn zones_get(&self, _h: GpuHandle, _p: &mut IllumParams) -> HwResult<()> {
            Err(HwError::new("NVAPI solo está disponible en Windows."))
        }
        pub fn zones_set(&self, _h: GpuHandle, _p: &mut IllumParams) -> HwResult<()> {
            Err(HwError::new("NVAPI solo está disponible en Windows."))
        }
    }
}

pub use imp::NvapiLib;
#[cfg(windows)]
pub use imp::{FnBuf, FnI2c};
