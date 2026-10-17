//! Mide el coste de redimensionar la ventana en cada página y captura el aviso a varios tamaños.
//!   GPUPY_SCREENSHOT=1 cargo run --release --example perf -- ./capturas

use gpu_py::app::{App, State};
use gpu_py::backend::DemoBackend;
use gpu_py::core::make_led;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize};
use std::rc::Rc;
use std::time::{Duration, Instant};

struct P(Rc<MinimalSoftwareWindow>);
impl Platform for P {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn render(win: &MinimalSoftwareWindow, w: usize, h: usize) -> Vec<slint::Rgb8Pixel> {
    let mut buf = vec![slint::Rgb8Pixel::default(); w * h];
    win.draw_if_needed(|r| {
        r.render(&mut buf, w);
    });
    buf
}

fn save(buf: &[slint::Rgb8Pixel], w: u32, h: u32, path: &std::path::Path) {
    let raw: Vec<u8> = buf.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    image::RgbImage::from_raw(w, h, raw).unwrap().save(path).unwrap();
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "capturas".into()));
    std::fs::create_dir_all(&out).unwrap();
    let data = std::env::temp_dir().join("gpupy_perf");
    std::fs::create_dir_all(data.join("_base").join("GPU PY")).unwrap();
    // SAFETY: un solo hilo, antes de crear nada.
    unsafe { std::env::set_var("GPU_PY_DATA_DIR", &data) };

    let win = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(P(win.clone()))).unwrap();
    win.set_size(PhysicalSize::new(1120, 760));
    let (led, err) = make_led(true);
    let app = App::new(Box::new(DemoBackend::new()), led, err, false);
    let ui = app.ui();
    let st = ui.global::<State>();
    for _ in 0..6 {
        std::thread::sleep(Duration::from_millis(1050));
        slint::platform::update_timers_and_animations();
    }

    let sizes = [(1020u32, 700u32), (1300, 800), (1100, 900), (1500, 760), (1020, 760), (1250, 700)];
    println!("{:<14} {:>12} {:>12}", "página", "ms/fotograma", "(solo cambio de tamaño)");
    for (i, name) in ["monitor", "overclock", "ventiladores", "led", "perfiles", "ajustes"].iter().enumerate() {
        st.set_page(i as i32);
        win.set_size(PhysicalSize::new(1120, 760));
        slint::platform::update_timers_and_animations();
        let _ = render(&win, 1120, 760);
        let t = Instant::now();
        let mut n = 0;
        for _ in 0..3 {
            for (w, h) in sizes {
                win.set_size(PhysicalSize::new(w, h));
                slint::platform::update_timers_and_animations();
                let _ = render(&win, w as usize, h as usize);
                n += 1;
            }
        }
        println!("{:<14} {:>10.1} ms", name, t.elapsed().as_secs_f64() * 1000.0 / n as f64);
    }

    // Aviso a distintos tamaños, incluso después de redimensionar
    st.set_page(2);
    for (k, (w, h)) in [(1120u32, 760u32), (1020, 700), (1500, 900), (1020, 700)].into_iter().enumerate() {
        win.set_size(PhysicalSize::new(w, h));
        st.set_toast_visible(false);
        slint::platform::update_timers_and_animations();
        let _ = render(&win, w as usize, h as usize);
        st.set_toast_text(["Ventiladores en automático.", "Ventiladores fijos al 50 %.", "No se pudo guardar el perfil: Acceso denegado (os error 5) en la carpeta de datos de la aplicación", "Ajustes guardados."][k % 4].into());
        st.set_toast_kind("ok".into());
        st.set_toast_visible(true);
        for _ in 0..3 {
            std::thread::sleep(Duration::from_millis(100));
            slint::platform::update_timers_and_animations();
        }
        let buf = render(&win, w as usize, h as usize);
        save(&buf, w, h, &out.join(format!("toast_{k}_{w}x{h}.png")));
    }
    println!("listo");
}
