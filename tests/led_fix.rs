//! Arreglo del LED «colgado» (había que pulsar Restaurar y luego Aplicar): el estado previo
//! (p. ej. una respiración) no debe sobrevivir a un color fijo, y si el controlador no acepta la
//! escritura, se verifica leyendo de vuelta y se reintenta.

use gpu_py::error::HwResult;
use gpu_py::nvapi::fake::FakeNvapi;
use gpu_py::nvapi::i2c::{DEV_NEW, FakeBus, I2cBus, PalitLed};
use gpu_py::nvapi::illum::{IllumApi, IllumLed, IllumParams, MODE_MANUAL};
use gpu_py::nvapi::raw::GpuHandle;
use gpu_py::nvapi::LedController;
use std::cell::Cell;
use std::rc::Rc;

/// Driver de zonas que «pierde» una escritura cuando se le indica.
struct FlakyIllum {
    inner: FakeNvapi,
    drop_next: Cell<bool>,
}
struct Shared(Rc<FlakyIllum>);
impl IllumApi for Shared {
    fn gpus(&self) -> HwResult<Vec<(GpuHandle, String)>> {
        self.0.inner.gpus()
    }
    fn get_control(&self, h: GpuHandle, p: &mut IllumParams) -> HwResult<()> {
        self.0.inner.get_control(h, p)
    }
    fn set_control(&self, h: GpuHandle, p: &mut IllumParams) -> HwResult<()> {
        if self.0.drop_next.replace(false) {
            return Ok(()); // el driver dice OK pero no se queda con el cambio
        }
        self.0.inner.set_control(h, p)
    }
}

fn zone0(api: &FlakyIllum) -> (u32, [u8; 4]) {
    let p = api.inner.current.borrow();
    (p.zones[0].ctrl_mode, p.zones[0].data[0..4].try_into().unwrap())
}

#[test]
fn illum_static_after_breathing_leaves_no_stale_params() {
    let api = Rc::new(FlakyIllum { inner: FakeNvapi::standard(), drop_next: Cell::new(false) });
    let mut led = IllumLed::new(Some(Box::new(Shared(api.clone()))), 0).unwrap();
    led.set_breathing((255, 0, 0), (0, 0, 255), 3000, None).unwrap();
    led.set_static((10, 20, 30), 80, None).unwrap();
    let p = api.inner.current.borrow();
    assert_eq!(p.zones[0].ctrl_mode, MODE_MANUAL);
    assert_eq!(&p.zones[0].data[0..4], &[10, 20, 30, 80]);
    assert!(p.zones[0].data[4..].iter().all(|&b| b == 0), "quedaron parámetros de la respiración");
}

#[test]
fn illum_recovers_when_driver_drops_a_write() {
    let api = Rc::new(FlakyIllum { inner: FakeNvapi::standard(), drop_next: Cell::new(false) });
    let mut led = IllumLed::new(Some(Box::new(Shared(api.clone()))), 0).unwrap();
    led.set_breathing((255, 0, 0), (0, 0, 255), 3000, None).unwrap();
    api.drop_next.set(true);
    led.set_static((1, 2, 3), 50, None).unwrap();
    assert_eq!(zone0(&api), (MODE_MANUAL, [1, 2, 3, 50]), "el reintento automático no dejó el color pedido");
}

#[test]
fn illum_legacy_mode_reproduces_the_original_bug() {
    let api = Rc::new(FlakyIllum { inner: FakeNvapi::standard(), drop_next: Cell::new(false) });
    let mut led = IllumLed::new(Some(Box::new(Shared(api.clone()))), 0).unwrap();
    led.legacy_behavior = true;
    led.set_breathing((255, 0, 0), (0, 0, 255), 3000, None).unwrap();
    api.drop_next.set(true);
    led.set_static((1, 2, 3), 50, None).unwrap();
    assert_ne!(zone0(&api), (MODE_MANUAL, [1, 2, 3, 50]), "el modo original no debería reintentar");
}

/// Bus I2C que ignora una escritura a un registro concreto.
struct FlakyBus {
    inner: FakeBus,
    drop_reg: Cell<Option<u8>>,
}
struct SharedBus(Rc<FlakyBus>);
impl I2cBus for SharedBus {
    fn gpu_name(&self) -> String {
        self.0.inner.gpu_name()
    }
    fn read(&self, d: u8, r: u8) -> HwResult<u8> {
        self.0.inner.read(d, r)
    }
    fn write(&self, d: u8, r: u8, p: &[u8]) -> HwResult<()> {
        if self.0.drop_reg.get() == Some(r) {
            self.0.drop_reg.set(None);
            return Ok(());
        }
        self.0.inner.write(d, r, p)
    }
}

#[test]
fn i2c_static_stops_effect_first_and_retries_lost_write() {
    let bus = Rc::new(FlakyBus { inner: FakeBus::standard(), drop_reg: Cell::new(None) });
    let mut led = PalitLed::new(Box::new(SharedBus(bus.clone())));
    led.set_breathing((255, 0, 0), (0, 0, 255), 3000, None).unwrap();
    bus.drop_reg.set(Some(0x6D)); // se pierde la escritura del canal verde
    bus.inner.log.borrow_mut().clear();
    led.set_static((9, 99, 199), 70, None).unwrap();
    let rd = |r| bus.inner.read(DEV_NEW, r).unwrap();
    assert_eq!([rd(0x60), rd(0x6C), rd(0x6D), rd(0x6E), rd(0x6F)], [1, 9, 99, 199, 70]);
    // lo primero que hace es parar el efecto anterior (buzón y registro 0x60 a 0)
    let log = bus.inner.log.borrow();
    assert_eq!((log[0].1, log[0].2.clone()), (0xE0, vec![0]));
    assert_eq!((log[1].1, log[1].2.clone()), (0x60, vec![0]));
}

#[test]
fn i2c_never_writes_outside_the_whitelist() {
    let bus = Rc::new(FlakyBus { inner: FakeBus::standard(), drop_reg: Cell::new(None) });
    let mut led = PalitLed::new(Box::new(SharedBus(bus.clone())));
    led.set_static((1, 2, 3), 100, None).unwrap();
    led.set_breathing((1, 2, 3), (4, 5, 6), 2000, None).unwrap();
    led.set_cycle(50, true).unwrap();
    led.set_strobe((1, 1, 1), 10, false).unwrap();
    led.restore_default().unwrap();
    for (dev, reg, data) in bus.inner.log.borrow().iter() {
        let (lo, hi) = (*reg as u16, *reg as u16 + data.len() as u16 - 1);
        assert_eq!(*dev, DEV_NEW);
        assert!((0x60..=0x75).contains(&lo) && hi <= 0x75 || (0xE0..=0xE8).contains(&lo) && hi <= 0xE8);
    }
}
