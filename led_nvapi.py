"""
led_nvapi.py - Control del LED/RGB de la GPU mediante NVAPI.

VelocityX 1.4 controla el LED con las funciones del driver
    NvAPI_GPU_ClientIlluminationZonesGetControl   (id 0x3DBF5764)
    NvAPI_GPU_ClientIlluminationZonesSetControl   (id 0x197D065E)
obtenidas con nvapi_QueryInterface. Este módulo usa esas mismas funciones y el mismo
formato de estructura (versión 0x1194C = 6476 bytes, hasta 32 zonas de 200 bytes).

Diseño de la estructura (deducido del código de VelocityX.exe):

    IllumParams  (0x194C bytes)
      +0x00 u32  version  = 0x1194C  (tamaño | versión 1 << 16)
      +0x04 u32  flags    bit0 = bDefault (pedir valores de fábrica)
      +0x08 u32  num_zones
      +0x0C u8[64] reservado
      +0x4C ZoneControl[32]

    ZoneControl  (200 bytes)
      +0x00 u32  zone_type    1 = RGB, 3 = RGBW, 4 = un solo color (solo brillo)
      +0x04 u32  ctrl_mode    0 = manual, 1 = lineal por tramos (efectos)
      +0x08 ...  datos según el modo:
          modo 0, tipo 1:  R, G, B, brillo%
          modo 0, tipo 3:  R, G, B, W, brillo%
          modo 0, tipo 4:  brillo%
          modo 1, tipo 1:  color A (R,G,B,brillo%), color B (R,G,B,brillo%),
                           u32 tipo_de_ciclo(=2), u8 grupos(=1), u8 pad,
                           u16 t_subida_ms, u16 t_bajada_ms, u16 t_A_ms, u16 t_B_ms,
                           u16 t_reposo_ms, u16 desfase_ms

Solo funciona en Windows con driver NVIDIA. Para pruebas sin GPU existe FakeNvapi.
"""
from __future__ import annotations

import copy
import ctypes
import os
import struct
from ctypes import (POINTER, Structure, c_char, c_int, c_uint8, c_uint32,
                    c_void_p)

ID_INITIALIZE = 0x0150E828
ID_ENUM_GPUS = 0xE5AC921F
ID_GPU_NAME = 0xCEEE8E9F
ID_ERROR_MSG = 0x6C2D048C
ID_ZONES_GET = 0x3DBF5764
ID_ZONES_SET = 0x197D065E

MAX_ZONES = 32
PARAMS_VERSION = 0x1194C

ZONE_RGB, ZONE_RGBW, ZONE_SINGLE = 1, 3, 4
MODE_MANUAL, MODE_PIECEWISE = 0, 1
ZONE_NAMES = {ZONE_RGB: "RGB", ZONE_RGBW: "RGBW", ZONE_SINGLE: "Un color (brillo)"}

_STATUS_TEXT = {
    -1: "Error genérico de NVAPI.",
    -3: "El driver no implementa esta función (tarjeta o driver no compatibles).",
    -4: "NVAPI no está inicializada.",
    -5: "Argumento inválido.",
    -6: "No se encontró ninguna GPU NVIDIA.",
    -9: "Versión de estructura incompatible con el driver.",
    -104: "Función no soportada por esta GPU.",
}


class LedError(Exception):
    """Error con mensaje listo para mostrar al usuario."""


# --------------------------------------------------------------------------- #
#  Estructuras
# --------------------------------------------------------------------------- #
class ZoneControl(Structure):
    _pack_ = 1
    _fields_ = [("zone_type", c_uint32), ("ctrl_mode", c_uint32), ("data", c_uint8 * 192)]


class IllumParams(Structure):
    _pack_ = 1
    _fields_ = [("version", c_uint32), ("flags", c_uint32), ("num_zones", c_uint32),
                ("rsvd", c_uint8 * 64), ("zones", ZoneControl * MAX_ZONES)]


assert ctypes.sizeof(ZoneControl) == 200
assert ctypes.sizeof(IllumParams) == PARAMS_VERSION & 0xFFFF == 0x194C


def new_params(default: bool = False) -> IllumParams:
    p = IllumParams()
    ctypes.memset(ctypes.byref(p), 0, ctypes.sizeof(p))
    p.version = PARAMS_VERSION
    p.flags = 1 if default else 0
    return p


# --------------------------------------------------------------------------- #
#  Acceso real a nvapi64.dll
# --------------------------------------------------------------------------- #
class NvapiLib:
    def __init__(self):
        if os.name != "nt":
            raise LedError("NVAPI solo está disponible en Windows.")
        dll_name = "nvapi64.dll" if ctypes.sizeof(c_void_p) == 8 else "nvapi.dll"
        try:
            dll = ctypes.WinDLL(dll_name)
            qi = dll.nvapi_QueryInterface
        except (OSError, AttributeError):
            raise LedError(f"No se pudo cargar {dll_name}. ¿Está instalado el driver de NVIDIA?")
        qi.restype = c_void_p
        qi.argtypes = [c_uint32]
        self._qi = qi
        self._init = self._fn(ID_INITIALIZE)
        self._enum = self._fn(ID_ENUM_GPUS, POINTER(c_void_p * 64), POINTER(c_uint32))
        self._name = self._fn(ID_GPU_NAME, c_void_p, c_char * 64)
        self._err = self._fn(ID_ERROR_MSG, c_int, c_char * 64, optional=True)
        self._get = self._fn(ID_ZONES_GET, c_void_p, POINTER(IllumParams))
        self._set = self._fn(ID_ZONES_SET, c_void_p, POINTER(IllumParams))
        self.check(self._init(), "NvAPI_Initialize")

    def _fn(self, fid, *argtypes, optional=False):
        ptr = self._qi(fid)
        if not ptr:
            if optional:
                return None
            raise LedError(f"El driver no expone la función NVAPI 0x{fid:08X}. "
                           "Actualiza el driver de NVIDIA.")
        return ctypes.WINFUNCTYPE(c_int, *argtypes)(ptr)

    def check(self, status: int, what: str):
        if status == 0:
            return
        msg = None
        if self._err:
            buf = (c_char * 64)()
            if self._err(status, buf) == 0:
                msg = buf.value.decode(errors="replace")
        msg = _STATUS_TEXT.get(status) or msg or "error desconocido"
        raise LedError(f"{what} falló ({status}): {msg}")

    def gpus(self):
        handles, count = (c_void_p * 64)(), c_uint32(0)
        self.check(self._enum(ctypes.byref(handles), ctypes.byref(count)), "NvAPI_EnumPhysicalGPUs")
        out = []
        for i in range(count.value):
            buf = (c_char * 64)()
            name = buf.value.decode(errors="replace") if self._name(handles[i], buf) == 0 else "GPU NVIDIA"
            out.append((handles[i], name))
        return out

    def get_control(self, handle, params: IllumParams):
        self.check(self._get(handle, ctypes.byref(params)), "ClientIlluminationZonesGetControl")

    def set_control(self, handle, params: IllumParams):
        self.check(self._set(handle, ctypes.byref(params)), "ClientIlluminationZonesSetControl")


# --------------------------------------------------------------------------- #
#  GPU simulada (para --demo y pruebas)
# --------------------------------------------------------------------------- #
class FakeNvapi:
    """Imita al driver: guarda el último estado recibido. Sirve para --demo y tests."""

    def __init__(self, zone_types=(ZONE_RGB, ZONE_RGB)):
        self.default = new_params()
        self.default.num_zones = len(zone_types)
        for i, t in enumerate(zone_types):
            self.default.zones[i].zone_type = t
            self.default.zones[i].ctrl_mode = MODE_MANUAL
            self.default.zones[i].data[0:4] = (0, 255, 0, 100)
        self.current = copy.deepcopy(self.default)
        self.history = []

    def gpus(self):
        return [(1, "NVIDIA GeForce RTX 3070 (SIMULADA)")]

    def get_control(self, handle, params):
        src = self.default if params.flags & 1 else self.current
        ctypes.memmove(ctypes.byref(params), ctypes.byref(src), ctypes.sizeof(params))
        params.version = PARAMS_VERSION
        params.flags = 0

    def set_control(self, handle, params):
        if params.version != PARAMS_VERSION:
            raise LedError("Versión de estructura incompatible (simulador).")
        self.current = copy.deepcopy(params)
        self.history.append(copy.deepcopy(params))


# --------------------------------------------------------------------------- #
#  Control de alto nivel
# --------------------------------------------------------------------------- #
def parse_color(text: str):
    t = text.strip().lstrip("#")
    if len(t) != 6:
        raise LedError(f"Color inválido «{text}». Usa el formato #RRGGBB.")
    try:
        return int(t[0:2], 16), int(t[2:4], 16), int(t[4:6], 16)
    except ValueError:
        raise LedError(f"Color inválido «{text}». Usa el formato #RRGGBB.")


class IllumLed:
    def __init__(self, api=None, gpu_index: int = 0):
        self.api = api or NvapiLib()
        gpus = self.api.gpus()
        if not gpus:
            raise LedError("NVAPI no encontró ninguna GPU NVIDIA.")
        self.handle, self.gpu_name = gpus[min(gpu_index, len(gpus) - 1)]
        self.refresh()

    # -- información --------------------------------------------------------
    def refresh(self):
        p = self._read()
        self.num_zones = min(p.num_zones, MAX_ZONES)
        self.zone_types = [p.zones[i].zone_type for i in range(self.num_zones)]

    def describe(self) -> str:
        if not self.num_zones:
            return f"{self.gpu_name}: el driver no reporta zonas de iluminación."
        zs = ", ".join(f"#{i}: {ZONE_NAMES.get(t, f'tipo {t}')}" for i, t in enumerate(self.zone_types))
        return f"{self.gpu_name}: {self.num_zones} zona(s) → {zs}"

    # -- bajo nivel ---------------------------------------------------------
    def _read(self, default: bool = False) -> IllumParams:
        p = new_params(default)
        self.api.get_control(self.handle, p)
        return p

    def _write(self, p: IllumParams):
        p.version = PARAMS_VERSION
        p.flags &= ~1                    # VelocityX siempre limpia bDefault antes de escribir
        self.api.set_control(self.handle, p)

    def _targets(self, p, zones):
        if not p.num_zones:
            raise LedError("Tu GPU no reporta zonas de iluminación controlables por NVAPI.")
        idx = range(min(p.num_zones, MAX_ZONES)) if zones is None else \
            [z for z in zones if 0 <= z < min(p.num_zones, MAX_ZONES)]
        return list(idx)

    @staticmethod
    def _chk_rgb(rgb):
        if len(rgb) != 3 or any(not 0 <= c <= 255 for c in rgb):
            raise LedError("El color debe tener 3 componentes entre 0 y 255.")
        return tuple(int(c) for c in rgb)

    # -- efectos ------------------------------------------------------------
    def set_static(self, rgb, brightness: int = 100, zones=None):
        """Color fijo (modo manual), igual que la función de color de VelocityX."""
        r, g, b = self._chk_rgb(rgb)
        br = max(0, min(100, int(brightness)))
        p = self._read()
        for i in self._targets(p, zones):
            z = p.zones[i]
            z.ctrl_mode = MODE_MANUAL
            if z.zone_type == ZONE_RGB:
                z.data[0:4] = (r, g, b, br)
            elif z.zone_type == ZONE_RGBW:
                z.data[0:5] = (r, g, b, 0, br)
            elif z.zone_type == ZONE_SINGLE:
                z.data[0] = br & 0x7F
        self._write(p)

    def set_off(self, zones=None):
        self.set_static((0, 0, 0), 0, zones)

    def set_breathing(self, rgb, rgb2=(0, 0, 0), period_ms: int = 3000, zones=None):
        """Respiración: va del color A al color B y vuelve (modo lineal por tramos)."""
        a, b = self._chk_rgb(rgb), self._chk_rgb(rgb2)
        half = max(100, min(30000, int(period_ms) // 2))
        p = self._read()
        for i in self._targets(p, zones):
            z = p.zones[i]
            if z.zone_type != ZONE_RGB:
                continue                 # los efectos solo están documentados para zonas RGB
            for k in range(192):
                z.data[k] = 0
            z.ctrl_mode = MODE_PIECEWISE
            z.data[0:4] = (*a, 100 if any(a) else 0)
            z.data[4:8] = (*b, 100 if any(b) else 0)
            struct.pack_into("<IBxHHHHHH", z.data, 8, 2, 1, half, half, 100, 100, 0, 0)
        self._write(p)

    def restore_default(self):
        """Pide al driver los valores de fábrica (bDefault=1) y los aplica."""
        p = self._read(default=True)
        self._write(p)
