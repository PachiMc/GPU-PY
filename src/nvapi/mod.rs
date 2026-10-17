//! Todo lo que habla con NVAPI (la misma vía que usa PNY VelocityX).
//!
//! * `raw`   – carga de `nvapi64.dll` + `nvapi_QueryInterface` (solo Windows; en otros sistemas falla limpio)
//! * `oc`    – overclock de reloj y límite de potencia (Pstates20 / power policies)
//! * `illum` – LED por zonas de iluminación del driver
//! * `i2c`   – LED por I2C (controladores PALIT) con lista blanca de registros
//! * `fake`  – drivers simulados para `--demo` y pruebas

pub mod fake;
pub mod i2c;
pub mod illum;
pub mod oc;
pub mod raw;

use crate::error::{HwError, HwResult};

pub type Rgb = (u8, u8, u8);

/// Interfaz común de los controladores de LED (IllumLed / PalitLed).
pub trait LedController {
    fn describe(&self) -> String;
    fn num_zones(&self) -> usize;
    fn supports_effects(&self) -> bool {
        false
    }
    fn set_static(&mut self, rgb: Rgb, brightness: i32, zones: Option<&[usize]>) -> HwResult<()>;
    fn set_off(&mut self, zones: Option<&[usize]>) -> HwResult<()>;
    fn set_breathing(&mut self, rgb: Rgb, rgb2: Rgb, period_ms: u32, zones: Option<&[usize]>) -> HwResult<()>;
    fn set_cycle(&mut self, _speed: u32, _reverse: bool) -> HwResult<()> {
        Err(HwError::new("Este controlador no admite ese efecto."))
    }
    fn set_strobe(&mut self, _rgb: Rgb, _speed: u32, _reverse: bool) -> HwResult<()> {
        Err(HwError::new("Este controlador no admite ese efecto."))
    }
    fn restore_default(&mut self) -> HwResult<()>;
}

/// `#RRGGBB` → (r, g, b)
pub fn parse_color(text: &str) -> HwResult<Rgb> {
    let bad = || HwError::new(format!("Color inválido «{text}». Usa el formato #RRGGBB."));
    let t = text.trim().trim_start_matches('#');
    if t.len() != 6 || !t.is_ascii() {
        return Err(bad());
    }
    let p = |i: usize| u8::from_str_radix(&t[i..i + 2], 16).map_err(|_| bad());
    Ok((p(0)?, p(2)?, p(4)?))
}

pub fn color_hex((r, g, b): Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_color_ok_and_errors() {
        assert_eq!(parse_color("#ff0066").unwrap(), (255, 0, 102));
        assert_eq!(parse_color(" 00ff80 ").unwrap(), (0, 255, 128));
        assert!(parse_color("#ff00").is_err());
        assert!(parse_color("#gg0000").is_err());
        assert!(parse_color("#ñ00000").is_err());
        assert_eq!(color_hex((1, 2, 255)), "#0102ff");
    }
}
