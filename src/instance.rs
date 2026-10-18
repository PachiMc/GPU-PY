//! Una sola instancia de GPU PY por sesión (dos instancias se pelearían por los ventiladores y el LED).
//! Importante con el inicio de Windows: la instancia arrancada en la bandeja está viva aunque no se vea ventana.

pub struct InstanceGuard(#[allow(dead_code)] usize);

/// `None` = ya hay otra instancia en ejecución.
pub fn acquire() -> Option<InstanceGuard> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
        use windows_sys::Win32::System::Threading::CreateMutexW;
        let name: Vec<u16> = "Local\\GPU-PY-instancia-unica\0".encode_utf16().collect();
        // SAFETY: nombre terminado en NUL; el manejador se mantiene abierto mientras viva el proceso.
        let (handle, err) = unsafe {
            let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            (h, GetLastError())
        };
        if handle.is_null() {
            return Some(InstanceGuard(0)); // no se pudo comprobar: no bloquear el arranque
        }
        if err == ERROR_ALREADY_EXISTS {
            return None;
        }
        Some(InstanceGuard(handle as usize))
    }
    #[cfg(not(windows))]
    {
        Some(InstanceGuard(0))
    }
}
