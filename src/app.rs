//! Ventana principal: conecta la interfaz Slint (`ui/*.slint`) con el backend.
//! Equivale a `ui.py` de la versión Python.

use crate::backend::{GpuBackend, MIN_FAN_PCT, Telemetry, is_admin};
use crate::core::{
    APP_NAME, DEFAULT_CURVE, LedState, Profile, ProfileStore, SAFETY_TEMP, Settings, VERSION, apply_led_state,
    apply_profile_to_backend, data_dir, default_profile, interpolate, led_mut,
};
use crate::nvapi::{LedController, color_hex, parse_color};
use crate::applog;
use crate::power::{RESUME_GAP_SECS, ResumeDetector, now_secs};
use crate::reader::TelemetryReader;
use crate::{autostart, install};
use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

slint::include_modules!();

use crate::tray::{TrayAction, TrayController};

const HISTORY: usize = 120;
const FAN_MODES: [&str; 3] = ["auto", "manual", "curve"];
const LED_MODES: [&str; 6] = ["keep", "static", "breathing", "off", "cycle", "strobe"];
const LED_LABELS: [&str; 6] = ["No cambiar", "Color fijo", "Respiración", "Apagado", "Ciclo", "Estrobo"];

type Action = Box<dyn FnOnce(&Rc<App>)>;

/// Esperas entre intentos de reaplicar tras reanudar (acumuladas: a los 4, 12 y 30 s). El driver y el
/// controlador del LED tardan unos segundos en estar listos después de despertar.
const RESUME_DELAYS: [Duration; 3] = [Duration::from_secs(4), Duration::from_secs(8), Duration::from_secs(18)];

#[derive(Clone)]
struct OcApplied {
    power_w: f64,
    core: i32,
    mem: i32,
    lock_on: bool,
    lock_mhz: i32,
}

/// Lo último que el usuario aplicó (o que aplicó el perfil de inicio): es lo que se reaplica al reanudar.
#[derive(Clone, Default)]
struct Applied {
    oc: Option<OcApplied>,
    /// (modo, velocidad manual): "auto" | "manual" | "curve"
    fan: Option<(String, i32)>,
    led: Option<LedState>,
}

impl Applied {
    fn is_empty(&self) -> bool {
        self.oc.is_none() && self.fan.is_none() && self.led.is_none()
    }
}

struct Inner {
    be: Box<dyn GpuBackend>,
    led: Option<Box<dyn LedController>>,
    led_error: Option<String>,
    store: ProfileStore,
    settings: Settings,
    risk_ack: bool,
    fan_supported: bool,
    last_fan: Option<i32>,
    hist: [VecDeque<f64>; 3], // temperatura, uso, potencia
    in_tray: bool,
    tick: u32,
    applied: Applied,
    resume: ResumeDetector,
    reader: Option<TelemetryReader>,
    last_seq: u64,
    pending: Option<(Action, Option<Action>)>,
    zones: Rc<VecModel<bool>>,
    tray: TrayController,
}

pub struct App {
    /// Referencia FUERTE: una ventana que no está visible (arranque oculto, bandeja) se destruye si nadie la
    /// retiene, y el siguiente tic de un temporizador abortaría el programa. Los callbacks solo guardan
    /// `Weak<App>`, así que no hay ciclo de referencias.
    ui: AppWindow,
    st: RefCell<Inner>,
    toast_timer: Timer,
    poll_timer: Timer,
    watch_timer: Timer,
    startup_timer: Timer,
    resume_timer: Timer,
}

fn ss(s: impl Into<SharedString>) -> SharedString {
    s.into()
}

impl App {
    pub fn ui(&self) -> AppWindow {
        self.ui.clone_strong()
    }

    /// `true` si la ventana está oculta con su icono en la bandeja del sistema (no invisible).
    pub fn in_tray(&self) -> bool {
        self.st.borrow().in_tray
    }

    /// Número de tics del temporizador de telemetría (avanza cada segundo aunque la ventana esté oculta).
    pub fn ticks(&self) -> u32 {
        self.st.borrow().tick
    }

    fn state<'a>(ui: &'a AppWindow) -> State<'a> {
        ui.global::<State>()
    }

    // =========================================================================================
    // Construcción
    // =========================================================================================
    pub fn new(
        be: Box<dyn GpuBackend>,
        led: Option<Box<dyn LedController>>,
        led_error: Option<String>,
        startup: bool,
    ) -> Rc<Self> {
        let ui = AppWindow::new().expect("no se pudo crear la ventana");
        let fan_supported = be.limits().num_fans > 0 || be.is_demo();
        let app = Rc::new(App {
            ui: ui.clone_strong(),
            st: RefCell::new(Inner {
                be,
                led,
                led_error,
                store: ProfileStore::load(),
                settings: Settings::load(),
                risk_ack: false,
                fan_supported,
                last_fan: None,
                hist: Default::default(),
                in_tray: false,
                tick: 0,
                applied: Applied::default(),
                resume: ResumeDetector::new(RESUME_GAP_SECS),
                reader: None,
                last_seq: 0,
                pending: None,
                zones: Rc::new(VecModel::default()),
                tray: TrayController::new(),
            }),
            toast_timer: Timer::default(),
            poll_timer: Timer::default(),
            watch_timer: Timer::default(),
            startup_timer: Timer::default(),
            resume_timer: Timer::default(),
        });

        // Lecturas del driver en otro hilo (si el backend lo permite): la interfaz nunca espera a NVML.
        {
            let mut i = app.st.borrow_mut();
            if let Some(src) = i.be.telemetry_source() {
                i.reader = Some(TelemetryReader::spawn(src, Duration::from_millis(1000), Duration::from_secs(5)));
            }
        }
        app.populate_static(&ui);
        app.connect(&ui);
        app.load_current_values();
        app.refresh_profiles();
        app.refresh_curve(None);
        app.refresh_install_status();
        app.led_color_changed();
        app.start_timers();

        // Cerrar con la X: a la bandeja o salir.
        {
            let weak = Rc::downgrade(&app);
            ui.window().on_close_requested(move || {
                if let Some(app) = weak.upgrade() {
                    if app.st.borrow().settings.tray_enabled {
                        app.hide_to_tray(false);
                    } else {
                        app.exit_app();
                    }
                }
                CloseRequestResponse::HideWindow
            });
        }

        // Arranque normal o con Windows (--startup).
        applog::log(&format!(
            "ventana creada (startup={startup}, minimizado={}, bandeja={})",
            app.st.borrow().settings.start_minimized,
            app.st.borrow().settings.tray_enabled
        ));
        let hidden_start = startup && app.st.borrow().settings.start_minimized;
        if hidden_start {
            if app.st.borrow().settings.tray_enabled {
                app.hide_to_tray(true);
            } else {
                ui.show().ok();
                ui.window().set_minimized(true);
            }
        } else {
            ui.show().ok();
        }
        if !startup && autostart::is_supported() {
            // Si el inicio con Windows apunta a un archivo que ya no existe, ofrecer repararlo.
            let weak = Rc::downgrade(&app);
            app.startup_timer.start(TimerMode::SingleShot, Duration::from_millis(2500), move || {
                if let Some(a) = weak.upgrade() {
                    a.check_broken_autostart();
                }
            });
        }
        if startup {
            let weak = Rc::downgrade(&app);
            app.startup_timer.start(TimerMode::SingleShot, Duration::from_millis(1500), move || {
                if let Some(a) = weak.upgrade() {
                    a.run_startup(0, false);
                }
            });
        }
        app
    }

    /// Ejecuta la aplicación hasta que se pida salir (la ventana puede estar oculta en la bandeja).
    pub fn run(self: &Rc<Self>) -> Result<(), slint::PlatformError> {
        slint::run_event_loop_until_quit()
    }

    fn populate_static(&self, ui: &AppWindow) {
        let st = Self::state(ui);
        let inner = self.st.borrow();
        let l = inner.be.limits().clone();
        st.set_gpu_name(ss(inner.be.name().replace("NVIDIA ", "")));
        st.set_driver(ss(inner.be.driver()));
        st.set_version(ss(VERSION));
        st.set_is_admin(is_admin() || inner.be.is_demo());
        st.set_demo(inner.be.is_demo());

        st.set_g_core_max(l.max_core_clock as f32);
        st.set_g_mem_max((l.max_mem_clock + 1000) as f32);
        st.set_power_min(l.power_min_w as i32);
        st.set_power_max(l.power_max_w as i32);
        st.set_power(l.power_default_w as i32);
        st.set_core_min(l.core_off_min);
        st.set_core_max(l.core_off_max);
        st.set_mem_min(l.mem_off_min);
        st.set_mem_max(l.mem_off_max);
        st.set_lock_range_max(l.max_core_clock.max(900));
        st.set_lock_mhz(l.max_core_clock);

        if !l.oc_supported && !inner.be.is_demo() {
            st.set_oc_notice(ss(format!(
                "No se pudo acceder a los offsets de reloj (ni por NVML ni por NVAPI).\nMotivo: {}\n\
                 Ejecuta gpu-py --oc-info y envíame el resultado.",
                inner.be.oc_error().unwrap_or("desconocido")
            )));
            st.set_oc_notice_kind(ss("warn"));
        } else if inner.be.oc_source() == Some("nvapi") {
            st.set_oc_notice(ss("Overclock vía NVAPI, la misma vía que usa VelocityX."));
            st.set_oc_notice_kind(ss("ok"));
        }

        st.set_fan_supported(inner.fan_supported);
        for (i, (t, p)) in DEFAULT_CURVE.iter().enumerate() {
            Self::set_curve_row(&st, i, &t.to_string(), &p.to_string());
        }

        // LED
        let effects = inner.led.as_ref().is_some_and(|l| l.supports_effects());
        let (info, ok) = match &inner.led {
            Some(led) => (led.describe(), led.num_zones() > 0),
            None => (
                format!(
                    "LED no disponible: {}\nEjecuta gpu-py --i2c-scan y envíame el archivo led_report.txt.",
                    inner.led_error.as_deref().unwrap_or("sin información")
                ),
                false,
            ),
        };
        st.set_led_info(ss(info));
        st.set_led_ok(ok);
        st.set_led_effects(effects);
        let n = if effects { 6 } else { 4 };
        st.set_led_modes(ModelRc::new(VecModel::from(
            LED_LABELS[..n].iter().map(|s| ss(*s)).collect::<Vec<_>>(),
        )));
        let zones = inner.led.as_ref().map_or(0, |l| l.num_zones());
        st.set_led_zone_count(zones as i32);
        if zones > 1 {
            for _ in 0..zones.min(32) {
                inner.zones.push(true);
            }
        }
        st.set_led_zones(ModelRc::from(inner.zones.clone()));

        // Ajustes
        let s = &inner.settings;
        st.set_autostart_supported(autostart::is_supported());
        st.set_autostart_on(autostart::is_supported() && autostart::is_enabled());
        st.set_start_minimized(s.start_minimized);
        st.set_tray_enabled(s.tray_enabled);
        st.set_reapply_on_resume(s.reapply_on_resume);
        st.set_startup_choice(ss(s.startup_profile.clone().unwrap_or_else(|| "Ninguno".into())));
        st.set_perm_text(ss(if is_admin() || inner.be.is_demo() { "Administrador" } else { "Usuario estándar" }));
        st.set_data_dir(ss(data_dir().display().to_string()));
    }

    fn start_timers(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.poll_timer.start(TimerMode::Repeated, Duration::from_millis(1000), move || {
            if let Some(a) = weak.upgrade() {
                a.poll();
            }
        });
        // Minimizar → bandeja (Slint no avisa de la minimización, se consulta) y eventos del tray.
        let weak = Rc::downgrade(self);
        self.watch_timer.start(TimerMode::Repeated, Duration::from_millis(250), move || {
            if let Some(a) = weak.upgrade() {
                a.watch();
            }
        });
    }

    // =========================================================================================
    // Callbacks
    // =========================================================================================
    fn connect(self: &Rc<Self>, ui: &AppWindow) {
        let st = Self::state(ui);
        macro_rules! on {
            ($setter:ident, $method:ident) => {{
                let w = Rc::downgrade(self);
                st.$setter(move || {
                    if let Some(a) = w.upgrade() {
                        a.$method();
                    }
                });
            }};
        }
        on!(on_apply_oc, apply_oc);
        on!(on_reset_all, reset_all);
        on!(on_apply_fan, apply_fan);
        on!(on_curve_edited, curve_edited);
        on!(on_apply_led, apply_led);
        on!(on_led_default, led_default);
        on!(on_led_color_changed, led_color_changed);
        on!(on_save_profile, save_profile);
        on!(on_load_profile, load_profile);
        on!(on_delete_profile, delete_profile);
        on!(on_settings_changed, settings_changed);
        on!(on_autostart_toggled, autostart_toggled);
        on!(on_do_install, do_install);
        on!(on_open_data_dir, open_data_dir);
        on!(on_dialog_accept, dialog_accept);
        on!(on_dialog_dismiss, dialog_dismiss);
        {
            let w = Rc::downgrade(self);
            st.on_run_startup(move || {
                if let Some(a) = w.upgrade() {
                    a.run_startup(0, true);
                }
            });
        }
        {
            let w = Rc::downgrade(self);
            st.on_select_profile(move |i| {
                if let Some(a) = w.upgrade() {
                    a.select_profile(i as usize);
                }
            });
        }
        {
            let w = Rc::downgrade(self);
            st.on_hex_entered(move |t| {
                if let Some(a) = w.upgrade() {
                    a.hex_entered(&t);
                }
            });
        }
        {
            let w = Rc::downgrade(self);
            st.on_zone_toggled(move |i, v| {
                if let Some(a) = w.upgrade() {
                    a.st.borrow().zones.set_row_data(i as usize, v);
                }
            });
        }
        st.on_hex_of(|r, g, b| ss(color_hex((r.clamp(0, 255) as u8, g.clamp(0, 255) as u8, b.clamp(0, 255) as u8))));
    }

    // =========================================================================================
    // Avisos y diálogos
    // =========================================================================================
    fn toast(&self, msg: &str, kind: &str) {
        if self.st.borrow().in_tray {
            return; // con la ventana oculta no hay dónde mostrarlo
        }
        let ui = self.ui();
        let s = Self::state(&ui);
        s.set_toast_text(ss(msg));
        s.set_toast_kind(ss(kind));
        s.set_toast_visible(true);
        let weak = self.ui.as_weak();
        let ms = match kind {
            "error" => 7000,
            "warn" => 5500,
            _ => 3500,
        };
        self.toast_timer.start(TimerMode::SingleShot, Duration::from_millis(ms), move || {
            if let Some(ui) = weak.upgrade() {
                ui.global::<State>().set_toast_visible(false);
            }
        });
    }

    fn alert(&self, title: &str, message: &str) {
        self.show_dialog(title, message, "Entendido", "", false);
        self.st.borrow_mut().pending = None;
    }

    fn show_dialog(&self, title: &str, message: &str, ok: &str, cancel: &str, danger: bool) {
        let ui = self.ui();
        let s = Self::state(&ui);
        s.set_dialog_title(ss(title));
        s.set_dialog_message(ss(message));
        s.set_dialog_ok(ss(ok));
        s.set_dialog_cancel(ss(cancel));
        s.set_dialog_danger(danger);
        s.set_dialog_visible(true);
    }

    /// Diálogo modal asíncrono: `on_ok` se ejecuta al aceptar y `on_cancel` (si hay) al rechazar.
    fn ask(
        &self,
        title: &str,
        message: &str,
        ok: &str,
        cancel: &str,
        danger: bool,
        on_ok: impl FnOnce(&Rc<App>) + 'static,
        on_cancel: Option<Action>,
    ) {
        self.show_dialog(title, message, ok, cancel, danger);
        self.st.borrow_mut().pending = Some((Box::new(on_ok), on_cancel));
    }

    fn dialog_accept(self: &Rc<Self>) {
        Self::state(&self.ui()).set_dialog_visible(false);
        let pending = self.st.borrow_mut().pending.take();
        if let Some((ok, _)) = pending {
            ok(self);
        }
    }

    fn dialog_dismiss(self: &Rc<Self>) {
        Self::state(&self.ui()).set_dialog_visible(false);
        let pending = self.st.borrow_mut().pending.take();
        if let Some((_, Some(cancel))) = pending {
            cancel(self);
        }
    }

    // =========================================================================================
    // Estado interfaz <-> perfil
    // =========================================================================================
    fn set_curve_row(st: &State, i: usize, t: &str, p: &str) {
        let (t, p) = (ss(t), ss(p));
        match i {
            0 => (st.set_ct1(t), st.set_cp1(p)),
            1 => (st.set_ct2(t), st.set_cp2(p)),
            2 => (st.set_ct3(t), st.set_cp3(p)),
            3 => (st.set_ct4(t), st.set_cp4(p)),
            _ => (st.set_ct5(t), st.set_cp5(p)),
        };
    }

    fn curve_rows(st: &State) -> [(String, String); 5] {
        [
            (st.get_ct1().into(), st.get_cp1().into()),
            (st.get_ct2().into(), st.get_cp2().into()),
            (st.get_ct3().into(), st.get_cp3().into()),
            (st.get_ct4().into(), st.get_cp4().into()),
            (st.get_ct5().into(), st.get_cp5().into()),
        ]
    }

    /// Puntos válidos de la curva (ignora filas mal escritas), ordenados.
    pub fn get_curve(&self) -> Vec<(i32, i32)> {
        let ui = self.ui();
        let mut pts: Vec<(i32, i32)> = Self::curve_rows(&Self::state(&ui))
            .iter()
            .filter_map(|(t, p)| Some((t.trim().parse::<i32>().ok()?, p.trim().parse::<i32>().ok()?)))
            .map(|(t, p)| (t.clamp(20, 95), p.clamp(MIN_FAN_PCT, 100)))
            .collect();
        pts.sort_unstable();
        if pts.len() >= 2 { pts } else { DEFAULT_CURVE.to_vec() }
    }

    fn get_led_state(&self) -> LedState {
        let ui = self.ui();
        let s = Self::state(&ui);
        let zones = {
            let z = self.st.borrow().zones.clone();
            (z.row_count() > 0).then(|| (0..z.row_count()).filter(|&i| z.row_data(i).unwrap_or(false)).collect())
        };
        LedState {
            mode: LED_MODES.get(s.get_led_mode().max(0) as usize).unwrap_or(&"keep").to_string(),
            color: color_hex((s.get_r1() as u8, s.get_g1() as u8, s.get_b1() as u8)),
            color2: color_hex((s.get_r2() as u8, s.get_g2() as u8, s.get_b2() as u8)),
            brightness: s.get_led_bright(),
            period: s.get_led_period(),
            zones,
            speed: s.get_led_speed(),
            reverse: s.get_led_reverse(),
        }
    }

    fn set_led_state(&self, led: &LedState) {
        let ui = self.ui();
        let s = Self::state(&ui);
        let idx = LED_MODES.iter().position(|m| *m == led.mode).unwrap_or(0);
        s.set_led_mode(idx as i32);
        let (r1, g1, b1) = parse_color(&led.color).unwrap_or((255, 45, 85));
        let (r2, g2, b2) = parse_color(&led.color2).unwrap_or((0, 0, 0));
        s.set_r1(r1.into());
        s.set_g1(g1.into());
        s.set_b1(b1.into());
        s.set_r2(r2.into());
        s.set_g2(g2.into());
        s.set_b2(b2.into());
        s.set_led_bright(led.brightness.clamp(0, 100));
        s.set_led_period(led.period.clamp(1000, 10000));
        s.set_led_speed(led.speed.clamp(1, 100));
        s.set_led_reverse(led.reverse);
        self.led_color_changed();
    }

    pub fn get_state(&self) -> Profile {
        let ui = self.ui();
        let s = Self::state(&ui);
        Profile {
            power_w: s.get_power() as f64,
            core_off: s.get_core(),
            mem_off: s.get_mem(),
            lock_on: s.get_lock_on(),
            lock_mhz: s.get_lock_mhz(),
            fan_mode: FAN_MODES[s.get_fan_mode().clamp(0, 2) as usize].into(),
            fan_manual: s.get_fan_manual(),
            curve: self.get_curve(),
            led: Some(self.get_led_state()),
        }
    }

    pub fn set_state(&self, p: &Profile) {
        {
            let ui = self.ui();
            let s = Self::state(&ui);
            s.set_power(p.power_w as i32);
            s.set_core(p.core_off);
            s.set_mem(p.mem_off);
            s.set_lock_on(p.lock_on);
            s.set_lock_mhz(p.lock_mhz);
            s.set_fan_mode(FAN_MODES.iter().position(|m| *m == p.fan_mode).unwrap_or(0) as i32);
            s.set_fan_manual(p.fan_manual);
            for (i, (t, pc)) in p.curve.iter().take(5).enumerate() {
                Self::set_curve_row(&s, i, &t.to_string(), &pc.to_string());
            }
        }
        self.set_led_state(p.led.as_ref().unwrap_or(&LedState::default()));
        self.refresh_curve(None);
    }

    fn load_current_values(&self) {
        let (core, mem) = {
            let mut i = self.st.borrow_mut();
            (i.be.core_offset(), i.be.mem_offset())
        };
        let ui = self.ui();
        let s = Self::state(&ui);
        s.set_core(core);
        s.set_mem(mem);
    }

    // =========================================================================================
    // Acciones: overclock / ventiladores
    // =========================================================================================
    fn confirm_risk(self: &Rc<Self>, then: impl FnOnce(&Rc<App>) + 'static) {
        {
            let i = self.st.borrow();
            if i.risk_ack || i.be.is_demo() {
                drop(i);
                then(self);
                return;
            }
        }
        self.ask(
            "Antes de continuar",
            "Modificar frecuencias, potencia o ventiladores puede causar inestabilidad (pantallazos, cierres \
             de juegos) y, si se abusa, reducir la vida útil de la GPU.\n\nHazlo de forma gradual y vigila las \
             temperaturas. Reiniciar el PC revierte estos cambios.",
            "Entiendo, continuar",
            "Cancelar",
            false,
            move |app| {
                app.st.borrow_mut().risk_ack = true;
                then(app);
            },
            None,
        );
    }

    fn apply_oc(self: &Rc<Self>) {
        self.confirm_risk(|app| app.do_apply_oc(false));
    }

    fn do_apply_oc(&self, silent: bool) {
        let mut p = self.get_state();
        p.fan_mode = "keep".into(); // esta página no toca ventiladores ni LED
        p.led = None;
        self.st.borrow_mut().applied.oc = Some(OcApplied {
            power_w: p.power_w,
            core: p.core_off,
            mem: p.mem_off,
            lock_on: p.lock_on,
            lock_mhz: p.lock_mhz,
        });
        let errs = {
            let mut i = self.st.borrow_mut();
            apply_profile_to_backend(i.be.as_mut(), &p, None)
        };
        if !errs.is_empty() {
            self.alert("Algunos ajustes no se aplicaron", &errs.join("\n\n"));
        } else if !silent {
            self.toast("Overclock aplicado correctamente.", "ok");
        }
    }

    fn apply_fan(self: &Rc<Self>) {
        self.confirm_risk(|app| app.do_apply_fan(false));
    }

    fn do_apply_fan(&self, silent: bool) {
        self.st.borrow_mut().last_fan = None;
        let ui = self.ui();
        let s = Self::state(&ui);
        let manual = s.get_fan_manual();
        self.st.borrow_mut().applied.fan = Some((FAN_MODES[s.get_fan_mode().clamp(0, 2) as usize].to_string(), manual));
        let result = {
            let mut i = self.st.borrow_mut();
            match s.get_fan_mode() {
                0 => i.be.fan_auto().map(|_| "Ventiladores en automático.".to_string()),
                1 => i.be.set_fan(manual).map(|_| format!("Ventiladores fijos al {manual} %.")),
                _ => Ok("Curva activa mientras GPU PY esté abierto.".to_string()),
            }
        };
        match result {
            Ok(msg) if !silent => self.toast(&msg, "ok"),
            Ok(_) => {}
            Err(e) => self.fan_failed(&e.0),
        }
    }

    fn fan_failed(&self, msg: &str) {
        self.st.borrow_mut().applied.fan = None; // no reaplicar algo que el driver rechaza
        let ui = self.ui();
        let s = Self::state(&ui);
        s.set_fan_mode(0);
        s.set_fan_supported(false);
        self.st.borrow_mut().fan_supported = false;
        self.alert("Ventiladores", &format!("{msg}\n\nEn muchas GeForce con Windows el driver no permite este control."));
    }

    fn reset_all(&self) {
        let (errs, profile) = {
            let mut i = self.st.borrow_mut();
            let errs = i.be.reset_all();
            i.last_fan = None;
            i.applied.oc = None;
            i.applied.fan = None;
            (errs, default_profile(i.be.as_ref()))
        };
        self.set_state(&profile);
        if errs.is_empty() {
            self.toast("Todo restablecido a valores de fábrica.", "ok");
        } else {
            self.toast("Restablecido (algunos ajustes no son compatibles).", "warn");
        }
    }

    // =========================================================================================
    // Reanudar tras suspender / hibernar
    // =========================================================================================
    /// Anota lo que aplica un perfil (perfil de inicio) para poder reaplicarlo.
    fn remember_profile(&self, p: &Profile) {
        let mut i = self.st.borrow_mut();
        i.applied.oc = Some(OcApplied {
            power_w: p.power_w,
            core: p.core_off,
            mem: p.mem_off,
            lock_on: p.lock_on,
            lock_mhz: p.lock_mhz,
        });
        i.applied.fan = matches!(p.fan_mode.as_str(), "auto" | "manual" | "curve").then(|| (p.fan_mode.clone(), p.fan_manual));
        i.applied.led = p.led.clone().filter(|l| !matches!(l.mode.as_str(), "keep" | "default"));
    }

    /// El PC acaba de despertar: el driver restableció overclock, ventiladores y LED.
    pub fn handle_resume(self: &Rc<Self>, gap_secs: f64) {
        self.handle_resume_with(gap_secs, RESUME_DELAYS.to_vec());
    }

    pub fn handle_resume_with(self: &Rc<Self>, gap_secs: f64, delays: Vec<Duration>) {
        let (enabled, nothing) = {
            let mut i = self.st.borrow_mut();
            i.last_fan = None; // la curva de ventiladores volverá a fijar la velocidad en el siguiente tic
            (i.settings.reapply_on_resume, i.applied.is_empty())
        };
        applog::log(&format!("reanudación detectada (~{gap_secs:.0} s dormido)"));
        if !enabled {
            applog::log("reaplicar tras reanudar: desactivado en Ajustes");
        } else if nothing {
            applog::log("reaplicar tras reanudar: no hay nada aplicado que restaurar");
        } else {
            self.schedule_reapply(0, delays);
        }
    }

    fn schedule_reapply(self: &Rc<Self>, attempt: usize, delays: Vec<Duration>) {
        let Some(wait) = delays.get(attempt).copied() else {
            applog::log("reaplicar tras reanudar: se agotaron los intentos");
            self.toast("No se pudieron reaplicar los ajustes tras reanudar. Aplícalos de nuevo.", "warn");
            return;
        };
        let weak = Rc::downgrade(self);
        self.resume_timer.start(TimerMode::SingleShot, wait, move || {
            let Some(app) = weak.upgrade() else { return };
            let errs = app.reapply_now();
            if errs.is_empty() {
                applog::log(&format!("reaplicar tras reanudar: correcto (intento {})", attempt + 1));
                app.toast("Ajustes reaplicados tras reanudar.", "ok");
            } else {
                applog::log(&format!("reaplicar tras reanudar: intento {} con errores: {}", attempt + 1, errs.join(" | ")));
                app.schedule_reapply(attempt + 1, delays.clone());
            }
        });
    }

    /// Vuelve a aplicar lo último aplicado y comprueba los offsets. Devuelve los errores (vacío = todo bien).
    fn reapply_now(&self) -> Vec<String> {
        let mut i = self.st.borrow_mut();
        let applied = i.applied.clone();
        let mut errs = Vec::new();

        if let Some(oc) = &applied.oc {
            let p = Profile {
                power_w: oc.power_w,
                core_off: oc.core,
                mem_off: oc.mem,
                lock_on: oc.lock_on,
                lock_mhz: oc.lock_mhz,
                fan_mode: "keep".into(),
                led: None,
                ..Profile::default()
            };
            errs.extend(apply_profile_to_backend(i.be.as_mut(), &p, None));
            // Comprobación: el driver puede aceptar la orden y no quedarse con el valor.
            let l = i.be.limits().clone();
            if l.oc_supported {
                let (want_core, want_mem) =
                    (l.core_off_min.max(l.core_off_max.min(oc.core)), l.mem_off_min.max(l.mem_off_max.min(oc.mem)));
                let (got_core, got_mem) = (i.be.core_offset(), i.be.mem_offset());
                if got_core != want_core {
                    errs.push(format!("offset del núcleo: {got_core} MHz en lugar de {want_core} MHz"));
                }
                if got_mem != want_mem {
                    errs.push(format!("offset de la memoria: {got_mem} MHz en lugar de {want_mem} MHz"));
                }
            }
        }

        if let Some((mode, manual)) = &applied.fan {
            let r = match mode.as_str() {
                "manual" => i.be.set_fan(*manual),
                "auto" => i.be.fan_auto(),
                _ => Ok(()), // curva: la fija el bucle de telemetría
            };
            if let Err(e) = r {
                errs.push(format!("ventilador: {e}"));
            }
        }

        if let Some(st) = &applied.led {
            // Al despertar NVAPI/I2C puede tardar en responder: si no había controlador, se vuelve a buscar.
            if i.led.is_none() && !i.be.is_demo() {
                let (led, err) = crate::core::make_led(false);
                i.led = led;
                i.led_error = err;
            }
            match i.led.as_deref_mut() {
                Some(led) => {
                    if let Err(e) = apply_led_state(led, st) {
                        errs.push(format!("LED: {e}"));
                    }
                }
                None => errs.push("LED: controlador no disponible".into()),
            }
        }
        errs
    }

    // ---- curva ----
    fn curve_edited(&self) {
        self.refresh_curve(None);
    }

    fn refresh_curve(&self, marker_temp: Option<f64>) {
        // Mismos márgenes que las etiquetas de ejes de ui/pages.slint (lienzo 440×250)
        let (l, r, t, b) = (34.0, 428.0, 8.0, 226.0);
        let x = |temp: f64| l + (r - l) * (temp - 20.0) / 75.0;
        let y = |pct: f64| b - (b - t) * pct / 100.0;
        let pts = self.get_curve();

        let mut grid = String::new();
        for pct in [0.0, 25.0, 50.0, 75.0, 100.0] {
            grid += &format!("M{l:.1} {:.1} L{r:.1} {:.1} ", y(pct), y(pct));
        }
        let mut line = format!("M{:.1} {:.1}", x(20.0), y(pts[0].1 as f64));
        let mut dots = String::new();
        for (tp, pc) in &pts {
            let (px, py) = (x(*tp as f64), y(*pc as f64));
            line += &format!(" L{px:.1} {py:.1}");
            dots += &format!("M{:.1} {py:.1} A4.5 4.5 0 1 0 {:.1} {py:.1} A4.5 4.5 0 1 0 {:.1} {py:.1} Z ", px - 4.5, px + 4.5, px - 4.5);
        }
        line += &format!(" L{:.1} {:.1}", x(95.0), y(pts[pts.len() - 1].1 as f64));

        let ui = self.ui();
        let s = Self::state(&ui);
        s.set_curve_grid(ss(grid));
        s.set_curve_line(ss(line));
        s.set_curve_points(ss(dots));
        s.set_safety_x(x(SAFETY_TEMP) as f32);
        s.set_marker_x(marker_temp.map_or(-1.0, |t| x(t.clamp(20.0, 95.0)) as f32));
    }

    // =========================================================================================
    // LED
    // =========================================================================================
    fn led_color_changed(&self) {
        let ui = self.ui();
        let s = Self::state(&ui);
        let (r, g, b) = if s.get_led_edit() == 0 { (s.get_r1(), s.get_g1(), s.get_b1()) } else { (s.get_r2(), s.get_g2(), s.get_b2()) };
        s.set_hex_edit(ss(color_hex((r as u8, g as u8, b as u8))));
    }

    fn hex_entered(&self, text: &str) {
        let Ok((r, g, b)) = parse_color(text) else { return };
        let ui = self.ui();
        let s = Self::state(&ui);
        if s.get_led_edit() == 0 {
            (s.set_r1(r.into()), s.set_g1(g.into()), s.set_b1(b.into()));
        } else {
            (s.set_r2(r.into()), s.set_g2(g.into()), s.set_b2(b.into()));
        }
    }

    fn apply_led(&self) {
        let st = self.get_led_state();
        let mut i = self.st.borrow_mut();
        let Some(led) = i.led.as_deref_mut() else {
            let msg = format!("El control de LED no está disponible:\n{}", i.led_error.clone().unwrap_or_default());
            drop(i);
            return self.alert("LED", &msg);
        };
        if st.mode == "keep" {
            drop(i);
            return self.toast("Elige un efecto distinto de «No cambiar» para aplicar.", "warn");
        }
        if st.zones.as_ref().is_some_and(Vec::is_empty) {
            drop(i);
            return self.toast("Selecciona al menos una zona.", "warn");
        }
        let r = apply_led_state(led, &st);
        if r.is_ok() {
            i.applied.led = Some(st);
        }
        drop(i);
        match r {
            Ok(()) => self.toast("LED aplicado.", "ok"),
            Err(e) => self.alert("LED", &e.0),
        }
    }

    fn led_default(&self) {
        let mut i = self.st.borrow_mut();
        let Some(led) = i.led.as_deref_mut() else {
            drop(i);
            return self.apply_led();
        };
        let r = led.restore_default();
        if r.is_ok() {
            i.applied.led = None; // el usuario pidió el color original: no reaplicar nada
        }
        drop(i);
        match r {
            Ok(()) => self.toast("LED restaurado a los valores originales.", "ok"),
            Err(e) => self.alert("LED", &e.0),
        }
    }

    // =========================================================================================
    // Perfiles
    // =========================================================================================
    fn describe_profile(p: &Profile) -> String {
        let led = p.led.as_ref().map_or("keep", |l| l.mode.as_str());
        let led_label = LED_MODES.iter().position(|m| *m == led).map_or(led, |i| LED_LABELS[i]);
        let fan = match p.fan_mode.as_str() {
            "manual" => format!("fijo {} %", p.fan_manual),
            "curve" => "curva".to_string(),
            _ => "automático".to_string(),
        };
        let lock = if p.lock_on { format!(" · techo {} MHz", p.lock_mhz) } else { String::new() };
        format!(
            "Potencia {} W\nNúcleo {:+} MHz · Memoria {:+} MHz{lock}\nVentiladores: {fan}\nLED: {led_label}",
            p.power_w as i32, p.core_off, p.mem_off
        )
    }

    fn refresh_profiles(&self) {
        let ui = self.ui();
        let s = Self::state(&ui);
        let names = self.st.borrow().store.names();
        s.set_profiles(ModelRc::new(VecModel::from(names.iter().map(|n| ss(n.as_str())).collect::<Vec<_>>())));
        let mut startup = vec![ss("Ninguno")];
        startup.extend(names.iter().map(|n| ss(n.as_str())));
        s.set_startup_profiles(ModelRc::new(VecModel::from(startup)));
    }

    fn selected_profile(&self) -> Option<String> {
        let ui = self.ui();
        let i = Self::state(&ui).get_profile_sel();
        let names = self.st.borrow().store.names();
        usize::try_from(i).ok().and_then(|i| names.get(i).cloned())
    }

    fn select_profile(&self, index: usize) {
        let Some(name) = self.st.borrow().store.names().get(index).cloned() else { return };
        let info = Self::describe_profile(&self.st.borrow().store.data[&name]);
        let ui = self.ui();
        let s = Self::state(&ui);
        s.set_profile_sel(index as i32);
        s.set_pname(ss(name));
        s.set_profile_info(ss(info));
    }

    fn save_profile(&self) {
        let ui = self.ui();
        let typed = Self::state(&ui).get_pname().trim().to_string();
        let name = if typed.is_empty() { self.selected_profile().unwrap_or_default() } else { typed };
        if name.is_empty() {
            return self.toast("Escribe un nombre para el perfil.", "warn");
        }
        let profile = self.get_state();
        let saved = {
            let mut i = self.st.borrow_mut();
            i.store.data.insert(name.clone(), profile);
            i.store.save()
        };
        self.refresh_profiles();
        if let Some(idx) = self.st.borrow().store.names().iter().position(|n| *n == name) {
            self.select_profile(idx);
        }
        match saved {
            Ok(()) => self.toast(&format!("Perfil «{name}» guardado."), "ok"),
            Err(e) => self.toast(&format!("No se pudo guardar el perfil: {e}"), "error"),
        }
    }

    fn load_profile(self: &Rc<Self>) {
        let Some(name) = self.selected_profile() else {
            return self.toast("Selecciona un perfil de la lista.", "warn");
        };
        let Some(profile) = self.st.borrow().store.data.get(&name).cloned() else {
            return self.toast("Selecciona un perfil de la lista.", "warn");
        };
        self.set_state(&profile);
        self.confirm_risk(move |app| {
            app.do_apply_oc(true);
            app.do_apply_fan(true);
            let has_led = app.st.borrow().led.is_some();
            if has_led && app.get_led_state().mode != "keep" {
                app.apply_led();
            }
            app.toast(&format!("Perfil «{name}» aplicado."), "ok");
        });
    }

    fn delete_profile(self: &Rc<Self>) {
        let Some(name) = self.selected_profile() else { return };
        self.ask(
            "Eliminar perfil",
            &format!("¿Eliminar el perfil «{name}»? No se puede deshacer."),
            "Eliminar",
            "Cancelar",
            true,
            move |app| {
                {
                    let mut i = app.st.borrow_mut();
                    i.store.data.remove(&name);
                    let _ = i.store.save();
                    if i.settings.startup_profile.as_deref() == Some(name.as_str()) {
                        i.settings.startup_profile = None;
                        let _ = i.settings.save();
                        Self::state(&app.ui()).set_startup_choice(ss("Ninguno"));
                    }
                }
                let ui = app.ui();
                let s = Self::state(&ui);
                s.set_profile_sel(-1);
                s.set_profile_info(ss("Selecciona un perfil de la lista."));
                app.refresh_profiles();
            },
            None,
        );
    }

    // =========================================================================================
    // Ajustes / inicio con Windows / instalación
    // =========================================================================================
    fn settings_changed(&self) {
        let ui = self.ui();
        let s = Self::state(&ui);
        let choice = s.get_startup_choice().to_string();
        let leave_tray = {
            let mut i = self.st.borrow_mut();
            i.settings.start_minimized = s.get_start_minimized();
            i.settings.tray_enabled = s.get_tray_enabled();
            i.settings.reapply_on_resume = s.get_reapply_on_resume();
            i.settings.startup_profile = (choice != "Ninguno").then_some(choice);
            if let Err(e) = i.settings.save() {
                drop(i);
                return self.toast(&format!("No se pudieron guardar los ajustes: {e}"), "error");
            }
            !i.settings.tray_enabled && i.in_tray
        };
        if leave_tray {
            self.show_from_tray(); // no quedarse oculto sin icono
        }
        self.toast("Ajustes guardados.", "ok");
    }

    fn autostart_toggled(self: &Rc<Self>) {
        let want = Self::state(&self.ui()).get_autostart_on();
        if want && install::should_suggest_install() {
            self.ask(
                "Ubicación temporal",
                &format!(
                    "Estás ejecutando GPU PY desde:\n{}\n\nSi esa carpeta se borra, el inicio automático dejará de \
                     funcionar. ¿Lo instalo en una carpeta permanente ({})?",
                    install::current_exe().parent().map(|p| p.display().to_string()).unwrap_or_default(),
                    install::install_dir().display()
                ),
                "Instalar y activar",
                "Solo activar",
                false,
                |app| match install::install() {
                    Ok(dst) => match autostart::enable(20, Some(&dst)) {
                        Ok(()) => {
                            app.refresh_install_status();
                            app.toast(&format!("Instalado en {}. Ya puedes borrar la descarga.", dst.display()), "ok");
                        }
                        Err(e) => app.autostart_failed(true, &e.0),
                    },
                    Err(e) => {
                        Self::state(&app.ui()).set_autostart_on(false);
                        app.alert("Instalación", &e.0);
                    }
                },
                Some(Box::new(|app| match autostart::enable(20, None) {
                    Ok(()) => app.toast("GPU PY se iniciará con Windows.", "ok"),
                    Err(e) => app.autostart_failed(true, &e.0),
                })),
            );
            return;
        }
        let r = if want { autostart::enable(20, None) } else { autostart::disable() };
        match r {
            Ok(()) => self.toast(
                if want { "GPU PY se iniciará con Windows." } else { "Inicio con Windows desactivado." },
                "ok",
            ),
            Err(e) => self.autostart_failed(want, &e.0),
        }
    }

    fn check_broken_autostart(&self) {
        let Some(cmd) = autostart::task_command() else { return };
        if !autostart::task_is_broken(&cmd) {
            return;
        }
        applog::log(&format!("el inicio con Windows apunta a un archivo inexistente: {cmd}"));
        self.ask(
            "El inicio con Windows está roto",
            &format!(
                "La tarea de inicio apunta a:\n{cmd}\n\nese archivo ya no existe, por eso GPU PY no se abre al iniciar \
                 Windows. ¿La repito para usar este ejecutable?"
            ),
            "Reparar",
            "Ahora no",
            false,
            |app| match autostart::enable(20, None) {
                Ok(()) => app.toast("Inicio con Windows reparado.", "ok"),
                Err(e) => app.alert("Inicio con Windows", &e.0),
            },
            None,
        );
    }

    fn autostart_failed(&self, wanted: bool, msg: &str) {
        Self::state(&self.ui()).set_autostart_on(!wanted);
        self.alert("Inicio con Windows", msg);
    }

    fn refresh_install_status(&self) {
        let ui = self.ui();
        let s = Self::state(&ui);
        let (text, enabled) = if !install::is_frozen() {
            ("Ejecutando una compilación de desarrollo (no aplica instalación).".to_string(), false)
        } else if install::is_installed() {
            (format!("Instalado en:\n{}", install::installed_exe().display()), false)
        } else {
            (
                format!(
                    "Portable, ejecutándose desde:\n{}\nSe instalará en:\n{}",
                    install::current_exe().display(),
                    install::installed_exe().display()
                ),
                true,
            )
        };
        s.set_install_status(ss(text));
        s.set_install_enabled(enabled);
    }

    fn do_install(&self) {
        match install::install() {
            Ok(dst) => {
                if autostart::is_enabled() {
                    if let Err(e) = autostart::enable(20, Some(&dst)) {
                        self.alert("Inicio con Windows", &e.0);
                    }
                }
                self.refresh_install_status();
                self.toast(&format!("Instalado en {}. Ya puedes borrar la descarga.", dst.display()), "ok");
            }
            Err(e) => self.alert("Instalación", &e.0),
        }
    }

    fn open_data_dir(&self) {
        let path = data_dir();
        let opener = if cfg!(windows) { "explorer" } else { "xdg-open" };
        if std::process::Command::new(opener).arg(&path).spawn().is_err() {
            self.toast(&path.display().to_string(), "info");
        }
    }

    /// Aplica el perfil de inicio (con reintentos: el driver puede tardar en estar listo).
    fn run_startup(self: &Rc<Self>, attempt: u32, manual: bool) {
        let profile = {
            let i = self.st.borrow();
            i.settings.startup_profile.as_ref().and_then(|n| i.store.data.get(n).map(|p| (n.clone(), p.clone())))
        };
        let Some((name, prof)) = profile else {
            applog::log("perfil de inicio: ninguno configurado");
            if manual {
                self.toast("Elige primero un perfil de inicio.", "warn");
            }
            return;
        };
        applog::log(&format!("perfil de inicio «{name}»: intento {}", attempt + 1));
        self.set_state(&prof);
        self.remember_profile(&prof);
        let errs = {
            let mut i = self.st.borrow_mut();
            // Al iniciar sesión NVAPI puede no estar listo aún: si el LED no se detectó, se vuelve a intentar.
            if i.led.is_none() && !i.be.is_demo() && attempt > 0 {
                let (led, err) = crate::core::make_led(false);
                i.led = led;
                i.led_error = err;
            }
            let Inner { be, led, .. } = &mut *i;
            apply_profile_to_backend(be.as_mut(), &prof, led_mut(led))
        };
        if !errs.is_empty() && attempt < 3 && !manual {
            applog::log(&format!("perfil de inicio con errores, se reintenta en 8 s: {}", errs.join(" | ")));
            let weak = Rc::downgrade(self);
            self.startup_timer.start(TimerMode::SingleShot, Duration::from_secs(8), move || {
                if let Some(a) = weak.upgrade() {
                    a.run_startup(attempt + 1, false);
                }
            });
            return;
        }
        if errs.is_empty() {
            applog::log(&format!("perfil de inicio «{name}» aplicado"));
            self.toast(&format!("Perfil «{name}» aplicado."), "ok");
        } else {
            applog::log(&format!("perfil de inicio aplicado con errores: {}", errs.join(" | ")));
            self.toast(&format!("Perfil aplicado con errores: {}", errs[0]), "warn");
        }
    }

    // =========================================================================================
    // Bandeja del sistema
    // =========================================================================================
    fn hide_to_tray(&self, silent: bool) {
        let _ = silent;
        let tray_on = self.st.borrow().settings.tray_enabled;
        if tray_on && self.st.borrow_mut().tray.start() {
            self.set_in_tray(true);
            self.ui().hide().ok();
            applog::log("bandeja: ventana oculta, icono en la bandeja del sistema");
            return;
        }
        // Sin bandeja (desactivada o aún no disponible): NUNCA dejar la aplicación invisible. Si la ventana no
        // llegó a mostrarse (arranque con Windows) se muestra minimizada, con su botón en la barra de tareas.
        // Mientras siga minimizada, `watch()` reintenta crear el icono cada 2 s y entonces la oculta.
        let ui = self.ui();
        if !ui.window().is_visible() {
            ui.show().ok();
            applog::log(if tray_on {
                "bandeja: no disponible todavía; ventana minimizada en la barra de tareas (se reintentará)"
            } else {
                "ventana minimizada en la barra de tareas"
            });
        }
        ui.window().set_minimized(true);
    }

    fn set_in_tray(&self, v: bool) {
        let mut i = self.st.borrow_mut();
        i.in_tray = v;
        if let Some(r) = &i.reader {
            r.set_slow(v); // en la bandeja: una lectura cada 5 s
        }
    }

    fn show_from_tray(&self) {
        self.st.borrow_mut().tray.stop();
        self.set_in_tray(false);
        let ui = self.ui();
        ui.show().ok();
        ui.window().set_minimized(false);
    }

    /// Cada 250 ms: eventos del tray y detección de "minimizar".
    fn watch(self: &Rc<Self>) {
        let gap = self.st.borrow_mut().resume.tick(now_secs());
        if let Some(gap) = gap {
            self.handle_resume(gap);
        }
        let action = self.st.borrow().tray.poll();
        match action {
            Some(TrayAction::Show) => self.show_from_tray(),
            Some(TrayAction::ApplyProfile) => self.run_startup(0, true),
            Some(TrayAction::Quit) => self.exit_app(),
            None => {}
        }
        let (tray_on, in_tray) = {
            let i = self.st.borrow();
            (i.settings.tray_enabled, i.in_tray)
        };
        if tray_on && !in_tray && self.ui.window().is_minimized() {
            self.hide_to_tray(false);
        }
    }

    pub fn exit_app(&self) {
        {
            let mut i = self.st.borrow_mut();
            let manual = self.ui.global::<State>().get_fan_mode() != 0;
            if manual {
                let _ = i.be.fan_auto(); // nunca dejes el ventilador en manual
            }
            i.tray.stop();
        }
        self.st.borrow_mut().reader = None; // detiene el hilo lector
        self.poll_timer.stop();
        self.watch_timer.stop();
        let _ = slint::quit_event_loop();
    }

    // =========================================================================================
    // Telemetría
    // =========================================================================================
    fn poll(&self) {
        let hidden = {
            let mut i = self.st.borrow_mut();
            i.tick = i.tick.wrapping_add(1);
            i.in_tray
        };
        // En la bandeja: bajo consumo, una lectura cada 5 s.
        if hidden && self.st.borrow().tick % 5 != 0 {
            return;
        }
        let (t, default_w) = {
            let mut i = self.st.borrow_mut();
            let default_w = i.be.limits().power_default_w;
            let t = match i.reader.as_ref().map(|r| r.latest()) {
                Some(Some((seq, t))) => {
                    if seq == i.last_seq {
                        return; // aún no hay muestra nueva
                    }
                    i.last_seq = seq;
                    t
                }
                Some(None) => return, // el hilo aún no ha leído nada
                None => i.be.read(),
            };
            i.hist[0].push_back(t.temp);
            i.hist[1].push_back(t.util_gpu as f64);
            i.hist[2].push_back(100.0 * t.power_w / default_w.max(1.0));
            for h in i.hist.iter_mut() {
                while h.len() > HISTORY {
                    h.pop_front();
                }
            }
            (t, default_w)
        };

        if !hidden {
            self.show_telemetry(&t, default_w);
        }

        // La curva sigue activa aunque la ventana esté en la bandeja.
        let (curve_mode, fan_supported) = {
            let ui = self.ui();
            (Self::state(&ui).get_fan_mode() == 2, self.st.borrow().fan_supported)
        };
        if curve_mode && fan_supported {
            let target = if t.temp >= SAFETY_TEMP { 100 } else { interpolate(&self.get_curve(), t.temp) };
            let last = self.st.borrow().last_fan;
            if last.is_none_or(|l| (target - l).abs() >= 3) || target == 100 {
                let r = self.st.borrow_mut().be.set_fan(target);
                match r {
                    Ok(()) => self.st.borrow_mut().last_fan = Some(target),
                    Err(e) => self.fan_failed(&e.0),
                }
            }
        }
        if !hidden && Self::state(&self.ui()).get_page() == 2 {
            self.refresh_curve(Some(t.temp));
        }
    }

    fn show_telemetry(&self, t: &Telemetry, default_w: f64) {
        let ui = self.ui();
        let s = Self::state(&ui);
        s.set_g_temp(t.temp as f32);
        s.set_g_core(t.core_clock as f32);
        s.set_g_mem(t.mem_clock as f32);
        s.set_g_fan(t.fan_pct as f32);
        let limit = match s.get_power() {
            0 => default_w as i32,
            p => p,
        };
        s.set_stat_power(ss(format!("{:.0} W / {} W", t.power_w, limit)));
        s.set_stat_util(ss(format!("{} %", t.util_gpu)));
        s.set_stat_vram(ss(format!("{} / {} MB", t.vram_used_mb, t.vram_total_mb)));
        s.set_bar_power((t.power_w / limit.max(1) as f64).min(1.0) as f32);
        s.set_bar_util(t.util_gpu as f32 / 100.0);
        s.set_bar_vram((t.vram_used_mb as f64 / t.vram_total_mb.max(1) as f64) as f32);
        s.set_throttled(!t.throttle.is_empty());
        s.set_throttle(ss(if t.throttle.is_empty() { "Sin limitaciones" } else { t.throttle.as_str() }));

        if s.get_page() == 0 {
            let i = self.st.borrow();
            let paths = chart_paths(&i.hist);
            s.set_hist_grid(ss(paths.grid.clone()));
            s.set_hist_temp(ss(paths.lines[0].clone()));
            s.set_hist_util(ss(paths.lines[1].clone()));
            s.set_hist_power(ss(paths.lines[2].clone()));
            s.set_hist_temp_fill(ss(paths.temp_fill));
        }
    }
}

/// Comandos SVG de la gráfica de historial.
pub struct ChartPaths {
    pub grid: String,
    /// temperatura, uso, potencia
    pub lines: [String; 3],
    /// relleno bajo la temperatura (el único; cada relleno extra se vuelve a teselar en cada fotograma)
    pub temp_fill: String,
}

/// Lienzo de 520×190, escala fija 0–100. Se dibujan como mucho ~60 puntos por serie (la mitad del
/// historial): visualmente idéntico y la mitad de trabajo para el renderizador en cada fotograma.
pub fn chart_paths(hist: &[VecDeque<f64>; 3]) -> ChartPaths {
    const W: f64 = 520.0;
    const H: f64 = 190.0;
    let (l, r, t, b) = (34.0, W - 8.0, 8.0, H - 10.0);
    let y = |v: f64| b - (b - t) * v.clamp(0.0, 100.0) / 100.0;
    let mut grid = String::new();
    for v in [0.0, 25.0, 50.0, 75.0, 100.0] {
        grid += &format!("M{l:.1} {:.1} L{r:.1} {:.1} ", y(v), y(v));
    }
    let step = (r - l) / (HISTORY - 1) as f64;
    let line = |dq: &VecDeque<f64>| -> String {
        if dq.len() < 2 {
            return String::new();
        }
        let skip = dq.len().saturating_sub(HISTORY);
        let pts: Vec<f64> = dq.iter().skip(skip).copied().collect();
        let last = pts.len() - 1;
        let mut out = String::with_capacity(pts.len() * 6);
        let mut first = true;
        for (i, v) in pts.iter().enumerate() {
            if (last - i) % 2 != 0 {
                continue; // la última muestra siempre se dibuja
            }
            out += &format!("{}{:.1} {:.1} ", if first { "M" } else { "L" }, l + i as f64 * step, y(*v));
            first = false;
        }
        out
    };
    let lines = [line(&hist[0]), line(&hist[1]), line(&hist[2])];
    let temp_fill = if lines[0].is_empty() {
        String::new()
    } else {
        let n = hist[0].len().min(HISTORY);
        format!("{}L{:.1} {b:.1} L{l:.1} {b:.1} Z", lines[0], l + (n - 1) as f64 * step)
    };
    ChartPaths { grid, lines, temp_fill }
}

/// Ventana mínima para errores fatales en modo gráfico.
pub fn show_fatal(message: &str) {
    if let Ok(w) = FatalWindow::new() {
        w.set_message(ss(message));
        let weak = w.as_weak();
        w.on_dismiss(move || {
            if let Some(w) = weak.upgrade() {
                w.hide().ok();
            }
        });
        let _ = w.run();
    } else {
        eprintln!("ERROR: {message}");
    }
}

#[allow(dead_code)]
const _APP: &str = APP_NAME;
