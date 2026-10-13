"""
gpu_backend.py - Acceso a la GPU NVIDIA mediante NVML (paquete oficial nvidia-ml-py).

Incluye DemoBackend, una GPU simulada para probar la interfaz sin tarjeta.
"""
from __future__ import annotations

import math
import os
import random
import time
from dataclasses import dataclass

from led_nvapi import LedError

try:
    import pynvml as nv          # lo provee el paquete "nvidia-ml-py"
    NVMLError = nv.NVMLError
except Exception:                # paquete no instalado
    nv = None

    class NVMLError(Exception):
        pass


class BackendError(Exception):
    """Error con mensaje ya traducido para mostrar al usuario."""


# Topes "blandos" de seguridad para los sliders (se intersectan con los del driver)
SOFT_CORE_OFFSET = (-300, 250)
SOFT_MEM_OFFSET = (-1000, 1000)
MIN_FAN_PCT = 30

THROTTLE_REASONS = {
    0x04: "Límite de potencia",
    0x08: "Frenado por hardware",
    0x20: "Límite térmico (SW)",
    0x40: "Límite térmico (HW)",
    0x80: "Freno de energía",
}


@dataclass
class Telemetry:
    temp: float = 0.0
    core_clock: int = 0
    mem_clock: int = 0
    fan_pct: int = 0
    power_w: float = 0.0
    util_gpu: int = 0
    vram_used_mb: int = 0
    vram_total_mb: int = 0
    throttle: str = ""


@dataclass
class Limits:
    power_min_w: float = 100.0
    power_max_w: float = 220.0
    power_default_w: float = 220.0
    core_off_min: int = SOFT_CORE_OFFSET[0]
    core_off_max: int = SOFT_CORE_OFFSET[1]
    mem_off_min: int = SOFT_MEM_OFFSET[0]
    mem_off_max: int = SOFT_MEM_OFFSET[1]
    max_core_clock: int = 2100
    max_mem_clock: int = 7000
    num_fans: int = 0
    oc_supported: bool = False


def is_admin() -> bool:
    try:
        if os.name == "nt":
            import ctypes
            return bool(ctypes.windll.shell32.IsUserAnAdmin())
        return os.geteuid() == 0
    except Exception:
        return False


def _s(x):
    return x.decode(errors="replace") if isinstance(x, bytes) else str(x)


def _explain(e) -> str:
    code = getattr(e, "value", None)
    if nv is not None:
        if code == nv.NVML_ERROR_NO_PERMISSION:
            return "Permiso denegado: ejecuta el programa como Administrador."
        if code == nv.NVML_ERROR_NOT_SUPPORTED:
            return "Tu GPU o tu driver no soportan esta función."
        if code == nv.NVML_ERROR_INVALID_ARGUMENT:
            return "Valor fuera del rango permitido por el driver."
        if code == nv.NVML_ERROR_GPU_IS_LOST:
            return "La GPU dejó de responder (¿driver reiniciándose?)."
    return f"Error NVML: {e}"


# --------------------------------------------------------------------------- #
#  Backend real
# --------------------------------------------------------------------------- #
class NvmlBackend:
    demo = False

    def __init__(self, index: int = 0):
        if nv is None:
            raise BackendError("Falta la librería. Instala con:\n    pip install nvidia-ml-py")
        try:
            nv.nvmlInit()
            self.h = nv.nvmlDeviceGetHandleByIndex(index)
        except NVMLError as e:
            raise BackendError(
                "No se pudo iniciar NVML. ¿Está instalado el driver de NVIDIA?\n" + _explain(e))
        self.name = _s(nv.nvmlDeviceGetName(self.h))
        try:
            self.driver = _s(nv.nvmlSystemGetDriverVersion())
        except NVMLError:
            self.driver = "?"
        self.limits = self._probe_limits()
        self._oc = None                 # OcNvapi (se crea bajo demanda)
        self.oc_source = "nvml" if self.limits.oc_supported else None
        self.oc_error = None
        self._probe_nvapi_oc()

    # -- utilidades ---------------------------------------------------------
    def _call(self, fname, *args):
        fn = getattr(nv, fname, None)
        if fn is None:
            raise BackendError("Tu versión de nvidia-ml-py es muy antigua.\n"
                               "Actualiza con: pip install -U nvidia-ml-py")
        try:
            return fn(*args)
        except NVMLError as e:
            raise BackendError(_explain(e))

    def _try(self, fname, *args, default=None):
        try:
            return self._call(fname, *args)
        except BackendError:
            return default

    def _nvapi(self):
        """Acceso a NVAPI (Pstates20 / power policies), la misma vía que usa VelocityX."""
        if self._oc is None:
            try:
                from oc_nvapi import NvapiOCApi, OcNvapi
                self._oc = OcNvapi(NvapiOCApi())
            except LedError as e:
                raise BackendError(str(e))
        return self._oc

    @staticmethod
    def _oc_call(fn, *args):
        try:
            return fn(*args)
        except LedError as e:
            raise BackendError(str(e))

    def _probe_nvapi_oc(self):
        """Si NVML no expone offsets de reloj, intenta con NVAPI."""
        L = self.limits
        if L.oc_supported:
            return
        try:
            oc = self._nvapi()
            core, mem = oc.get_range(0), oc.get_range(4)
        except (BackendError, LedError) as e:
            self.oc_error = str(e)
            return
        if core is None and mem is None:
            self.oc_error = "El driver no lista relojes editables en el P-state 0."
            return
        self.oc_source, L.oc_supported = "nvapi", True
        L.core_off_min, L.core_off_max = (max(core[0], SOFT_CORE_OFFSET[0]), min(core[1], SOFT_CORE_OFFSET[1])) \
            if core else (0, 0)
        L.mem_off_min, L.mem_off_max = (max(mem[0], SOFT_MEM_OFFSET[0]), min(mem[1], SOFT_MEM_OFFSET[1])) \
            if mem else (0, 0)

    def _probe_limits(self) -> Limits:
        h, L = self.h, Limits()
        pc = self._try("nvmlDeviceGetPowerManagementLimitConstraints", h)
        if pc:
            L.power_min_w, L.power_max_w = pc[0] / 1000, pc[1] / 1000
        pd = self._try("nvmlDeviceGetPowerManagementDefaultLimit", h)
        L.power_default_w = pd / 1000 if pd else L.power_max_w

        if self._try("nvmlDeviceGetGpcClkVfOffset", h) is not None:
            L.oc_supported = True
            r = self._try("nvmlDeviceGetGpcClkMinMaxVfOffset", h)
            if r:
                L.core_off_min = max(r[0], SOFT_CORE_OFFSET[0])
                L.core_off_max = min(r[1], SOFT_CORE_OFFSET[1])
            r = self._try("nvmlDeviceGetMemClkMinMaxVfOffset", h)
            if r:
                L.mem_off_min = max(r[0], SOFT_MEM_OFFSET[0])
                L.mem_off_max = min(r[1], SOFT_MEM_OFFSET[1])
        L.max_core_clock = self._try("nvmlDeviceGetMaxClockInfo", h, nv.NVML_CLOCK_GRAPHICS,
                                     default=L.max_core_clock)
        L.max_mem_clock = self._try("nvmlDeviceGetMaxClockInfo", h, nv.NVML_CLOCK_MEM,
                                    default=L.max_mem_clock)
        L.num_fans = self._try("nvmlDeviceGetNumFans", h, default=0) or 0
        return L

    # -- lectura ------------------------------------------------------------
    def read(self) -> Telemetry:
        h, t = self.h, Telemetry()
        t.temp = self._try("nvmlDeviceGetTemperature", h, nv.NVML_TEMPERATURE_GPU, default=0)
        t.core_clock = self._try("nvmlDeviceGetClockInfo", h, nv.NVML_CLOCK_GRAPHICS, default=0)
        t.mem_clock = self._try("nvmlDeviceGetClockInfo", h, nv.NVML_CLOCK_MEM, default=0)
        t.fan_pct = self._try("nvmlDeviceGetFanSpeed", h, default=0)
        t.power_w = (self._try("nvmlDeviceGetPowerUsage", h, default=0) or 0) / 1000
        u = self._try("nvmlDeviceGetUtilizationRates", h)
        t.util_gpu = u.gpu if u else 0
        m = self._try("nvmlDeviceGetMemoryInfo", h)
        if m:
            t.vram_used_mb, t.vram_total_mb = m.used // 2**20, m.total // 2**20
        mask = (self._try("nvmlDeviceGetCurrentClocksEventReasons", h)
                or self._try("nvmlDeviceGetCurrentClocksThrottleReasons", h) or 0)
        t.throttle = ", ".join(v for k, v in THROTTLE_REASONS.items() if mask & k)
        return t

    def get_core_offset(self) -> int:
        if self.oc_source == "nvapi":
            try:
                return self._oc.get_offset(0) or 0
            except LedError:
                return 0
        return self._try("nvmlDeviceGetGpcClkVfOffset", self.h, default=0) or 0

    def get_mem_offset(self) -> int:
        if self.oc_source == "nvapi":
            try:
                return self._oc.get_offset(4) or 0
            except LedError:
                return 0
        return self._try("nvmlDeviceGetMemClkVfOffset", self.h, default=0) or 0

    # -- escritura (requieren Administrador) --------------------------------
    def set_power_limit(self, watts: float):
        L = self.limits
        watts = max(L.power_min_w, min(L.power_max_w, watts))
        try:
            self._call("nvmlDeviceSetPowerManagementLimit", self.h, int(round(watts * 1000)))
        except BackendError as e_nvml:                    # plan B: NVAPI, como VelocityX
            try:
                pct = watts / max(1.0, L.power_default_w) * 100
                self._oc_call(self._nvapi().set_power_pct, pct)
            except BackendError as e_api:
                raise BackendError(f"{e_nvml}  |  NVAPI: {e_api}")

    def set_core_offset(self, mhz: int):
        L = self.limits
        v = int(max(L.core_off_min, min(L.core_off_max, mhz)))
        if self.oc_source == "nvapi":
            if (L.core_off_min, L.core_off_max) != (0, 0):
                self._oc_call(self._oc.set_offset, 0, v)
            return
        self._call("nvmlDeviceSetGpcClkVfOffset", self.h, v)

    def set_mem_offset(self, mhz: int):
        L = self.limits
        v = int(max(L.mem_off_min, min(L.mem_off_max, mhz)))
        if self.oc_source == "nvapi":
            if (L.mem_off_min, L.mem_off_max) != (0, 0):
                self._oc_call(self._oc.set_offset, 4, v)
            return
        self._call("nvmlDeviceSetMemClkVfOffset", self.h, v)

    def lock_clocks(self, min_mhz: int, max_mhz: int):
        self._call("nvmlDeviceSetGpuLockedClocks", self.h, int(min_mhz), int(max_mhz))

    def unlock_clocks(self):
        self._call("nvmlDeviceResetGpuLockedClocks", self.h)

    def set_fan(self, pct: int):
        pct = int(max(MIN_FAN_PCT, min(100, pct)))
        for i in range(max(1, self.limits.num_fans)):
            self._call("nvmlDeviceSetFanSpeed_v2", self.h, i, pct)

    def fan_auto(self):
        for i in range(max(1, self.limits.num_fans)):
            self._call("nvmlDeviceSetDefaultFanSpeed_v2", self.h, i)

    def reset_all(self) -> list[str]:
        errs = []
        for label, fn, args in [
            ("Potencia", self.set_power_limit, (self.limits.power_default_w,)),
            ("Núcleo", self.set_core_offset, (0,)),
            ("Memoria", self.set_mem_offset, (0,)),
            ("Bloqueo de reloj", self.unlock_clocks, ()),
            ("Ventilador", self.fan_auto, ()),
        ]:
            try:
                fn(*args)
            except BackendError as e:
                errs.append(f"{label}: {e}")
        return errs

    def close(self):
        try:
            nv.nvmlShutdown()
        except Exception:
            pass


# --------------------------------------------------------------------------- #
#  GPU simulada (para --demo)
# --------------------------------------------------------------------------- #
class DemoBackend:
    demo = True
    name = "NVIDIA GeForce RTX 3070 (SIMULADA)"
    driver = "000.00"

    def __init__(self):
        self.limits = Limits(power_min_w=100, power_max_w=240, power_default_w=220,
                             core_off_min=-300, core_off_max=250,
                             mem_off_min=-1000, mem_off_max=1000,
                             max_core_clock=2100, max_mem_clock=7001,
                             num_fans=2, oc_supported=True)
        self.oc_source, self.oc_error = "demo", None
        self.power_limit = 220.0
        self.core_off = 0
        self.mem_off = 0
        self.fan_manual = None
        self.locked = None
        self._temp = 42.0
        self._t0 = time.time()

    def read(self) -> Telemetry:
        t = time.time() - self._t0
        load = max(0.05, min(1.0, 0.6 + 0.35 * math.sin(t / 8) + random.uniform(-.03, .03)))
        power = min(self.power_limit, 25 + load * 215)
        fan = self.fan_manual if self.fan_manual is not None else \
            int(min(100, 30 + max(0, self._temp - 45) * 1.7))
        target = 32 + power / 240 * 55 - fan * 0.22
        self._temp += (target - self._temp) * 0.1
        core = 1900 + self.core_off - max(0, self._temp - 72) * 8
        if self.locked:
            core = min(core, self.locked)
        tl = ""
        if power >= self.power_limit - 1:
            tl = "Límite de potencia"
        return Telemetry(temp=round(self._temp, 1), core_clock=int(core),
                         mem_clock=7001 + self.mem_off, fan_pct=fan, power_w=round(power, 1),
                         util_gpu=int(load * 100), vram_used_mb=int(2000 + load * 1500),
                         vram_total_mb=8192, throttle=tl)

    def get_core_offset(self): return self.core_off
    def get_mem_offset(self): return self.mem_off
    def set_power_limit(self, w): self.power_limit = float(w)
    def set_core_offset(self, m): self.core_off = int(m)
    def set_mem_offset(self, m): self.mem_off = int(m)
    def lock_clocks(self, lo, hi): self.locked = hi
    def unlock_clocks(self): self.locked = None
    def set_fan(self, p): self.fan_manual = max(MIN_FAN_PCT, min(100, int(p)))
    def fan_auto(self): self.fan_manual = None

    def reset_all(self):
        self.power_limit = self.limits.power_default_w
        self.core_off = self.mem_off = 0
        self.locked = self.fan_manual = None
        return []

    def close(self): pass
