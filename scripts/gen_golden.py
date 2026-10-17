#!/usr/bin/env python3
"""Genera tests/golden/golden.json ejecutando el código Python ORIGINAL de GPU-PY.

Uso:  python3 scripts/gen_golden.py /ruta/a/GPU-PY   (carpeta con led_i2c.py, led_nvapi.py, oc_nvapi.py, core.py)
La prueba `tests/golden.rs` repite los mismos escenarios en Rust y exige resultados idénticos.
"""
import json, sys, os
src = sys.argv[1] if len(sys.argv) > 1 else "."
sys.path.insert(0, src)
import led_i2c as I, led_nvapi as N, oc_nvapi as O
import core as C

out = {}

# ---------------- LED por I2C ----------------
I2C_CASES = {
    "1202": dict(digits=b"120", rev=2), "1200": dict(digits=b"120", rev=0), "1201": dict(digits=b"120", rev=1),
    "2303": dict(digits=b"230", rev=3), "2307": dict(digits=b"230", rev=7), "502": dict(digits=b"050", rev=2),
    "100": dict(digits=b"010", rev=0), "1202b": dict(digits=b"120", rev=0x22),
    "legacy": dict(legacy=True), "absent": dict(absent=True),
}
OPS = [
    ("static_a", lambda l: l.set_static((255, 0, 102), 100)),
    ("static_b", lambda l: l.set_static((200, 101, 51), 37)),
    ("static_tie", lambda l: l.set_static((255, 253, 1), 50)),
    ("off", lambda l: l.set_off()),
    ("breath_a", lambda l: l.set_breathing((255, 0, 0), (0, 0, 255), 3000)),
    ("breath_b", lambda l: l.set_breathing((10, 20, 30), (0, 0, 0), 50)),
    ("breath_c", lambda l: l.set_breathing((1, 2, 3), (4, 5, 6), 99999)),
    ("cycle_a", lambda l: l.set_cycle(50, False)),
    ("cycle_b", lambda l: l.set_cycle(300, True)),
    ("strobe", lambda l: l.set_strobe((0, 255, 0), 50, True)),
    ("restore", lambda l: l.restore_default()),
]
i2c = {}
for name, kw in I2C_CASES.items():
    bus = I.FakeBus(**kw)
    led = I.PalitLed(bus)
    entry = {"id": led.id, "protocol": led.protocol, "describe": led.describe(),
             "effects": bool(led.protocol) and bool(led.supports_effects),
             "scan": I.scan_report(bus, led), "ops": {}}
    for op, fn in OPS:
        bus.log.clear()
        try:
            fn(led)
            entry["ops"][op] = {"log": [[d, r, p.hex()] for d, r, p in bus.log]}
        except Exception as e:  # LedError
            entry["ops"][op] = {"err": str(e)}
    i2c[name] = entry
out["i2c"] = i2c

# ---------------- LED por zonas NVAPI ----------------
def dump(api):
    p = api.current
    return {"version": p.version, "flags": p.flags, "num": p.num_zones,
            "zones": [[p.zones[i].zone_type, p.zones[i].ctrl_mode, bytes(p.zones[i].data)[:24].hex()]
                      for i in range(p.num_zones)]}

ILLUM = {
    "rgb2": (N.ZONE_RGB, N.ZONE_RGB),
    "mixed": (N.ZONE_RGB, N.ZONE_RGBW, N.ZONE_SINGLE),
    "none": (),
}
IOPS = [
    ("static", lambda l: l.set_static((255, 0, 102), 100)),
    ("static_zone1", lambda l: l.set_static((1, 2, 3), 50, zones=[1])),
    ("static_badzone", lambda l: l.set_static((1, 2, 3), 50, zones=[7, -1])),
    ("breath", lambda l: l.set_breathing((255, 0, 0), (0, 0, 255), 3000)),
    ("breath_short", lambda l: l.set_breathing((0, 9, 0), (0, 0, 0), 10)),
    ("off", lambda l: l.set_off()),
    ("restore", lambda l: l.restore_default()),
]
illum = {}
for name, types in ILLUM.items():
    api = N.FakeNvapi(types)
    led = N.IllumLed(api)
    entry = {"describe": led.describe(), "ops": {}}
    for op, fn in IOPS:
        try:
            fn(led)
            entry["ops"][op] = dump(api)
        except Exception as e:
            entry["ops"][op] = {"err": str(e)}
    illum[name] = entry
out["illum"] = illum

# ---------------- Overclock NVAPI ----------------
def run_oc(kind, api):
    oc = O.OcNvapi(api)
    res = {}
    def t(label, fn):
        try:
            v = fn()
            res[label] = {"ok": list(v) if isinstance(v, tuple) else v}
        except Exception as e:
            res[label] = {"err": str(e)}
    t("off_core", lambda: oc.get_offset(O.DOM_CORE)); t("off_mem", lambda: oc.get_offset(O.DOM_MEM))
    t("rng_core", lambda: oc.get_range(O.DOM_CORE)); t("rng_mem", lambda: oc.get_range(O.DOM_MEM))
    t("rng_bad", lambda: oc.get_range(9))
    t("set_core_50", lambda: oc.set_offset(O.DOM_CORE, 50)); t("set_mem_-100", lambda: oc.set_offset(O.DOM_MEM, -100))
    t("set_core_999", lambda: oc.set_offset(O.DOM_CORE, 999))
    t("off_core2", lambda: oc.get_offset(O.DOM_CORE)); t("off_mem2", lambda: oc.get_offset(O.DOM_MEM))
    t("pwr_range", lambda: oc.power_range()); t("pwr_get", lambda: oc.get_power_pct())
    for v in (110.5, 100.5, 101.5, 10, 500):
        t(f"pwr_set_{v}", lambda v=v: oc.set_power_pct(v))
    t("pwr_get2", lambda: oc.get_power_pct())
    res["set_calls"] = [list(c) for c in api.set_calls]
    return res

out["oc"] = {
    "standard": run_oc("std", O.FakeOCApi()),
    "no_v3": run_oc("nov3", O.FakeOCApi(v3_supported=False)),
    "locked": run_oc("locked", O.FakeOCApi(editable=False)),
}

# ---------------- Curva de ventiladores ----------------
curve = [(30, 30), (50, 40), (65, 60), (75, 80), (85, 100)]
out["interp"] = {str(t): C.interpolate(curve, t) for t in
                 [10, 30, 31.5, 40, 49.9, 50, 57.5, 58.5, 64.9, 70, 74.5, 79, 80.25, 84.9, 85, 99]}
out["interp_unsorted"] = C.interpolate([(85, 100), (30, 30), (50, 41)], 40.5)

path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "tests", "golden", "golden.json")
with open(path, "w") as f:
    json.dump(out, f, indent=1, sort_keys=True)
print("golden escrito en", os.path.normpath(path), "- escenarios:", list(out))
