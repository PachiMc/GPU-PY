//! Icono de GPU PY en la bandeja del sistema.
//!
//! En Windows usa el crate `tray-icon` (corre en el hilo de la interfaz: Slint/winit ya bombea los mensajes
//! de Windows y los eventos se consultan con `poll()`). En otros sistemas es un sustituto que nunca se
//! puede crear, lo que permite probar en Linux el comportamiento «sin bandeja».
//!
//! Crear el icono puede fallar al iniciar sesión (el Explorador aún no ha creado la barra de tareas), por eso
//! los intentos fallidos se espacian 2 s en lugar de repetirse a cada consulta.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    ApplyProfile,
    Quit,
}

const RETRY_EVERY: Duration = Duration::from_secs(2);

pub struct TrayController {
    inner: imp::Tray,
    retry_after: Option<Instant>,
}

impl TrayController {
    pub fn new() -> Self {
        Self { inner: imp::Tray::new(), retry_after: None }
    }

    pub fn running(&self) -> bool {
        self.inner.running()
    }

    /// Muestra el icono. Devuelve `false` si no se pudo crear (se reintentará pasados 2 s).
    pub fn start(&mut self) -> bool {
        if self.inner.running() {
            return true;
        }
        if self.retry_after.is_some_and(|t| Instant::now() < t) {
            return false;
        }
        let ok = self.inner.create();
        self.retry_after = (!ok).then(|| Instant::now() + RETRY_EVERY);
        ok
    }

    pub fn stop(&mut self) {
        self.inner.destroy();
        self.retry_after = None;
    }

    /// Siguiente acción pendiente del usuario (si la hay).
    pub fn poll(&self) -> Option<TrayAction> {
        self.inner.poll()
    }
}

impl Default for TrayController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
mod imp {
    use super::TrayAction;
    use crate::core::APP_NAME;
    use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    pub struct Tray {
        icon: Option<TrayIcon>,
        ids: Option<(MenuId, MenuId, MenuId)>,
        cached: Option<Icon>,
    }

    fn load_icon() -> Option<Icon> {
        let img = image::load_from_memory(include_bytes!("../assets/gpupy.png")).ok()?;
        let img = img.resize(64, 64, image::imageops::FilterType::Lanczos3).to_rgba8();
        let (w, h) = img.dimensions();
        Icon::from_rgba(img.into_raw(), w, h).ok()
    }

    impl Tray {
        pub fn new() -> Self {
            Self { icon: None, ids: None, cached: None }
        }

        pub fn running(&self) -> bool {
            self.icon.is_some()
        }

        pub fn create(&mut self) -> bool {
            if self.cached.is_none() {
                self.cached = load_icon(); // se decodifica una sola vez
            }
            let Some(icon) = self.cached.clone() else { return false };
            let show = MenuItem::new("Mostrar GPU PY", true, None);
            let apply = MenuItem::new("Aplicar perfil de inicio", true, None);
            let quit = MenuItem::new("Salir", true, None);
            let menu = Menu::new();
            if menu.append_items(&[&show, &apply, &PredefinedMenuItem::separator(), &quit]).is_err() {
                return false;
            }
            let built = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false) // clic izquierdo = mostrar; derecho = menú
                .with_tooltip(APP_NAME)
                .with_icon(icon)
                .build();
            match built {
                Ok(t) => {
                    self.ids = Some((show.id().clone(), apply.id().clone(), quit.id().clone()));
                    self.icon = Some(t);
                    true
                }
                Err(e) => {
                    crate::applog::log(&format!("bandeja: no se pudo crear el icono: {e}"));
                    false
                }
            }
        }

        pub fn destroy(&mut self) {
            self.icon = None; // al soltarlo desaparece de la bandeja
            self.ids = None;
        }

        pub fn poll(&self) -> Option<TrayAction> {
            let (show, apply, quit) = self.ids.as_ref()?;
            let mut action = None;
            while let Ok(ev) = MenuEvent::receiver().try_recv() {
                if ev.id == *show {
                    action = Some(TrayAction::Show);
                } else if ev.id == *apply {
                    action = Some(TrayAction::ApplyProfile);
                } else if ev.id == *quit {
                    action = Some(TrayAction::Quit);
                }
            }
            while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
                match ev {
                    TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
                    | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => {
                        action = action.or(Some(TrayAction::Show));
                    }
                    _ => {}
                }
            }
            action
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::TrayAction;

    /// Sin bandeja: no se puede crear nunca.
    pub struct Tray;

    impl Tray {
        pub fn new() -> Self {
            Self
        }
        pub fn running(&self) -> bool {
            false
        }
        pub fn create(&mut self) -> bool {
            false
        }
        pub fn destroy(&mut self) {}
        pub fn poll(&self) -> Option<TrayAction> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn without_tray_start_fails_and_is_rate_limited() {
        let mut t = TrayController::new();
        assert!(!t.start());
        assert!(t.retry_after.is_some(), "tras un fallo no debe reintentar de inmediato");
        assert!(!t.start());
        assert!(!t.running() && t.poll().is_none());
    }
}
