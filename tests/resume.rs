//! Al suspender/hibernar el driver restablece overclock, ventiladores y LED. GPU PY debe volver a aplicar lo
//! último que se aplicó, con reintentos si el driver aún no está listo, y respetar el ajuste que lo desactiva.

use gpu_py::app::{App, State};
use gpu_py::backend::{DemoBackend, GpuBackend, Limits};
use gpu_py::error::{HwError, HwResult};
use gpu_py::nvapi::fake::FakeNvapi;
use gpu_py::nvapi::illum::{IllumApi, IllumLed, IllumParams};
use gpu_py::nvapi::raw::GpuHandle;
use slint::ComponentHandle;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

struct P(Rc<MinimalSoftwareWindow>);
impl Platform for P {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

/// GPU simulada que el test puede «reiniciar» desde fuera (como hace el driver al despertar).
struct Gpu {
    inner: Rc<RefCell<DemoBackend>>,
    limits: Limits,
    fail_core_sets: Rc<Cell<u32>>, // las próximas N escrituras del núcleo fallan (driver aún no listo)
}
impl GpuBackend for Gpu {
    fn name(&self) -> &str { "GPU de prueba" }
    fn driver(&self) -> &str { "0" }
    fn is_demo(&self) -> bool { true }
    fn limits(&self) -> &Limits { &self.limits }
    fn oc_source(&self) -> Option<&str> { Some("demo") }
    fn oc_error(&self) -> Option<&str> { None }
    fn read(&mut self) -> gpu_py::backend::Telemetry { self.inner.borrow_mut().read() }
    fn core_offset(&mut self) -> i32 { self.inner.borrow_mut().core_offset() }
    fn mem_offset(&mut self) -> i32 { self.inner.borrow_mut().mem_offset() }
    fn set_power_limit(&mut self, w: f64) -> HwResult<()> { self.inner.borrow_mut().set_power_limit(w) }
    fn set_core_offset(&mut self, m: i32) -> HwResult<()> {
        if self.fail_core_sets.get() > 0 {
            self.fail_core_sets.set(self.fail_core_sets.get() - 1);
            return Err(HwError::new("El driver aún no está listo."));
        }
        self.inner.borrow_mut().set_core_offset(m)
    }
    fn set_mem_offset(&mut self, m: i32) -> HwResult<()> { self.inner.borrow_mut().set_mem_offset(m) }
    fn lock_clocks(&mut self, a: u32, b: u32) -> HwResult<()> { self.inner.borrow_mut().lock_clocks(a, b) }
    fn unlock_clocks(&mut self) -> HwResult<()> { self.inner.borrow_mut().unlock_clocks() }
    fn set_fan(&mut self, p: i32) -> HwResult<()> { self.inner.borrow_mut().set_fan(p) }
    fn fan_auto(&mut self) -> HwResult<()> { self.inner.borrow_mut().fan_auto() }
}

struct SharedIllum(Rc<FakeNvapi>);
impl IllumApi for SharedIllum {
    fn gpus(&self) -> HwResult<Vec<(GpuHandle, String)>> { self.0.gpus() }
    fn get_control(&self, h: GpuHandle, p: &mut IllumParams) -> HwResult<()> { self.0.get_control(h, p) }
    fn set_control(&self, h: GpuHandle, p: &mut IllumParams) -> HwResult<()> { self.0.set_control(h, p) }
}

fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..80 {
        std::thread::sleep(Duration::from_millis(40));
        slint::platform::update_timers_and_animations();
        if cond() {
            return;
        }
    }
    panic!("no se cumplió a tiempo: {what}");
}

fn settle(ms: u64) {
    for _ in 0..(ms / 40).max(1) {
        std::thread::sleep(Duration::from_millis(40));
        slint::platform::update_timers_and_animations();
    }
}

#[test]
fn settings_are_reapplied_after_resume() {
    let dir = std::env::temp_dir().join(format!("gpupy_resume_test_{}", std::process::id()));
    let app_dir = dir.join("_base").join("GPU PY");
    std::fs::create_dir_all(&app_dir).unwrap();
    std::fs::write(app_dir.join("settings.json"), r#"{"start_minimized": false, "tray_enabled": false}"#).unwrap();
    // SAFETY: único test de este binario, un solo hilo, antes de crear nada.
    unsafe { std::env::set_var("GPU_PY_DATA_DIR", &dir) };
    let win = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(P(win.clone()))).unwrap();

    let demo = Rc::new(RefCell::new(DemoBackend::new()));
    let fail = Rc::new(Cell::new(0u32));
    let gpu = Gpu { limits: demo.borrow().limits().clone(), inner: demo.clone(), fail_core_sets: fail.clone() };
    let api = Rc::new(FakeNvapi::standard());
    let led = IllumLed::new(Some(Box::new(SharedIllum(api.clone()))), 0).unwrap();
    let app = App::new(Box::new(gpu), Some(Box::new(led)), None, false);
    let st_ui = app.ui();
    let st = st_ui.global::<State>();

    let led_red = || api.current.borrow().zones[0].data[0..3] == [255, 0, 0];
    let fan_pct = || demo.borrow_mut().read_at(1.0).fan_pct;
    let core = || demo.borrow_mut().core_offset();
    let mem = || demo.borrow_mut().mem_offset();
    // Lo que simula el driver al despertar: todo vuelve a los valores por defecto
    let driver_reset = || {
        demo.borrow_mut().reset_all();
        let mut p = IllumParams::new(true);
        api.get_control(1, &mut p).unwrap();
        api.set_control(1, &mut p).unwrap();
    };

    // 1) El usuario aplica overclock, ventilador fijo y LED rojo
    st.set_power(200);
    st.set_core(120);
    st.set_mem(300);
    st.invoke_apply_oc();
    st.set_fan_mode(1);
    st.set_fan_manual(70);
    st.invoke_apply_fan();
    st.set_led_mode(1); // color fijo
    st.set_r1(255);
    st.set_g1(0);
    st.set_b1(0);
    st.invoke_apply_led();
    assert_eq!((core(), mem()), (120, 300));
    assert_eq!(fan_pct(), 70);
    assert!(led_red());

    // 2) El PC se suspende y al despertar el driver lo restablece todo
    driver_reset();
    assert_eq!(core(), 0);
    assert!(!led_red());
    let fast = vec![Duration::from_millis(40); 3];
    app.handle_resume_with(600.0, fast.clone());
    wait_until("se reaplica el overclock", || core() == 120);
    wait_until("se reaplica el LED", led_red);
    assert_eq!(mem(), 300);
    assert_eq!(fan_pct(), 70);

    // 3) El driver aún no está listo: los dos primeros intentos fallan y el tercero funciona
    driver_reset();
    fail.set(2);
    app.handle_resume_with(600.0, fast.clone());
    wait_until("se reaplica tras reintentos", || core() == 120 && led_red());
    assert_eq!(fail.get(), 0, "debían gastarse los fallos simulados antes de acertar");

    // 4) Con el ajuste desactivado no se toca nada
    st.set_reapply_on_resume(false);
    st.invoke_settings_changed();
    driver_reset();
    app.handle_resume_with(600.0, fast);
    settle(400);
    assert_eq!(core(), 0, "con el ajuste desactivado no debe reaplicar");
    assert!(!led_red());
}
