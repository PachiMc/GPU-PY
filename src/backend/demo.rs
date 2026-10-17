//! GPU simulada para probar la interfaz sin tarjeta (`--demo`).

use super::*;
use std::time::Instant;

pub struct DemoBackend {
    limits: Limits,
    power_limit: f64,
    core_off: i32,
    mem_off: i32,
    fan_manual: Option<i32>,
    locked: Option<u32>,
    temp: f64,
    t0: Instant,
    rng: u64,
}

impl DemoBackend {
    pub fn new() -> Self {
        Self {
            limits: Limits {
                power_min_w: 100.0,
                power_max_w: 240.0,
                power_default_w: 220.0,
                core_off_min: -300,
                core_off_max: 250,
                mem_off_min: -1000,
                mem_off_max: 1000,
                max_core_clock: 2100,
                max_mem_clock: 7001,
                num_fans: 2,
                oc_supported: true,
            },
            power_limit: 220.0,
            core_off: 0,
            mem_off: 0,
            fan_manual: None,
            locked: None,
            temp: 42.0,
            t0: Instant::now(),
            rng: 0x9E37_79B9_7F4A_7C15,
        }
    }

    /// Ruido uniforme en [-a, a] (xorshift, sin dependencias).
    fn jitter(&mut self, a: f64) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        ((self.rng >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) * a
    }

    /// Lectura con el tiempo `t` (s) dado, para poder probarla de forma determinista.
    pub fn read_at(&mut self, t: f64) -> Telemetry {
        let load = (0.6 + 0.35 * (t / 8.0).sin() + self.jitter(0.03)).clamp(0.05, 1.0);
        let power = self.power_limit.min(25.0 + load * 215.0);
        let fan = self.fan_manual.unwrap_or_else(|| (30.0 + (self.temp - 45.0).max(0.0) * 1.7).min(100.0) as i32);
        let target = 32.0 + power / 240.0 * 55.0 - fan as f64 * 0.22;
        self.temp += (target - self.temp) * 0.1;
        let mut core = 1900.0 + self.core_off as f64 - (self.temp - 72.0).max(0.0) * 8.0;
        if let Some(lock) = self.locked {
            core = core.min(lock as f64);
        }
        let throttle = if power >= self.power_limit - 1.0 { "Límite de potencia" } else { "" };
        Telemetry {
            temp: (self.temp * 10.0).round_ties_even() / 10.0,
            core_clock: core.max(0.0) as u32,
            mem_clock: (7001 + self.mem_off).max(0) as u32,
            fan_pct: fan as u32,
            power_w: (power * 10.0).round_ties_even() / 10.0,
            util_gpu: (load * 100.0) as u32,
            vram_used_mb: (2000.0 + load * 1500.0) as u64,
            vram_total_mb: 8192,
            throttle: throttle.into(),
        }
    }
}

impl Default for DemoBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl GpuBackend for DemoBackend {
    fn name(&self) -> &str {
        "NVIDIA GeForce RTX 3070 (SIMULADA)"
    }
    fn driver(&self) -> &str {
        "000.00"
    }
    fn is_demo(&self) -> bool {
        true
    }
    fn limits(&self) -> &Limits {
        &self.limits
    }
    fn oc_source(&self) -> Option<&str> {
        Some("demo")
    }
    fn oc_error(&self) -> Option<&str> {
        None
    }

    fn read(&mut self) -> Telemetry {
        let t = self.t0.elapsed().as_secs_f64();
        self.read_at(t)
    }

    fn core_offset(&mut self) -> i32 {
        self.core_off
    }
    fn mem_offset(&mut self) -> i32 {
        self.mem_off
    }
    fn set_power_limit(&mut self, w: f64) -> HwResult<()> {
        self.power_limit = w;
        Ok(())
    }
    fn set_core_offset(&mut self, m: i32) -> HwResult<()> {
        self.core_off = m;
        Ok(())
    }
    fn set_mem_offset(&mut self, m: i32) -> HwResult<()> {
        self.mem_off = m;
        Ok(())
    }
    fn lock_clocks(&mut self, _lo: u32, hi: u32) -> HwResult<()> {
        self.locked = Some(hi);
        Ok(())
    }
    fn unlock_clocks(&mut self) -> HwResult<()> {
        self.locked = None;
        Ok(())
    }
    fn set_fan(&mut self, p: i32) -> HwResult<()> {
        self.fan_manual = Some(p.clamp(MIN_FAN_PCT, 100));
        Ok(())
    }
    fn fan_auto(&mut self) -> HwResult<()> {
        self.fan_manual = None;
        Ok(())
    }

    fn reset_all(&mut self) -> Vec<String> {
        self.power_limit = self.limits.power_default_w;
        self.core_off = 0;
        self.mem_off = 0;
        self.locked = None;
        self.fan_manual = None;
        vec![]
    }
}
