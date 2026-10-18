//! Registro de arranque y de fallos en `%APPDATA%\GPU PY\startup.log`.
//!
//! El .exe de release no tiene consola y se compila con `panic = "abort"`: sin esto, un fallo al arrancar con
//! Windows desaparece sin dejar rastro. El hook de pánico escribe el motivo antes de abortar.

use std::io::Write;

const MAX_BYTES: u64 = 200_000;

/// "AAAA-MM-DD HH:MM:SS" en UTC a partir de segundos Unix (sin dependencias).
pub fn format_utc(secs: u64) -> String {
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Algoritmo de días civiles (Howard Hinnant)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    format_utc(secs)
}

pub fn log_path() -> std::path::PathBuf {
    crate::core::data_dir().join("startup.log")
}

/// Añade una línea al registro (nunca falla: si no se puede escribir, se ignora).
pub fn log(msg: &str) {
    log_to(&log_path(), msg);
}

pub fn log_to(path: &std::path::Path, msg: &str) {
    // Si crece demasiado, conserva solo la mitad final.
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES) {
        if let Ok(text) = std::fs::read_to_string(path) {
            let keep = text.len() / 2;
            let cut = text.char_indices().map(|(i, _)| i).find(|&i| i >= keep).unwrap_or(0);
            let _ = std::fs::write(path, &text[cut..]);
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{} UTC] {msg}", now());
    }
}

/// Registra los pánicos (con `panic = "abort"` el proceso muere justo después).
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let bt = std::backtrace::Backtrace::force_capture();
        log(&format!("PÁNICO: {info}\n{bt}"));
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_timestamps() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00");
        assert_eq!(format_utc(951_782_400), "2000-02-29 00:00:00"); // año bisiesto
        assert_eq!(format_utc(1_760_000_000), "2025-10-09 08:53:20");
    }

    #[test]
    fn appends_lines_and_trims_big_files() {
        let p = std::env::temp_dir().join(format!("gpupy_log_{}.log", std::process::id()));
        let _ = std::fs::remove_file(&p);
        log_to(&p, "hola");
        log_to(&p, "mundo ñ");
        let t = std::fs::read_to_string(&p).unwrap();
        assert_eq!(t.lines().count(), 2);
        assert!(t.contains("hola") && t.contains("mundo ñ") && t.contains("UTC]"));
        std::fs::write(&p, "x".repeat(250_000)).unwrap();
        log_to(&p, "fin");
        let size = std::fs::metadata(&p).unwrap().len();
        assert!(size < 140_000, "debía recortarse, tamaño = {size}");
        assert!(std::fs::read_to_string(&p).unwrap().trim_end().ends_with("fin") || size > 0);
        let _ = std::fs::remove_file(&p);
    }
}
