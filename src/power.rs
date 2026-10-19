//! Detección de «reanudación» tras suspender o hibernar.
//!
//! Los temporizadores se detienen mientras el PC duerme. Al despertar, el siguiente tic ve un salto enorme del
//! reloj de pared respecto al anterior: eso es una reanudación. Es independiente del sistema (sin ventanas
//! ocultas ni mensajes de Windows) y se prueba sin hardware. Un cambio manual de la hora hacia delante o un
//! bloqueo largo del programa también se detectan, lo que solo provoca una reaplicación inofensiva.

use std::time::{SystemTime, UNIX_EPOCH};

pub const RESUME_GAP_SECS: f64 = 10.0;

pub struct ResumeDetector {
    last: Option<f64>,
    threshold: f64,
}

impl ResumeDetector {
    pub fn new(threshold_secs: f64) -> Self {
        Self { last: None, threshold: threshold_secs }
    }

    /// Llamar con la hora actual (segundos Unix) en cada tic. Devuelve los segundos «perdidos» si hubo reanudación.
    pub fn tick(&mut self, now: f64) -> Option<f64> {
        let gap = self.last.map(|l| now - l);
        self.last = Some(now);
        gap.filter(|g| *g > self.threshold)
    }
}

pub fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_a_long_gap_only() {
        let mut d = ResumeDetector::new(10.0);
        assert_eq!(d.tick(1000.0), None, "el primer tic no puede ser una reanudación");
        assert_eq!(d.tick(1000.25), None);
        assert_eq!(d.tick(1003.0), None, "un tirón corto no cuenta");
        assert_eq!(d.tick(1900.0), Some(897.0), "15 minutos dormido");
        assert_eq!(d.tick(1900.25), None, "después vuelve a la normalidad");
    }

    #[test]
    fn clock_going_backwards_is_ignored() {
        let mut d = ResumeDetector::new(10.0);
        d.tick(5000.0);
        assert_eq!(d.tick(100.0), None);
        assert_eq!(d.tick(100.25), None);
    }
}
