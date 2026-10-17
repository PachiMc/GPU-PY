// En release el .exe es de subsistema "windows" (sin consola); la CLI re-engancha la consola padre.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
    };
    // SAFETY: llamadas simples a la API de consola; si algo falla, simplemente no hay salida por consola.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return; // no hay consola padre (p. ej. doble clic o tarea programada)
        }
        let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
        let h = CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        );
        if h != INVALID_HANDLE_VALUE {
            SetStdHandle(STD_OUTPUT_HANDLE, h);
            SetStdHandle(STD_ERROR_HANDLE, h);
        }
    }
}

fn main() {
    #[cfg(windows)]
    if std::env::args().len() > 1 {
        attach_parent_console();
    }
    std::process::exit(gpu_py::cli::run());
}
