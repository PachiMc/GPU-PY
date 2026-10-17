//! Perfiles/ajustes (compatibilidad con los JSON de la versión Python), demo y gráficas.

use gpu_py::app::chart_paths;
use gpu_py::backend::{DemoBackend, GpuBackend, throttle_text};
use gpu_py::core::{LedState, Profile, ProfileStore, apply_profile_to_backend, default_profile};
use std::collections::VecDeque;

const PYTHON_PROFILES: &str = r##"{
  "Gaming": {"power_w": 220, "core_off": 120, "mem_off": 400, "lock_on": false, "lock_mhz": 2100,
             "fan_mode": "curve", "fan_manual": 50, "curve": [[30,30],[50,40],[65,60],[75,80],[85,100]],
             "led": {"mode": "breathing", "color": "#ff2d55", "color2": "#0a84ff", "brightness": 100,
                     "period": 3000, "zones": [0, 1], "speed": 50, "reverse": false}},
  "Sin LED": {"power_w": 160, "core_off": -100, "mem_off": 0, "lock_on": true, "lock_mhz": 1700,
              "fan_mode": "auto", "fan_manual": 50, "curve": [[30,30],[50,40]], "led": null}
}"##;

fn tmp(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("gpupy_test_{}_{name}", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

#[test]
fn reads_profiles_written_by_the_python_version() {
    let path = tmp("py.json");
    std::fs::write(&path, PYTHON_PROFILES).unwrap();
    let store = ProfileStore::at(path);
    assert_eq!(store.names(), ["Gaming", "Sin LED"]);
    let g = &store.data["Gaming"];
    assert_eq!((g.power_w, g.core_off, g.mem_off), (220.0, 120, 400));
    assert_eq!(g.curve[4], (85, 100));
    assert_eq!(g.led.as_ref().unwrap().zones, Some(vec![0, 1]));
    assert!(store.data["Sin LED"].led.is_none());
}

#[test]
fn roundtrip_is_stable_and_missing_fields_get_defaults() {
    let path = tmp("rt.json");
    std::fs::write(&path, r#"{"Mínimo": {"core_off": 55}}"#).unwrap();
    let mut store = ProfileStore::at(path.clone());
    assert_eq!(store.data["Mínimo"].core_off, 55);
    assert_eq!(store.data["Mínimo"].curve.len(), 5); // por defecto
    store.save().unwrap();
    let again = ProfileStore::at(path);
    assert_eq!(again.data, store.data);
}

#[test]
fn corrupt_file_is_backed_up_before_overwriting() {
    let path = tmp("bad.json");
    std::fs::write(&path, "{esto no es json").unwrap();
    let mut store = ProfileStore::at(path.clone());
    assert!(store.data.is_empty());
    store.data.insert("Nuevo".into(), Profile::default());
    store.save().unwrap();
    assert_eq!(std::fs::read_to_string(path.with_extension("json.bak")).unwrap(), "{esto no es json");
}

#[test]
fn demo_backend_applies_and_resets_a_profile() {
    let mut be = DemoBackend::new();
    let p = Profile { power_w: 180.0, core_off: 90, mem_off: 300, lock_on: true, lock_mhz: 1800, ..default_profile(&be) };
    let errs = apply_profile_to_backend(&mut be, &p, None);
    assert!(errs.is_empty(), "{errs:?}");
    assert_eq!((be.core_offset(), be.mem_offset()), (90, 300));
    let t = be.read_at(5.0);
    assert!(t.core_clock <= 1800 && t.power_w <= 180.0);
    assert!(be.reset_all().is_empty());
    assert_eq!((be.core_offset(), be.mem_offset()), (0, 0));
}

#[test]
fn fan_is_clamped_to_safe_minimum() {
    let mut be = DemoBackend::new();
    be.set_fan(5).unwrap();
    assert_eq!(be.read_at(1.0).fan_pct, 30);
    be.fan_auto().unwrap();
}

#[test]
fn throttle_text_lists_reasons() {
    assert_eq!(throttle_text(0), "");
    assert_eq!(throttle_text(0x04 | 0x40), "Límite de potencia, Límite térmico (HW)");
}

#[test]
fn chart_paths_stay_inside_the_canvas_and_are_lightweight() {
    let mut h: [VecDeque<f64>; 3] = Default::default();
    for i in 0..130 {
        h[0].push_back(40.0 + i as f64 % 70.0);
        h[1].push_back(150.0); // fuera de rango: se recorta
        h[2].push_back(-20.0);
    }
    let c = chart_paths(&h);
    assert!(c.grid.starts_with('M'));
    assert!(c.temp_fill.starts_with('M') && c.temp_fill.ends_with('Z'));
    for line in &c.lines {
        assert!(line.starts_with('M'));
        let pts = line.split_whitespace().count() / 2;
        assert!(pts <= 61, "demasiados puntos por serie: {pts}");
        for tok in line.split_whitespace() {
            let n: f64 = tok.trim_start_matches(['M', 'L']).parse().unwrap();
            assert!((0.0..=520.0).contains(&n), "coordenada fuera del lienzo: {n}");
        }
    }
    // la última muestra (extremo derecho) siempre está
    assert!(c.lines[0].trim_end().rsplit(' ').nth(1).unwrap().trim_start_matches('L').parse::<f64>().unwrap() > 505.0);
    let empty: [VecDeque<f64>; 3] = Default::default();
    let e = chart_paths(&empty);
    assert!(e.lines[0].is_empty() && e.temp_fill.is_empty());
}

#[test]
fn led_state_serializes_like_python() {
    let v = serde_json::to_value(LedState::default()).unwrap();
    assert_eq!(v["mode"], "keep");
    assert_eq!(v["color"], "#ff2d55");
    assert!(v["zones"].is_null());
}
