//! Drivers NVAPI simulados: sirven para `--demo` y para probar el formato binario exacto
//! (los `assert` replican las comprobaciones del simulador de la versión Python).

use super::illum::{IllumApi, IllumParams, MODE_MANUAL, PARAMS_VERSION, ZONE_RGB};
use super::oc::*;
use super::raw::{GpuHandle, STATUS_INCOMPATIBLE_VERSION};
use crate::error::{HwError, HwResult};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

// ------------------------------------------------------------------------------------------
// LED por zonas
// ------------------------------------------------------------------------------------------
/// Imita al driver: guarda el último estado recibido.
pub struct FakeNvapi {
    default: IllumParams,
    pub current: RefCell<IllumParams>,
    pub history: RefCell<Vec<IllumParams>>,
}

impl FakeNvapi {
    pub fn new(zone_types: &[u32]) -> Self {
        let mut d = IllumParams::new(false);
        d.num_zones = zone_types.len() as u32;
        for (i, t) in zone_types.iter().enumerate() {
            d.zones[i].zone_type = *t;
            d.zones[i].ctrl_mode = MODE_MANUAL;
            d.zones[i].data[0..4].copy_from_slice(&[0, 255, 0, 100]);
        }
        Self { default: d, current: RefCell::new(d), history: RefCell::new(vec![]) }
    }

    /// Dos zonas RGB (como `FakeNvapi()` en Python).
    pub fn standard() -> Self {
        Self::new(&[ZONE_RGB, ZONE_RGB])
    }
}

impl IllumApi for FakeNvapi {
    fn gpus(&self) -> HwResult<Vec<(GpuHandle, String)>> {
        Ok(vec![(1, "NVIDIA GeForce RTX 3070 (SIMULADA)".into())])
    }

    fn get_control(&self, _h: GpuHandle, p: &mut IllumParams) -> HwResult<()> {
        let src = if p.flags & 1 != 0 { self.default } else { *self.current.borrow() };
        *p = src;
        p.version = PARAMS_VERSION;
        p.flags = 0;
        Ok(())
    }

    fn set_control(&self, _h: GpuHandle, p: &mut IllumParams) -> HwResult<()> {
        if p.version != PARAMS_VERSION {
            return Err(HwError::new("Versión de estructura incompatible (simulador)."));
        }
        *self.current.borrow_mut() = *p;
        self.history.borrow_mut().push(*p);
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------
// Overclock / potencia
// ------------------------------------------------------------------------------------------
/// Valida las posiciones exactas que usa VelocityX.
pub struct FakeOcApi {
    pub v3_supported: bool,
    pub editable: bool,
    delta: RefCell<HashMap<u32, i32>>,
    ranges: HashMap<u32, (i32, i32)>,
    power_cfg: (u32, u32, u32),
    pub power_pct: Cell<u32>,
    pub set_calls: RefCell<Vec<(u32, i32)>>,
}

impl FakeOcApi {
    pub fn new(v3_supported: bool, editable: bool, mem: (i32, i32), core: (i32, i32), power: (u32, u32, u32)) -> Self {
        Self {
            v3_supported,
            editable,
            delta: RefCell::new(HashMap::from([(DOM_CORE, 0), (DOM_MEM, 0)])),
            ranges: HashMap::from([(DOM_CORE, core), (DOM_MEM, mem)]),
            power_cfg: power,
            power_pct: Cell::new(100),
            set_calls: RefCell::new(vec![]),
        }
    }

    /// Valores por defecto del simulador Python.
    pub fn standard() -> Self {
        Self::new(true, true, (-500, 1000), (-300, 300), (50, 100, 120))
    }
}

impl OcApi for FakeOcApi {
    fn ps_get(&self, buf: &mut [u8]) -> i32 {
        let ver = u32_at(buf, 0);
        if ver == PS_V3 && !self.v3_supported {
            return STATUS_INCOMPATIBLE_VERSION;
        }
        assert!(ver == PS_V3 || ver == PS_V1);
        put_u32(buf, 4, u32::from(self.editable));
        put_u32(buf, 8, 1); // 1 P-state
        put_u32(buf, 12, 2); // 2 relojes
        put_u32(buf, 16, 0);
        let po = PSTATE_BASE;
        put_u32(buf, po, 0);
        for (c, dom) in [DOM_CORE, DOM_MEM].into_iter().enumerate() {
            let co = po + 8 + c * CLOCK_STRIDE;
            let (lo, hi) = self.ranges[&dom];
            put_u32(buf, co, dom);
            put_u32(buf, co + 4, 1);
            put_u32(buf, co + 8, 1);
            put_s32(buf, co + 0xC, self.delta.borrow()[&dom] * 1000);
            put_s32(buf, co + 0x10, lo * 1000);
            put_s32(buf, co + 0x14, hi * 1000);
        }
        0
    }

    fn ps_set(&self, buf: &mut [u8]) -> i32 {
        assert_eq!(u32_at(buf, 0), PS_V1, "VelocityX escribe con la versión 0x11C94");
        assert_eq!(
            (u32_at(buf, 8), u32_at(buf, 0xC), u32_at(buf, 0x10), u32_at(buf, PSTATE_BASE)),
            (1, 1, 0, 0)
        );
        let (dom, delta) = (u32_at(buf, 0x1C), s32_at(buf, 0x28));
        assert!(self.ranges.contains_key(&dom) && delta % 1000 == 0);
        let (lo, hi) = self.ranges[&dom];
        if !(lo * 1000..=hi * 1000).contains(&delta) {
            return -5;
        }
        self.delta.borrow_mut().insert(dom, delta / 1000);
        self.set_calls.borrow_mut().push((dom, delta / 1000));
        0
    }

    fn pw_info(&self, buf: &mut [u8]) -> HwResult<i32> {
        if u32_at(buf, 0) != PWR_INFO_V2 {
            return Ok(STATUS_INCOMPATIBLE_VERSION);
        }
        buf[4] = 1;
        buf[5] = 1;
        let (lo, de, hi) = self.power_cfg;
        put_u32(buf, 8, 0);
        for (off, v) in [0x10usize, 0x1C, 0x28].into_iter().zip([lo, de, hi]) {
            put_u32(buf, 8 + off, v * 1000);
        }
        Ok(0)
    }

    fn pw_get(&self, buf: &mut [u8]) -> HwResult<i32> {
        assert!(u32_at(buf, 0) == PWR_STATUS_V2 && buf[4] == 1 && u32_at(buf, 8) == 0);
        put_u32(buf, 8 + 0xC, self.power_pct.get() * 1000);
        Ok(0)
    }

    fn pw_set(&self, buf: &mut [u8]) -> HwResult<i32> {
        assert!(u32_at(buf, 0) == PWR_STATUS_V2 && buf[4] == 1 && u32_at(buf, 8) == 0);
        assert_eq!(u32_at(buf, 0x10) & 1, 0);
        self.power_pct.set(u32_at(buf, 0x14) / 1000);
        Ok(0)
    }

    fn check(&self, st: i32, what: &str) -> HwResult<()> {
        if st != 0 { Err(HwError::new(format!("{what} falló ({st})"))) } else { Ok(()) }
    }
}
