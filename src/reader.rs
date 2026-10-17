//! Lector de telemetría en un hilo propio: las llamadas a NVML pueden tardar decenas de milisegundos
//! (o más si el driver está ocupado) y en el hilo de la interfaz eso congela la ventana, sobre todo
//! al redimensionarla. La interfaz solo consulta el último valor, sin bloquearse.

use crate::backend::Telemetry;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

type Shared = Arc<Mutex<(u64, Option<Telemetry>)>>;

pub struct TelemetryReader {
    latest: Shared,
    stop: Arc<AtomicBool>,
    slow: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl TelemetryReader {
    /// `fast` = intervalo normal, `slow` = intervalo con la ventana en la bandeja.
    pub fn spawn(mut source: Box<dyn FnMut() -> Telemetry + Send>, fast: Duration, slow: Duration) -> Self {
        let latest: Shared = Arc::default();
        let (stop, slow_flag) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (l, s, sf) = (latest.clone(), stop.clone(), slow_flag.clone());
        let handle = std::thread::Builder::new()
            .name("gpupy-telemetria".into())
            .spawn(move || {
                let mut seq = 0u64;
                while !s.load(Ordering::Relaxed) {
                    let t = source();
                    seq += 1;
                    if let Ok(mut g) = l.lock() {
                        *g = (seq, Some(t));
                    }
                    let wait = if sf.load(Ordering::Relaxed) { slow } else { fast };
                    let step = Duration::from_millis(20).min(wait);
                    let mut waited = Duration::ZERO;
                    while waited < wait && !s.load(Ordering::Relaxed) {
                        std::thread::sleep(step);
                        waited += step;
                    }
                }
            })
            .ok();
        Self { latest, stop, slow: slow_flag, handle }
    }

    /// Última lectura y su número de secuencia (para no repetir muestras). No bloquea más que un instante.
    pub fn latest(&self) -> Option<(u64, Telemetry)> {
        let g = self.latest.lock().ok()?;
        g.1.clone().map(|t| (g.0, t))
    }

    pub fn set_slow(&self, slow: bool) {
        self.slow.store(slow, Ordering::Relaxed);
    }
}

impl Drop for TelemetryReader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // No se espera al hilo: podría estar dentro de una llamada lenta del driver.
        drop(self.handle.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn counting_source(n: Arc<AtomicUsize>) -> Box<dyn FnMut() -> Telemetry + Send> {
        Box::new(move || {
            let k = n.fetch_add(1, Ordering::SeqCst) + 1;
            Telemetry { temp: k as f64, ..Telemetry::default() }
        })
    }

    #[test]
    fn publishes_new_samples_with_increasing_sequence() {
        let n = Arc::new(AtomicUsize::new(0));
        let r = TelemetryReader::spawn(counting_source(n.clone()), Duration::from_millis(20), Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(200));
        let (seq, t) = r.latest().expect("debe haber una muestra");
        assert!(seq >= 3, "seq = {seq}");
        assert_eq!(t.temp as u64, seq);
    }

    #[test]
    fn slow_mode_reduces_rate_and_drop_stops_thread() {
        let n = Arc::new(AtomicUsize::new(0));
        let r = TelemetryReader::spawn(counting_source(n.clone()), Duration::from_millis(20), Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(120));
        r.set_slow(true);
        std::thread::sleep(Duration::from_millis(60)); // deja terminar la espera rápida en curso
        let before = n.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(300));
        assert!(n.load(Ordering::SeqCst) - before <= 1, "en modo lento no debería leer cada 20 ms");
        drop(r);
        std::thread::sleep(Duration::from_millis(60));
        let after = n.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(n.load(Ordering::SeqCst), after, "el hilo debe detenerse al soltar el lector");
    }

    #[test]
    fn slow_source_never_blocks_the_caller() {
        let r = TelemetryReader::spawn(
            Box::new(|| {
                std::thread::sleep(Duration::from_millis(400));
                Telemetry::default()
            }),
            Duration::from_millis(10),
            Duration::from_secs(5),
        );
        let t = std::time::Instant::now();
        let _ = r.latest();
        assert!(t.elapsed() < Duration::from_millis(50), "latest() no debe esperar al driver");
    }
}
