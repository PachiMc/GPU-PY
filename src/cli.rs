//! Línea de comandos (equivale a `gpupy.py`). Sin argumentos abre la interfaz.

use crate::app::{App, show_fatal};
use crate::backend::{DemoBackend, GpuBackend, NvmlBackend};
use crate::core::{
    LedState, ProfileStore, apply_led_state, apply_profile_to_backend, i2c_scan_text, led_mut, make_led, oc_info_text,
};
use crate::{applog, autostart, install, instance};
use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, ValueEnum)]
enum Toggle {
    On,
    Off,
    Status,
}

/// GPU PY — monitor, overclock, ventiladores y LED para GPUs NVIDIA (RTX 3070 PNY XLR8 y similares).
///
/// Sin argumentos abre la interfaz (ejecuta como Administrador para aplicar cambios).
#[derive(Parser)]
#[command(name = "gpu-py", version)]
struct Cli {
    /// usar una GPU simulada
    #[arg(long)]
    demo: bool,
    /// (interno) arranque automático con Windows
    #[arg(long, hide = true)]
    startup: bool,
    /// aplicar un perfil y salir
    #[arg(long, value_name = "PERFIL")]
    apply: Option<String>,
    /// restablecer valores de fábrica y salir
    #[arg(long)]
    reset: bool,
    /// listar perfiles y salir
    #[arg(long)]
    list: bool,
    /// LED: #RRGGBB, off o default
    #[arg(long, value_name = "COLOR")]
    led: Option<String>,
    /// mostrar el controlador LED detectado
    #[arg(long)]
    led_info: bool,
    /// diagnóstico de solo lectura del LED por I2C (guarda led_report.txt)
    #[arg(long)]
    i2c_scan: bool,
    /// mostrar qué overclock permite tu GPU
    #[arg(long)]
    oc_info: bool,
    /// gestionar el inicio con Windows
    #[arg(long, value_enum, value_name = "on|off|status")]
    autostart: Option<Toggle>,
    /// copiar el .exe a una carpeta permanente y salir
    #[arg(long)]
    install: bool,
}

fn fatal(msg: &str, gui: bool) -> ! {
    applog::log(&format!("ERROR FATAL: {msg}"));
    if gui {
        show_fatal(msg);
    } else {
        eprintln!("ERROR: {msg}");
    }
    std::process::exit(1)
}

/// Abre NVML. Al iniciar sesión el driver puede tardar en estar listo: con `--startup` se reintenta hasta
/// ~35 s (aún no hay ventana, así que esperar no molesta) en vez de abortar al primer fallo.
fn open_nvml(retry: bool) -> Result<NvmlBackend, crate::error::HwError> {
    let attempts = if retry { 8 } else { 1 };
    for n in 1..=attempts {
        match NvmlBackend::new(0) {
            Ok(b) => {
                applog::log(&format!("NVML listo (intento {n})"));
                return Ok(b);
            }
            Err(e) => {
                applog::log(&format!("NVML no disponible (intento {n}/{attempts}): {e}"));
                if n == attempts {
                    return Err(e);
                }
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
        }
    }
    unreachable!("attempts >= 1")
}

pub fn run() -> i32 {
    applog::install_panic_hook();
    let a = Cli::parse();
    applog::log(&format!(
        "inicio v{} args={:?} exe={} admin={}",
        crate::core::VERSION,
        std::env::args().skip(1).collect::<Vec<_>>(),
        std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
        crate::backend::is_admin()
    ));

    if a.list {
        let names = ProfileStore::load().names();
        println!("{}", if names.is_empty() { "(sin perfiles)".to_string() } else { names.join("\n") });
        return 0;
    }

    if let Some(t) = a.autostart {
        if matches!(t, Toggle::Status) {
            println!("{}", autostart::status_report(&autostart::system_runner, autostart::is_supported()));
            return 0;
        }
        let r = match t {
            Toggle::On => autostart::enable(20, None).map(|_| "Inicio con Windows: ACTIVADO".to_string()),
            Toggle::Off => autostart::disable().map(|_| "Inicio con Windows: DESACTIVADO".to_string()),
            Toggle::Status => Ok(format!(
                "Inicio con Windows: {}",
                if autostart::is_enabled() { "ACTIVADO" } else { "desactivado" }
            )),
        };
        match r {
            Ok(m) => println!("{m}"),
            Err(e) => fatal(&e.0, false),
        }
        return 0;
    }

    if a.install {
        match install::install() {
            Ok(dst) => println!("Instalado en {}", dst.display()),
            Err(e) => fatal(&e.0, false),
        }
        return 0;
    }

    if a.i2c_scan {
        let text = i2c_scan_text(a.demo).unwrap_or_else(|e| fatal(&e.0, false));
        println!("{text}");
        let path = std::env::current_exe().map(|p| p.with_file_name("led_report.txt")).unwrap_or_default();
        match std::fs::write(&path, format!("{text}\n")) {
            Ok(()) => println!("\n(Informe guardado en {})", path.display()),
            Err(_) => {}
        }
        return 0;
    }

    let (mut led, led_err) = make_led(a.demo);

    // ---- comandos de LED independientes del resto ----
    if a.led_info || a.led.is_some() {
        let Some(led) = led.as_deref_mut() else {
            fatal(led_err.as_deref().unwrap_or("LED no disponible."), false)
        };
        if a.led_info {
            println!("{}", led.describe());
        }
        if let Some(arg) = &a.led {
            let key = arg.trim().to_lowercase();
            let st = LedState {
                mode: if key == "off" || key == "default" { key } else { "static".into() },
                color: arg.clone(),
                ..LedState::default()
            };
            match apply_led_state(led, &st) {
                Ok(()) => println!("LED OK"),
                Err(e) => fatal(&e.0, false),
            }
        }
        if a.apply.is_none() && !a.reset {
            return 0;
        }
    }

    let cli = a.apply.is_some() || a.reset || a.oc_info;
    // Una sola instancia con interfaz (la que arrancó con Windows puede estar solo en la bandeja).
    let _instance = if cli || a.demo {
        None
    } else {
        match instance::acquire() {
            Some(g) => Some(g),
            None => {
                applog::log("ya hay otra instancia en ejecución; esta se cierra");
                if !a.startup {
                    show_fatal(
                        "GPU PY ya está en ejecución.\n\nBusca su icono en la bandeja del sistema (junto al reloj; \
                         puede estar dentro de «^») y haz clic para abrir la ventana.",
                    );
                }
                return 0;
            }
        }
    };
    let mut be: Box<dyn GpuBackend> = if a.demo {
        Box::new(DemoBackend::new())
    } else {
        match open_nvml(a.startup) {
            Ok(b) => Box::new(b),
            Err(e) => fatal(&e.0, !cli),
        }
    };

    if a.oc_info {
        println!("{}", oc_info_text(be.as_mut()));
        return 0;
    }

    if cli {
        let errs = if a.reset {
            be.reset_all()
        } else {
            let name = a.apply.as_deref().unwrap_or_default();
            let store = ProfileStore::load();
            let Some(prof) = store.data.get(name) else {
                let all = store.names();
                fatal(
                    &format!(
                        "No existe el perfil «{name}». Perfiles: {}",
                        if all.is_empty() { "(ninguno)".to_string() } else { all.join(", ") }
                    ),
                    false,
                )
            };
            if prof.fan_mode == "curve" {
                println!("Nota: la curva de ventiladores solo funciona con la interfaz abierta.");
            }
            apply_profile_to_backend(be.as_mut(), prof, led_mut(&mut led))
        };
        println!("{}", if errs.is_empty() { "OK".to_string() } else { errs.join("\n") });
        return i32::from(!errs.is_empty());
    }

    let app = App::new(be, led, led_err, a.startup);
    let result = app.run();
    applog::log(&format!("cierre: {}", result.as_ref().map_or_else(|e| format!("error {e}"), |_| "normal".into())));
    if let Err(e) = result {
        eprintln!("ERROR: {e}");
        return 1;
    }
    0
}
