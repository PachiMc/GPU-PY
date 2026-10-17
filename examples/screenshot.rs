//! Renderiza cada página de la interfaz con el renderizador por software y guarda PNG.
//! Sirve para revisar el diseño sin pantalla:
//!
//!   GPUPY_SCREENSHOT=1 cargo run --example screenshot -- ./capturas

use gpu_py::app::{App, State};
use gpu_py::backend::DemoBackend;
use gpu_py::core::make_led;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize};
use std::rc::Rc;
use std::time::Duration;

struct P(Rc<MinimalSoftwareWindow>);
impl Platform for P {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

const SAMPLE_PROFILES: &str = r##"{
  "Gaming": {"power_w": 220, "core_off": 120, "mem_off": 400, "lock_on": false, "lock_mhz": 2100,
             "fan_mode": "curve", "fan_manual": 50, "curve": [[30,30],[50,40],[65,60],[75,80],[85,100]],
             "led": {"mode": "breathing", "color": "#ff2d55", "color2": "#0a84ff", "brightness": 100,
                     "period": 3000, "zones": null, "speed": 50, "reverse": false}},
  "Silencioso": {"power_w": 160, "core_off": -100, "mem_off": 0, "lock_on": true, "lock_mhz": 1700,
             "fan_mode": "auto", "fan_manual": 50, "curve": [[30,30],[50,40],[65,60],[75,80],[85,100]], "led": null}
}"##;

fn shot(win: &MinimalSoftwareWindow, path: &std::path::Path) {
    const W: usize = 1120;
    const H: usize = 760;
    let mut buf = vec![slint::Rgb8Pixel::default(); W * H];
    win.draw_if_needed(|r| {
        r.render(&mut buf, W);
    });
    let raw: Vec<u8> = buf.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    image::RgbImage::from_raw(W as u32, H as u32, raw).unwrap().save(path).unwrap();
}

fn settle() {
    for _ in 0..4 {
        std::thread::sleep(Duration::from_millis(120));
        slint::platform::update_timers_and_animations();
    }
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "capturas".into()));
    std::fs::create_dir_all(&out).unwrap();
    let data = std::env::temp_dir().join("gpupy_shots");
    let app_dir = data.join("_base").join("GPU PY");
    std::fs::create_dir_all(&app_dir).unwrap();
    std::fs::write(app_dir.join("profiles.json"), SAMPLE_PROFILES).unwrap();
    // SAFETY: programa de un solo hilo, se fija antes de crear nada.
    unsafe { std::env::set_var("GPU_PY_DATA_DIR", &data) };

    let win = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(P(win.clone()))).unwrap();
    win.set_size(PhysicalSize::new(1120, 760));

    let (led, err) = make_led(true);
    let app = App::new(Box::new(DemoBackend::new()), led, err, false);
    let ui = app.ui();
    let st = ui.global::<State>();

    for _ in 0..12 {
        std::thread::sleep(Duration::from_millis(1050)); // deja correr la telemetría
        slint::platform::update_timers_and_animations();
    }
    let pages = ["monitor", "overclock", "ventiladores", "led", "perfiles", "ajustes"];
    for (i, name) in pages.iter().enumerate() {
        st.set_page(i as i32);
        if *name == "perfiles" {
            st.invoke_select_profile(0);
        }
        if *name == "led" {
            st.set_led_mode(2);
        }
        settle();
        shot(&win, &out.join(format!("{i}_{name}.png")));
        println!("capturada {name}");
    }
    st.set_page(1);
    st.set_dialog_title("Antes de continuar".into());
    st.set_dialog_message("Modificar frecuencias, potencia o ventiladores puede causar inestabilidad.".into());
    st.set_dialog_cancel("Cancelar".into());
    st.set_dialog_visible(true);
    st.set_toast_text("Overclock aplicado correctamente.".into());
    st.set_toast_visible(true);
    settle();
    shot(&win, &out.join("6_dialogo.png"));
}
