//! Control del LED por I2C (controladores "PALIT" de las PNY XLR8 / Palit).
//!
//! Cuando NVAPI no reporta zonas de iluminación, VelocityX 1.4 habla directamente con el controlador
//! de LED usando `NvAPI_I2CWriteEx` / `NvAPI_I2CReadEx` (puerto I2C 1). Todo lo de este archivo está
//! deducido del código de VelocityX.exe:
//!
//! * `NV_I2C_INFO_V3` (0x40 bytes, versión 0x30040): displayMask=0, bIsDDCPort=0, regAddrSize=1,
//!   i2cSpeed=0xFFFF, i2cSpeedKhz=0, portId=1, bIsPortIdSet=1.
//! * Controlador NUEVO (dirección de 8 bits 0x92): identificación con regs 0xF6..0xFA = "PALIT";
//!   dígitos ASCII en 0xFC..0xFE y byte 0xF5 → ID = dígitos × 10 + (reg[0xF5] & 0x1F).
//!   color fijo: reg 0x60 = 1; 0x6C,0x6D,0x6E = R,G,B; 0x6F = brillo %.
//!   buzón 0xE0: [0] parar · [1, dir, vel, 0x1F] ciclo · [2, dir, vel, 0, R, G, B, 100, 5] estrobo.
//!   respiración: parar y escribir 22 bytes en los regs 0x60..0x75.
//! * Controlador ANTIGUO (dirección de 8 bits 0x10): regs 0x07..0x0B = "PALIT"; color fijo: reg 0x03 <- [R,G,B,0xFF].
//!
//! **Seguridad**: solo se escriben los registros anteriores (lista blanca). VelocityX también habla con
//! otros chips (p. ej. 0x40, probablemente el regulador de voltaje); este módulo NUNCA los toca.
//! La identificación es de solo lectura.

#[cfg(windows)]
use super::raw::NvapiLib;
use super::{LedController, Rgb};
use crate::error::{HwError, HwResult};
use std::cell::RefCell;
use std::collections::HashMap;

pub const ID_I2C_WRITE: u32 = 0x283A_C65A;
pub const ID_I2C_READ: u32 = 0x4D7B_0709;
pub const I2C_INFO_VERSION: u32 = 0x30040;
pub const I2C_PORT: u8 = 1;

pub const DEV_NEW: u8 = 0x92; // 7 bits: 0x49
pub const DEV_OLD: u8 = 0x10; // 7 bits: 0x08
pub const REG_MAILBOX: u8 = 0xE0;
pub const IMG_FIRST: u8 = 0x60;
pub const IMG_LAST: u8 = 0x75;

/// Rangos contiguos [primero, último] que VelocityX escribe en cada dispositivo.
pub fn allowed_ranges(dev: u8) -> &'static [(u16, u16)] {
    match dev {
        DEV_NEW => &[(IMG_FIRST as u16, IMG_LAST as u16), (REG_MAILBOX as u16, REG_MAILBOX as u16 + 8)],
        DEV_OLD => &[(0x03, 0x06)],
        _ => &[],
    }
}

#[repr(C)]
pub struct I2cInfoV3 {
    pub version: u32,
    pub display_mask: u32,
    pub is_ddc_port: u8,
    pub i2c_dev_address: u8,
    pub reg_address: *mut u8,
    pub reg_addr_size: u32,
    pub data: *mut u8,
    pub cb_size: u32,
    pub i2c_speed: u32,
    pub i2c_speed_khz: i32,
    pub port_id: u8,
    pub is_port_id_set: u32,
}

// Mismos desplazamientos que en el código de VelocityX (+0x09 dirección, +0x10 reg, +0x20 datos...)
#[cfg(target_pointer_width = "64")]
const _: () = {
    use std::mem::{offset_of, size_of};
    assert!(size_of::<I2cInfoV3>() == 0x40);
    assert!(offset_of!(I2cInfoV3, i2c_dev_address) == 0x09);
    assert!(offset_of!(I2cInfoV3, reg_address) == 0x10);
    assert!(offset_of!(I2cInfoV3, reg_addr_size) == 0x18);
    assert!(offset_of!(I2cInfoV3, data) == 0x20);
    assert!(offset_of!(I2cInfoV3, cb_size) == 0x28);
    assert!(offset_of!(I2cInfoV3, i2c_speed) == 0x2C);
    assert!(offset_of!(I2cInfoV3, i2c_speed_khz) == 0x30);
    assert!(offset_of!(I2cInfoV3, port_id) == 0x34);
    assert!(offset_of!(I2cInfoV3, is_port_id_set) == 0x38);
};

/// Bus I2C (real por NVAPI o simulado).
pub trait I2cBus {
    fn gpu_name(&self) -> String {
        "GPU NVIDIA".into()
    }
    fn read(&self, dev: u8, reg: u8) -> HwResult<u8>;
    fn write(&self, dev: u8, reg: u8, payload: &[u8]) -> HwResult<()>;
}

// --------------------------------------------------------------------------------------------
// Bus real (NVAPI, solo Windows)
// --------------------------------------------------------------------------------------------
#[cfg(windows)]
pub struct NvapiBus {
    lib: NvapiLib,
    handle: usize,
    gpu_name: String,
    rd: super::raw::FnI2c,
    wr: super::raw::FnI2c,
}

#[cfg(windows)]
impl NvapiBus {
    pub fn new() -> HwResult<Self> {
        let lib = NvapiLib::new()?;
        let (handle, gpu_name) =
            lib.gpus()?.into_iter().next().ok_or_else(|| HwError::new("NVAPI no encontró ninguna GPU NVIDIA."))?;
        Ok(Self {
            wr: lib.function(ID_I2C_WRITE, false)?.unwrap(),
            rd: lib.function(ID_I2C_READ, false)?.unwrap(),
            lib,
            handle,
            gpu_name,
        })
    }

    fn info(dev: u8, reg: &mut [u8; 1], data: &mut [u8]) -> I2cInfoV3 {
        I2cInfoV3 {
            version: I2C_INFO_VERSION,
            display_mask: 0,
            is_ddc_port: 0,
            i2c_dev_address: dev,
            reg_address: reg.as_mut_ptr(),
            reg_addr_size: 1,
            data: data.as_mut_ptr(),
            cb_size: data.len() as u32,
            i2c_speed: 0xFFFF,
            i2c_speed_khz: 0,
            port_id: I2C_PORT,
            is_port_id_set: 1,
        }
    }
}

#[cfg(windows)]
impl I2cBus for NvapiBus {
    fn gpu_name(&self) -> String {
        self.gpu_name.clone()
    }

    fn read(&self, dev: u8, reg: u8) -> HwResult<u8> {
        let (mut reg_buf, mut data) = ([reg], [0u8]);
        let mut info = Self::info(dev, &mut reg_buf, &mut data);
        let mut unknown = 1u64;
        // SAFETY: `info` apunta a buffers vivos durante la llamada; layout comprobado arriba.
        let st = unsafe { (self.rd)(self.handle as *mut _, &mut info, &mut unknown) };
        self.lib.check(st, &format!("I2C lectura 0x{dev:02X}[0x{reg:02X}]"))?;
        Ok(data[0])
    }

    fn write(&self, dev: u8, reg: u8, payload: &[u8]) -> HwResult<()> {
        let mut reg_buf = [reg];
        let mut data = payload.to_vec();
        let mut info = Self::info(dev, &mut reg_buf, &mut data);
        let mut unknown = 1u64;
        // SAFETY: igual que `read`.
        let st = unsafe { (self.wr)(self.handle as *mut _, &mut info, &mut unknown) };
        self.lib.check(st, &format!("I2C escritura 0x{dev:02X}[0x{reg:02X}]"))
    }
}

/// Abre el bus I2C real (error limpio fuera de Windows).
pub fn open_real_bus() -> HwResult<Box<dyn I2cBus>> {
    #[cfg(windows)]
    {
        Ok(Box::new(NvapiBus::new()?))
    }
    #[cfg(not(windows))]
    {
        Err(HwError::new("NVAPI solo está disponible en Windows."))
    }
}

// --------------------------------------------------------------------------------------------
// Bus simulado (pruebas / --demo)
// --------------------------------------------------------------------------------------------
pub struct FakeBus {
    mem: RefCell<HashMap<u8, HashMap<u8, u8>>>,
    pub log: RefCell<Vec<(u8, u8, Vec<u8>)>>,
    absent: bool,
}

impl FakeBus {
    pub fn new(digits: &[u8; 3], rev: u8, legacy: bool, absent: bool) -> Self {
        let mut mem: HashMap<u8, HashMap<u8, u8>> = HashMap::from([(DEV_NEW, HashMap::new()), (DEV_OLD, HashMap::new())]);
        if !absent && !legacy {
            for (i, c) in b"PALIT".iter().enumerate() {
                mem.get_mut(&DEV_NEW).unwrap().insert(0xF6 + i as u8, *c);
            }
            for (i, c) in digits.iter().enumerate() {
                mem.get_mut(&DEV_NEW).unwrap().insert(0xFC + i as u8, *c);
            }
            mem.get_mut(&DEV_NEW).unwrap().insert(0xF5, rev);
        }
        if legacy {
            for (i, c) in b"PALIT".iter().enumerate() {
                mem.get_mut(&DEV_OLD).unwrap().insert(0x07 + i as u8, *c);
            }
            for (i, c) in b"2.0".iter().enumerate() {
                mem.get_mut(&DEV_OLD).unwrap().insert(0x0C + i as u8, *c);
            }
        }
        Self { mem: RefCell::new(mem), log: RefCell::new(vec![]), absent }
    }

    /// Controlador nuevo, dígitos "120", revisión 2 (como `FakeBus()` en Python).
    pub fn standard() -> Self {
        Self::new(b"120", 2, false, false)
    }
}

impl I2cBus for FakeBus {
    fn gpu_name(&self) -> String {
        "NVIDIA GeForce RTX 3070 (SIMULADA, I2C)".into()
    }

    fn read(&self, dev: u8, reg: u8) -> HwResult<u8> {
        let mem = self.mem.borrow();
        // Python: `not any(self.mem[dev])` itera las CLAVES del diccionario.
        let answers = mem.get(&dev).is_some_and(|m| m.keys().any(|&k| k != 0));
        if self.absent || !answers {
            return Err(HwError::new(format!("I2C lectura 0x{dev:02X}[0x{reg:02X}] falló: sin respuesta")));
        }
        Ok(mem[&dev].get(&reg).copied().unwrap_or(0))
    }

    fn write(&self, dev: u8, reg: u8, payload: &[u8]) -> HwResult<()> {
        self.log.borrow_mut().push((dev, reg, payload.to_vec()));
        let mut mem = self.mem.borrow_mut();
        let m = mem.entry(dev).or_default();
        for (i, b) in payload.iter().enumerate() {
            m.insert(reg.wrapping_add(i as u8), *b);
        }
        Ok(())
    }
}

// --------------------------------------------------------------------------------------------
// Reglas por ID (copiadas de la lógica de VelocityX)
// --------------------------------------------------------------------------------------------

/// Color fijo por registros 0x60/0x6C-0x6F (función 0x186d0 de VelocityX).
pub fn still_supported(i: i32) -> bool {
    if i >= 1000 && matches!(i % 10, 0 | 1) {
        return true;
    }
    if (11..=999).contains(&i) {
        return false;
    }
    if i > 1199 && i % 10 == 2 {
        return true;
    }
    if i <= 1999 {
        return false;
    }
    matches!(i % 10, 3 | 7)
}

pub fn still_needs_stop(i: i32) -> bool {
    (1199 < i && i <= 1999 && i % 10 == 2) || (i >= 2000 && matches!(i % 10, 3 | 7))
}

/// Respiración / ciclo / estroboscópico (funciones 0x18c40 y 0x18d80).
pub fn effects_supported(i: i32) -> bool {
    if i <= 2 || i == 10 {
        return false;
    }
    !(i >= 1000 && matches!(i % 10, 0 | 1))
}

/// IDs que terminan en 1 (>= 1000) son de un solo color: VelocityX usa el canal G para los tres.
pub fn gray_only(i: i32) -> bool {
    i >= 1000 && i % 10 == 1
}

// --------------------------------------------------------------------------------------------
// Controlador de alto nivel
// --------------------------------------------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Palit92,
    Palit10,
}

pub struct PalitLed {
    bus: Box<dyn I2cBus>,
    gpu_name: String,
    pub protocol: Option<Protocol>,
    pub id: i32,
    /// `true` = comportamiento idéntico al de VelocityX/versión Python (sin parar efectos ni verificar).
    /// Solo lo usan las pruebas diferenciales contra el código original.
    pub legacy_behavior: bool,
}

impl PalitLed {
    pub fn new(bus: Box<dyn I2cBus>) -> Self {
        let gpu_name = bus.gpu_name();
        let mut led = Self { bus, gpu_name, protocol: None, id: -1, legacy_behavior: false };
        let (protocol, id) = led.identify();
        led.protocol = protocol;
        led.id = id;
        led
    }

    pub fn bus(&self) -> &dyn I2cBus {
        self.bus.as_ref()
    }

    // -- identificación (solo lectura) ------------------------------------------------------
    fn read_block(&self, dev: u8, first: u8, n: u8) -> Option<Vec<u8>> {
        (0..n).map(|i| self.bus.read(dev, first + i).ok()).collect()
    }

    fn identify(&self) -> (Option<Protocol>, i32) {
        if let Some(r) = self.read_block(DEV_NEW, 0xF0, 16) {
            if &r[6..11] == b"PALIT" {
                let digits = (r[12] & 0xF) as i32 * 100 + (r[13] & 0xF) as i32 * 10 + (r[14] & 0xF) as i32;
                return (Some(Protocol::Palit92), digits * 10 + (r[5] & 0x1F) as i32);
            }
        }
        if let Some(r) = self.read_block(DEV_OLD, 0x00, 16) {
            if &r[7..12] == b"PALIT" {
                return (Some(Protocol::Palit10), 0);
            }
        }
        (None, -1)
    }

    // -- escritura con lista blanca ---------------------------------------------------------
    fn w(&self, dev: u8, reg: u8, data: &[u8]) -> HwResult<()> {
        let (first, last) = (reg as u16, reg as u16 + data.len() as u16 - 1);
        if !allowed_ranges(dev).iter().any(|&(lo, hi)| lo <= first && last <= hi) {
            return Err(HwError::new(format!("Escritura I2C bloqueada por seguridad: 0x{dev:02X}[0x{reg:02X}]")));
        }
        self.bus.write(dev, reg, data)
    }

    fn need(&self) -> HwResult<()> {
        if self.protocol.is_none() {
            return Err(HwError::new("No se detectó un controlador LED compatible por I2C."));
        }
        Ok(())
    }

    fn need_effects(&self) -> HwResult<()> {
        self.need()?;
        if !self.supports_effects() {
            return Err(HwError::new(format!("Esta tarjeta (ID {}) no admite efectos según VelocityX.", self.id)));
        }
        Ok(())
    }

    fn write_still(&self, r: u8, g: u8, b: u8, br: u8) -> HwResult<()> {
        self.w(DEV_NEW, 0x60, &[1])?;
        self.w(DEV_NEW, 0x6C, &[r])?;
        self.w(DEV_NEW, 0x6D, &[g])?;
        self.w(DEV_NEW, 0x6E, &[b])?;
        self.w(DEV_NEW, 0x6F, &[br])
    }

    /// Lectura de vuelta (solo lectura): ¿el controlador conserva el color pedido?
    fn still_applied(&self, r: u8, g: u8, b: u8, br: u8) -> bool {
        [(0x60, 1), (0x6C, r), (0x6D, g), (0x6E, b), (0x6F, br)]
            .iter()
            .all(|&(reg, want)| self.bus.read(DEV_NEW, reg).map_or(true, |v| v == want))
    }

    fn stop(&self) -> HwResult<()> {
        self.w(DEV_NEW, REG_MAILBOX, &[0])?;
        self.w(DEV_NEW, 0x60, &[0])
    }
}

impl LedController for PalitLed {
    fn describe(&self) -> String {
        let yn = |b: bool| if b { "sí" } else { "no" };
        match self.protocol {
            Some(Protocol::Palit92) => format!(
                "{}: controlador I2C PALIT (0x92), ID {} → color fijo: {}, efectos: {}",
                self.gpu_name,
                self.id,
                yn(still_supported(self.id)),
                yn(effects_supported(self.id))
            ),
            Some(Protocol::Palit10) => format!("{}: controlador I2C PALIT antiguo (0x10) → color fijo", self.gpu_name),
            None => format!("{}: no se encontró un controlador LED PALIT por I2C.", self.gpu_name),
        }
    }

    fn num_zones(&self) -> usize {
        usize::from(self.protocol.is_some())
    }

    fn supports_effects(&self) -> bool {
        self.protocol == Some(Protocol::Palit92) && effects_supported(self.id)
    }

    fn set_static(&mut self, (r, g, b): Rgb, brightness: i32, _zones: Option<&[usize]>) -> HwResult<()> {
        self.need()?;
        let br = brightness.clamp(0, 100) as u8;
        if self.protocol == Some(Protocol::Palit10) {
            let k = br as f64 / 100.0;
            // Python `round()` redondea los .5 al par.
            let sc = |c: u8| (c as f64 * k).round_ties_even() as u8;
            return self.w(DEV_OLD, 0x03, &[sc(r), sc(g), sc(b), 0xFF]);
        }
        if !still_supported(self.id) {
            return Err(HwError::new(format!(
                "VelocityX no define color fijo para este controlador (ID {}). \
                 Prueba un efecto o envíame el resultado de --i2c-scan.",
                self.id
            )));
        }
        let (r, g, b) = if gray_only(self.id) { (g, g, g) } else { (r, g, b) };
        if self.legacy_behavior {
            if still_needs_stop(self.id) {
                self.w(DEV_NEW, REG_MAILBOX, &[0])?;
            }
            return self.write_still(r, g, b, br);
        }
        // Tras una respiración/ciclo/estrobo quedan parámetros del efecto en los registros: se paran
        // primero (mismo paso que «Restaurar») y, si la lectura de vuelta no coincide, se repite.
        if effects_supported(self.id) {
            self.stop()?;
        } else if still_needs_stop(self.id) {
            self.w(DEV_NEW, REG_MAILBOX, &[0])?;
        }
        self.write_still(r, g, b, br)?;
        if !self.still_applied(r, g, b, br) {
            self.write_still(r, g, b, br)?;
        }
        Ok(())
    }

    fn set_off(&mut self, zones: Option<&[usize]>) -> HwResult<()> {
        self.set_static((0, 0, 0), 100, zones)
    }

    fn set_breathing(&mut self, a: Rgb, b: Rgb, period_ms: u32, _zones: Option<&[usize]>) -> HwResult<()> {
        self.need_effects()?;
        let t = (period_ms / 2).clamp(100, 30000) as u16;
        let lvl = |c: Rgb| if c != (0, 0, 0) { 100 } else { 0 };
        let mut img = [0u8; 22];
        img[0] = 0x22;
        img[1] = 0x01;
        img[2..6].copy_from_slice(&[(t & 0xFF) as u8, (t >> 8) as u8, (t & 0xFF) as u8, (t >> 8) as u8]);
        img[6..10].copy_from_slice(&[100, 0, 100, 0]);
        img[12..16].copy_from_slice(&[a.0, a.1, a.2, lvl(a)]);
        img[16..20].copy_from_slice(&[b.0, b.1, b.2, lvl(b)]);
        self.stop()?;
        for (k, v) in img.iter().enumerate() {
            // VelocityX escribe byte a byte
            self.w(DEV_NEW, IMG_FIRST + k as u8, &[*v])?;
        }
        Ok(())
    }

    fn set_cycle(&mut self, speed: u32, reverse: bool) -> HwResult<()> {
        self.need_effects()?;
        self.w(DEV_NEW, REG_MAILBOX, &[1, u8::from(reverse), (speed & 0xFF) as u8, 0x1F])
    }

    fn set_strobe(&mut self, (r, g, b): Rgb, speed: u32, reverse: bool) -> HwResult<()> {
        self.need_effects()?;
        self.w(DEV_NEW, REG_MAILBOX, &[2, u8::from(reverse), (speed & 0xFF) as u8, 0, r, g, b, 100, 5])
    }

    fn restore_default(&mut self) -> HwResult<()> {
        self.need()?;
        if self.protocol == Some(Protocol::Palit92) && effects_supported(self.id) {
            self.stop()
        } else {
            Err(HwError::new(
                "Este controlador no tiene función de restauración; reinicia el PC para volver al color original.",
            ))
        }
    }
}

// --------------------------------------------------------------------------------------------
// Informe de diagnóstico (solo lectura)
// --------------------------------------------------------------------------------------------
pub fn scan_report(led: &PalitLed) -> String {
    let bus = led.bus();
    let mut lines = vec![format!("GPU: {}", led.gpu_name)];
    let mut dump = |dev: u8, first: u8, n: u8, title: &str| {
        let mut errs = 0;
        let vals: Vec<String> = (0..n)
            .map(|i| match bus.read(dev, first + i) {
                Ok(v) => format!("{v:02X}"),
                Err(_) => {
                    errs += 1;
                    "??".to_string()
                }
            })
            .collect();
        let suffix = if errs == n { "   (sin respuesta)" } else { "" };
        lines.push(format!("{title}: {}{suffix}", vals.join(" ")));
    };
    dump(DEV_NEW, 0xF0, 16, "I2C 0x92 regs 0xF0-0xFF");
    dump(DEV_OLD, 0x00, 16, "I2C 0x10 regs 0x00-0x0F");
    lines.push(format!("→ {}", led.describe()));
    lines.join("\n")
}
