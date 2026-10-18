//! Arranque con Windows (`--startup`): si no se puede crear el icono de la bandeja (p. ej. el Explorador aún
//! no ha creado la barra de tareas al iniciar sesión), la aplicación NO puede quedar invisible: sin ventana y
//! sin icono no habría forma de abrirla. En Linux la bandeja nunca está disponible (sustituto), así que la
//! ventana debe mostrarse minimizada; en Windows la bandeja sí se crea y la ventana se oculta con su icono.
//! En ambos casos la app queda accesible: minimizada o en la bandeja del sistema.

use gpu_py::app::App;
use gpu_py::backend::DemoBackend;
use gpu_py::core::make_led;
use slint::ComponentHandle;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use std::rc::Rc;

struct P(Rc<MinimalSoftwareWindow>);
impl Platform for P {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

#[test]
fn startup_without_tray_never_leaves_the_app_invisible() {
    let dir = std::env::temp_dir().join(format!("gpupy_startup_test_{}", std::process::id()));
    let app_dir = dir.join("_base").join("GPU PY");
    std::fs::create_dir_all(&app_dir).unwrap();
    // Valores por defecto de la aplicación: iniciar minimizado + bandeja
    std::fs::write(app_dir.join("settings.json"), r#"{"start_minimized": true, "tray_enabled": true}"#).unwrap();
    // SAFETY: un solo hilo de prueba en este binario, antes de crear nada.
    unsafe { std::env::set_var("GPU_PY_DATA_DIR", &dir) };

    let win = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(P(win.clone()))).unwrap();

    let (led, err) = make_led(true);
    let app = App::new(Box::new(DemoBackend::new()), led, err, true /* --startup */);
    assert!(
        app.ui().window().is_visible() || app.in_tray(),
        "arranque con Windows: la ventana debe mostrarse (minimizada) o quedar en la bandeja, nunca invisible"
    );

    // Con la ventana oculta (como al irse a la bandeja) los temporizadores siguen corriendo: antes, la
    // ventana se destruía y el primer tic (`self.ui()`) abortaba el programa sin mensaje.
    let antes = app.ticks();
    app.ui().hide().unwrap();
    for _ in 0..4 {
        std::thread::sleep(std::time::Duration::from_millis(450));
        slint::platform::update_timers_and_animations(); // dispara telemetría (1 s), inicio (1,5 s) y vigilancia
    }
    assert!(
        app.ticks() > antes,
        "con la ventana oculta los temporizadores deben seguir corriendo (telemetría cada 1 s)"
    );
}
