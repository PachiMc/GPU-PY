"""
core.py - Lógica de GPU PY independiente de la interfaz (perfiles, ajustes, aplicación de perfiles...).
"""
from __future__ import annotations

import json
import os
import shutil
import sys
from dataclasses import asdict, dataclass, fields
from pathlib import Path

from gpu_backend import BackendError
from led_i2c import FakeBus, NvapiBus, PalitLed, scan_report
from led_nvapi import FakeNvapi, IllumLed, LedError, parse_color

APP_NAME = "GPU PY"
VERSION = "2.1"

DEFAULT_CURVE: list[tuple[int, int]] = [(30, 30), (50, 40), (65, 60), (75, 80), (85, 100)]
SAFETY_TEMP = 88          # por encima de esto el ventilador se fuerza al 100 %


def resource_path(rel: str) -> Path:
    """Ruta a un recurso, tanto en desarrollo como dentro de un .exe de PyInstaller."""
    base = Path(getattr(sys, "_MEIPASS", Path(__file__).resolve().parent))
    return base / rel


def data_dir() -> Path:
    base = Path(os.environ.get("APPDATA") or Path.home() / ".config")
    path = base / APP_NAME
    path.mkdir(parents=True, exist_ok=True)
    old = base / "VelocityPy"                      # migración desde la versión anterior
    if old.is_dir() and not (path / "profiles.json").exists() and (old / "profiles.json").exists():
        shutil.copy2(old / "profiles.json", path / "profiles.json")
    return path


# --------------------------------------------------------------------------- #
#  Ajustes de la aplicación y perfiles
# --------------------------------------------------------------------------- #
@dataclass(slots=True)
class Settings:
    start_minimized: bool = True
    startup_profile: str | None = None
    tray_enabled: bool = True

    @classmethod
    def load(cls) -> "Settings":
        try:
            raw = json.loads((data_dir() / "settings.json").read_text(encoding="utf-8"))
            known = {f.name for f in fields(cls)}
            return cls(**{k: v for k, v in raw.items() if k in known})
        except (OSError, ValueError, TypeError):
            return cls()

    def save(self) -> None:
        (data_dir() / "settings.json").write_text(json.dumps(asdict(self), indent=2), encoding="utf-8")


class ProfileStore:
    def __init__(self):
        self.path = data_dir() / "profiles.json"
        try:
            self.data: dict[str, dict] = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            self.data = {}

    def save(self) -> None:
        self.path.write_text(json.dumps(self.data, indent=2, ensure_ascii=False), encoding="utf-8")

    def names(self) -> list[str]:
        return sorted(self.data)


def default_led_state() -> dict:
    return dict(mode="keep", color="#ff2d55", color2="#000000", brightness=100,
                period=3000, zones=None, speed=50, reverse=False)


def default_profile(be) -> dict:
    L = be.limits
    return dict(power_w=L.power_default_w, core_off=0, mem_off=0, lock_on=False,
                lock_mhz=L.max_core_clock, fan_mode="auto", fan_manual=50,
                curve=[list(p) for p in DEFAULT_CURVE], led=default_led_state())


# --------------------------------------------------------------------------- #
#  Aplicar ajustes
# --------------------------------------------------------------------------- #
def apply_led_state(led, st: dict) -> None:
    """mode: keep | static | breathing | off | default | cycle | strobe"""
    mode, zones = st.get("mode", "keep"), st.get("zones")
    match mode:
        case "keep":
            return
        case "off":
            led.set_off(zones)
        case "default":
            led.restore_default()
        case "cycle" | "strobe":
            if not getattr(led, "supports_effects", False):
                raise LedError("Este controlador no admite ese efecto.")
            if mode == "cycle":
                led.set_cycle(st.get("speed", 50), st.get("reverse", False))
            else:
                led.set_strobe(parse_color(st["color"]), st.get("speed", 50), st.get("reverse", False))
        case "breathing":
            led.set_breathing(parse_color(st["color"]), parse_color(st.get("color2", "#000000")),
                              st.get("period", 3000), zones)
        case _:
            led.set_static(parse_color(st["color"]), st.get("brightness", 100), zones)


def apply_profile_to_backend(be, p: dict, led=None) -> list[str]:
    """Aplica un perfil directamente (sin GUI). Devuelve la lista de errores."""
    errs: list[str] = []

    def run(label, fn, *a, quiet=False):
        try:
            fn(*a)
        except BackendError as e:
            if not quiet:
                errs.append(f"{label}: {e}")

    run("Límite de potencia", be.set_power_limit, p["power_w"])
    run("Offset del núcleo", be.set_core_offset, p["core_off"])
    run("Offset de la memoria", be.set_mem_offset, p["mem_off"])
    if p.get("lock_on"):
        run("Bloqueo de reloj", be.lock_clocks, 210, p["lock_mhz"])
    else:
        run("Bloqueo de reloj", be.unlock_clocks, quiet=True)
    match p["fan_mode"]:
        case "manual":
            run("Ventilador", be.set_fan, p["fan_manual"])
        case "auto":
            run("Ventilador", be.fan_auto, quiet=True)
    if led is not None and p.get("led"):
        try:
            apply_led_state(led, p["led"])
        except LedError as e:
            errs.append(f"LED: {e}")
    return errs


def interpolate(curve, temp: float) -> int:
    pts = sorted(curve)
    if temp <= pts[0][0]:
        return pts[0][1]
    if temp >= pts[-1][0]:
        return pts[-1][1]
    for (t0, p0), (t1, p1) in zip(pts, pts[1:]):
        if t0 <= temp <= t1:
            return round(p0 + (p1 - p0) * (temp - t0) / max(1, t1 - t0))
    return pts[-1][1]


# --------------------------------------------------------------------------- #
#  Detección de hardware y diagnósticos
# --------------------------------------------------------------------------- #
def make_led(demo: bool):
    """Prueba primero las zonas de NVAPI y, si no hay, el controlador I2C PALIT."""
    if demo:
        return IllumLed(FakeNvapi()), None
    problems = []
    try:
        led = IllumLed()
        if led.num_zones:
            return led, None
        problems.append("NVAPI: el driver no reporta zonas de iluminación.")
    except LedError as e:
        problems.append(f"NVAPI: {e}")
    try:
        led = PalitLed(NvapiBus())
        if led.protocol:
            return led, None
        problems.append("I2C: no se detectó un controlador LED PALIT (dispositivos 0x92 / 0x10).")
    except LedError as e:
        problems.append(f"I2C: {e}")
    return None, "  ".join(problems)


def i2c_scan_text(demo: bool) -> str:
    return scan_report(FakeBus() if demo else NvapiBus())


def oc_info_text(be) -> str:
    L = be.limits
    out = [f"GPU: {be.name}  (driver {be.driver})",
           f"Overclock de reloj: {'SÍ' if L.oc_supported else 'NO'}  (vía {getattr(be, 'oc_source', None) or 'ninguna'})"]
    if L.oc_supported:
        out.append(f"  Núcleo : offset actual {be.get_core_offset():+d} MHz, rango {L.core_off_min:+d} … {L.core_off_max:+d}")
        out.append(f"  Memoria: offset actual {be.get_mem_offset():+d} MHz, rango {L.mem_off_min:+d} … {L.mem_off_max:+d}")
    elif getattr(be, "oc_error", None):
        out.append(f"  Motivo: {be.oc_error}")
    out.append(f"Potencia: {L.power_min_w:.0f} … {L.power_max_w:.0f} W (por defecto {L.power_default_w:.0f} W)")
    return "\n".join(out)
