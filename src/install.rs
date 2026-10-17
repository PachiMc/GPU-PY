//! Auto-instalación de GPU PY en una carpeta permanente.
//!
//! El .exe es portable, pero la tarea de inicio con Windows apunta a la ruta exacta del ejecutable:
//! si se ejecuta desde Descargas y esa carpeta se vacía, el inicio automático se rompe. Este módulo
//! copia el exe a una ubicación fija (`%LOCALAPPDATA%\GPU PY`) para que eso no pase.

use crate::core::APP_NAME;
use crate::error::{HwError, HwResult};
use std::path::{Path, PathBuf};

pub const INSTALLED_NAME: &str = "GPU-PY.exe";

/// ¿Es un ejecutable de release (instalable)? En desarrollo (`cargo run`) no aplica.
pub fn is_frozen() -> bool {
    !cfg!(debug_assertions)
}

pub fn current_exe() -> PathBuf {
    std::env::current_exe().and_then(|p| p.canonicalize()).unwrap_or_default()
}

pub fn install_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("HOME"))
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    base.join(APP_NAME)
}

pub fn installed_exe() -> PathBuf {
    install_dir().join(INSTALLED_NAME)
}

pub fn is_installed() -> bool {
    is_frozen() && installed_exe().canonicalize().is_ok_and(|p| p == current_exe())
}

/// Carpetas típicamente temporales (Descargas, Temp, Escritorio...).
pub fn is_temporary_location(parent: &Path, temp_dir: &Path) -> bool {
    let name = |p: Option<&Path>| p.and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_lowercase());
    let names = [name(Some(parent)), name(parent.parent())];
    const TYPICAL: [&str; 6] = ["descargas", "downloads", "temp", "tmp", "escritorio", "desktop"];
    if names.iter().flatten().any(|n| TYPICAL.contains(&n.as_str())) {
        return true;
    }
    parent.to_string_lossy().to_lowercase().starts_with(&temp_dir.to_string_lossy().to_lowercase())
}

pub fn should_suggest_install() -> bool {
    if !is_frozen() || is_installed() {
        return false;
    }
    current_exe().parent().is_some_and(|p| is_temporary_location(p, &std::env::temp_dir()))
}

/// Copia el exe en ejecución a la carpeta permanente. Devuelve la ruta destino.
pub fn install() -> HwResult<PathBuf> {
    if !is_frozen() {
        return Err(HwError::new("La instalación solo aplica al .exe (estás ejecutando una compilación de desarrollo)."));
    }
    let dst = installed_exe();
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| HwError::new(format!("No se pudo copiar a {}: {e}", dst.display())))?;
    }
    if is_installed() {
        return Ok(dst);
    }
    std::fs::copy(std::env::current_exe().unwrap_or_default(), &dst)
        .map_err(|e| HwError::new(format!("No se pudo copiar a {}: {e}", dst.display())))?;
    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_temporary_locations() {
        let tmp = Path::new("/tmp");
        assert!(is_temporary_location(Path::new("C:/Users/a/Downloads"), tmp));
        assert!(is_temporary_location(Path::new("C:/Users/a/Descargas/GPU"), tmp));
        assert!(is_temporary_location(Path::new("/tmp/xyz"), tmp));
        assert!(!is_temporary_location(Path::new("C:/Program Files/GPU PY"), tmp));
    }
}
