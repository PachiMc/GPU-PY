//! Icono de GPU PY en la bandeja del sistema (solo Windows, crate `tray-icon`).
//!
//! Corre en el hilo de la interfaz: Slint (winit) ya bombea los mensajes de Windows, y los eventos
//! del icono/menú se consultan periódicamente con `poll()`.

use crate::core::APP_NAME;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    ApplyProfile,
    Quit,
}

pub struct TrayController {
    icon: Option<TrayIcon>,
    ids: Option<(MenuId, MenuId, MenuId)>,
}

fn load_icon() -> Option<Icon> {
    let img = image::load_from_memory(include_bytes!("../assets/gpupy.png")).ok()?;
    let img = img.resize(64, 64, image::imageops::FilterType::Lanczos3).to_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).ok()
}

impl TrayController {
    pub fn new() -> Self {
        Self { icon: None, ids: None }
    }

    pub fn running(&self) -> bool {
        self.icon.is_some()
    }

    /// Muestra el icono. Devuelve `false` si no se pudo crear.
    pub fn start(&mut self) -> bool {
        if self.icon.is_some() {
            return true;
        }
        let Some(icon) = load_icon() else { return false };
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
            Err(_) => false,
        }
    }

    pub fn stop(&mut self) {
        self.icon = None; // al soltarlo desaparece de la bandeja
        self.ids = None;
    }

    /// Siguiente acción pendiente del usuario (si la hay).
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

impl Default for TrayController {
    fn default() -> Self {
        Self::new()
    }
}
