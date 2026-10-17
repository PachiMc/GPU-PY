//! Prueba diferencial: los mismos escenarios que `scripts/gen_golden.py` ejecutó con el código Python
//! original deben dar EXACTAMENTE los mismos bytes, registros y mensajes en Rust.
//! (Los LED se ponen en `legacy_behavior = true`: el port añade un arreglo de «estado colgado» que
//! el original no tiene; ver `tests/led_fix.rs`.)

use gpu_py::core::interpolate;
use gpu_py::error::HwResult;
use gpu_py::nvapi::fake::{FakeNvapi, FakeOcApi};
use gpu_py::nvapi::i2c::{FakeBus, I2cBus, PalitLed, Protocol, scan_report};
use gpu_py::nvapi::illum::{IllumApi, IllumLed, IllumParams, ZONE_RGB, ZONE_RGBW, ZONE_SINGLE};
use gpu_py::nvapi::oc::{DOM_CORE, DOM_MEM, OcNvapi};
use gpu_py::nvapi::raw::GpuHandle;
use gpu_py::nvapi::LedController;
use serde_json::{Value, json};
use std::rc::Rc;

struct SharedBus(Rc<FakeBus>);
impl I2cBus for SharedBus {
    fn gpu_name(&self) -> String {
        self.0.gpu_name()
    }
    fn read(&self, d: u8, r: u8) -> HwResult<u8> {
        self.0.read(d, r)
    }
    fn write(&self, d: u8, r: u8, p: &[u8]) -> HwResult<()> {
        self.0.write(d, r, p)
    }
}

struct SharedIllum(Rc<FakeNvapi>);
impl IllumApi for SharedIllum {
    fn gpus(&self) -> HwResult<Vec<(GpuHandle, String)>> {
        self.0.gpus()
    }
    fn get_control(&self, h: GpuHandle, p: &mut IllumParams) -> HwResult<()> {
        self.0.get_control(h, p)
    }
    fn set_control(&self, h: GpuHandle, p: &mut IllumParams) -> HwResult<()> {
        self.0.set_control(h, p)
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn res(r: HwResult<Value>) -> Value {
    match r {
        Ok(v) => v,
        Err(e) => json!({ "err": e.0 }),
    }
}

fn i2c_section() -> Value {
    let cases: Vec<(&str, FakeBus)> = vec![
        ("1202", FakeBus::new(b"120", 2, false, false)),
        ("1200", FakeBus::new(b"120", 0, false, false)),
        ("1201", FakeBus::new(b"120", 1, false, false)),
        ("2303", FakeBus::new(b"230", 3, false, false)),
        ("2307", FakeBus::new(b"230", 7, false, false)),
        ("502", FakeBus::new(b"050", 2, false, false)),
        ("100", FakeBus::new(b"010", 0, false, false)),
        ("1202b", FakeBus::new(b"120", 0x22, false, false)),
        ("legacy", FakeBus::new(b"120", 2, true, false)),
        ("absent", FakeBus::new(b"120", 2, false, true)),
    ];
    let mut out = serde_json::Map::new();
    for (name, bus) in cases {
        let bus = Rc::new(bus);
        let mut led = PalitLed::new(Box::new(SharedBus(bus.clone())));
        led.legacy_behavior = true;
        let scan = scan_report(&led); // igual que en gen_golden.py: antes de las operaciones
        let ops: Vec<(&str, Box<dyn Fn(&mut PalitLed) -> HwResult<()>>)> = vec![
            ("static_a", Box::new(|l| l.set_static((255, 0, 102), 100, None))),
            ("static_b", Box::new(|l| l.set_static((200, 101, 51), 37, None))),
            ("static_tie", Box::new(|l| l.set_static((255, 253, 1), 50, None))),
            ("off", Box::new(|l| l.set_off(None))),
            ("breath_a", Box::new(|l| l.set_breathing((255, 0, 0), (0, 0, 255), 3000, None))),
            ("breath_b", Box::new(|l| l.set_breathing((10, 20, 30), (0, 0, 0), 50, None))),
            ("breath_c", Box::new(|l| l.set_breathing((1, 2, 3), (4, 5, 6), 99999, None))),
            ("cycle_a", Box::new(|l| l.set_cycle(50, false))),
            ("cycle_b", Box::new(|l| l.set_cycle(300, true))),
            ("strobe", Box::new(|l| l.set_strobe((0, 255, 0), 50, true))),
            ("restore", Box::new(|l| l.restore_default())),
        ];
        let mut ops_json = serde_json::Map::new();
        for (op, f) in ops {
            bus.log.borrow_mut().clear();
            let r = f(&mut led).map(|_| {
                json!({ "log": bus.log.borrow().iter().map(|(d, r, p)| json!([d, r, hex(p)])).collect::<Vec<_>>() })
            });
            ops_json.insert(op.into(), res(r));
        }
        let proto = match led.protocol {
            Some(Protocol::Palit92) => json!("palit92"),
            Some(Protocol::Palit10) => json!("palit10"),
            None => Value::Null,
        };
        out.insert(
            name.into(),
            json!({ "id": led.id, "protocol": proto, "describe": led.describe(), "effects": led.supports_effects(),
                    "scan": scan, "ops": ops_json }),
        );
    }
    Value::Object(out)
}

fn dump(api: &FakeNvapi) -> Value {
    let p = api.current.borrow();
    json!({ "version": p.version, "flags": p.flags, "num": p.num_zones,
            "zones": (0..p.num_zones as usize).map(|i| json!([p.zones[i].zone_type, p.zones[i].ctrl_mode, hex(&p.zones[i].data[..24])])).collect::<Vec<_>>() })
}

fn illum_section() -> Value {
    let cases: Vec<(&str, Vec<u32>)> = vec![
        ("rgb2", vec![ZONE_RGB, ZONE_RGB]),
        ("mixed", vec![ZONE_RGB, ZONE_RGBW, ZONE_SINGLE]),
        ("none", vec![]),
    ];
    let mut out = serde_json::Map::new();
    for (name, types) in cases {
        let api = Rc::new(FakeNvapi::new(&types));
        let mut led = IllumLed::new(Some(Box::new(SharedIllum(api.clone()))), 0).unwrap();
        led.legacy_behavior = true;
        let ops: Vec<(&str, Box<dyn Fn(&mut IllumLed) -> HwResult<()>>)> = vec![
            ("static", Box::new(|l| l.set_static((255, 0, 102), 100, None))),
            ("static_zone1", Box::new(|l| l.set_static((1, 2, 3), 50, Some(&[1])))),
            ("static_badzone", Box::new(|l| l.set_static((1, 2, 3), 50, Some(&[7])))),
            ("breath", Box::new(|l| l.set_breathing((255, 0, 0), (0, 0, 255), 3000, None))),
            ("breath_short", Box::new(|l| l.set_breathing((0, 9, 0), (0, 0, 0), 10, None))),
            ("off", Box::new(|l| l.set_off(None))),
            ("restore", Box::new(|l| l.restore_default())),
        ];
        let mut ops_json = serde_json::Map::new();
        for (op, f) in ops {
            ops_json.insert(op.into(), res(f(&mut led).map(|_| dump(&api))));
        }
        out.insert(name.into(), json!({ "describe": led.describe(), "ops": ops_json }));
    }
    Value::Object(out)
}

fn oc_run(api: FakeOcApi) -> Value {
    let api = Rc::new(api);
    struct Shared(Rc<FakeOcApi>);
    use gpu_py::nvapi::oc::OcApi;
    impl OcApi for Shared {
        fn ps_get(&self, b: &mut [u8]) -> i32 { self.0.ps_get(b) }
        fn ps_set(&self, b: &mut [u8]) -> i32 { self.0.ps_set(b) }
        fn pw_info(&self, b: &mut [u8]) -> HwResult<i32> { self.0.pw_info(b) }
        fn pw_get(&self, b: &mut [u8]) -> HwResult<i32> { self.0.pw_get(b) }
        fn pw_set(&self, b: &mut [u8]) -> HwResult<i32> { self.0.pw_set(b) }
        fn check(&self, s: i32, w: &str) -> HwResult<()> { self.0.check(s, w) }
    }
    let oc = OcNvapi::new(Box::new(Shared(api.clone())));
    let mut m = serde_json::Map::new();
    let mut t = |label: &str, v: HwResult<Value>| {
        m.insert(label.into(), match v {
            Ok(v) => json!({ "ok": v }),
            Err(e) => json!({ "err": e.0 }),
        });
    };
    t("off_core", oc.get_offset(DOM_CORE).map(|v| json!(v)));
    t("off_mem", oc.get_offset(DOM_MEM).map(|v| json!(v)));
    t("rng_core", oc.get_range(DOM_CORE).map(|v| json!(v.map(|(a, b)| [a, b]))));
    t("rng_mem", oc.get_range(DOM_MEM).map(|v| json!(v.map(|(a, b)| [a, b]))));
    t("rng_bad", oc.get_range(9).map(|v| json!(v.map(|(a, b)| [a, b]))));
    t("set_core_50", oc.set_offset(DOM_CORE, 50).map(|v| json!(v)));
    t("set_mem_-100", oc.set_offset(DOM_MEM, -100).map(|v| json!(v)));
    t("set_core_999", oc.set_offset(DOM_CORE, 999).map(|v| json!(v)));
    t("off_core2", oc.get_offset(DOM_CORE).map(|v| json!(v)));
    t("off_mem2", oc.get_offset(DOM_MEM).map(|v| json!(v)));
    t("pwr_range", oc.power_range().map(|(a, b, c)| json!([a, b, c])));
    t("pwr_get", oc.get_power_pct().map(|v| json!(v)));
    for (label, v) in [("110.5", 110.5), ("100.5", 100.5), ("101.5", 101.5), ("10", 10.0), ("500", 500.0)] {
        t(&format!("pwr_set_{label}"), oc.set_power_pct(v).map(|v| json!(v)));
    }
    t("pwr_get2", oc.get_power_pct().map(|v| json!(v)));
    m.insert("set_calls".into(), json!(api.set_calls.borrow().iter().map(|(d, v)| json!([d, v])).collect::<Vec<_>>()));
    Value::Object(m)
}

#[test]
fn rust_matches_original_python() {
    let golden: Value = serde_json::from_str(include_str!("golden/golden.json")).unwrap();

    let curve = [(30, 30), (50, 40), (65, 60), (75, 80), (85, 100)];
    let interp: serde_json::Map<String, Value> = [
        ("10", 10.0), ("30", 30.0), ("31.5", 31.5), ("40", 40.0), ("49.9", 49.9), ("50", 50.0), ("57.5", 57.5),
        ("58.5", 58.5), ("64.9", 64.9), ("70", 70.0), ("74.5", 74.5), ("79", 79.0), ("80.25", 80.25),
        ("84.9", 84.9), ("85", 85.0), ("99", 99.0),
    ]
    .iter()
    .map(|(k, t)| ((*k).to_string(), json!(interpolate(&curve, *t))))
    .collect();

    let mine = json!({
        "i2c": i2c_section(),
        "illum": illum_section(),
        "oc": {
            "standard": oc_run(FakeOcApi::standard()),
            "no_v3": oc_run(FakeOcApi::new(false, true, (-500, 1000), (-300, 300), (50, 100, 120))),
            "locked": oc_run(FakeOcApi::new(true, false, (-500, 1000), (-300, 300), (50, 100, 120))),
        },
        "interp": interp,
        "interp_unsorted": interpolate(&[(85, 100), (30, 30), (50, 41)], 40.5),
    });

    if mine != golden {
        let _ = std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/target/mine.json"), serde_json::to_string_pretty(&mine).unwrap());
    }
    for section in ["interp", "interp_unsorted", "oc", "illum", "i2c"] {
        assert_eq!(mine[section], golden[section], "difiere la sección «{section}» respecto al código Python original");
    }
}
