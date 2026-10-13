"""
oc_nvapi.py - Overclock de reloj y límite de potencia vía NVAPI (la misma vía que VelocityX).

NVML no expone offsets de reloj en muchos drivers de Windows, pero NVAPI sí. Funciones usadas
(ids de nvapi_QueryInterface, confirmados dentro de VelocityX.exe):
    NvAPI_GPU_GetPstates20                    0x6FF81213
    NvAPI_GPU_SetPstates20                    0x0F4DAE6B
    NvAPI_GPU_ClientPowerPoliciesGetInfo      0x34206D86
    NvAPI_GPU_ClientPowerPoliciesGetStatus    0x70916171
    NvAPI_GPU_ClientPowerPoliciesSetStatus    0xAD95F5ED

Pstates20 (deducido del código de VelocityX):
    versión de lectura: 0x31CF8 (v3, 0x1CF8 bytes); si el driver responde -9, 0x11C94 (v1, 0x1C94 bytes)
    +0x00 version | +0x04 flags (bit0 = editable) | +0x08 numPstates | +0x0C numClocks | +0x10 numBaseVoltages
    +0x14 pstates[16], cada uno de 0x1C8 bytes:
          +0x00 pstateId | +0x04 flags | +0x08 clocks[8], cada uno de 0x2C bytes:
                +0x00 domainId (0 = núcleo, 4 = memoria) | +0x04 tipo | +0x08 flags (bit0 = editable)
                +0x0C delta kHz (s32) | +0x10 delta mínimo kHz | +0x14 delta máximo kHz | +0x18 datos de frecuencia
    escritura (v1): numPstates=1, numClocks=1, pstate[0].id=0, clock[0].domainId=dominio,
                    clock[0].delta = MHz * 1000

Power policies (porcentaje * 1000):
    Info   v2 0x208C8 (0x8C8 bytes) o v1 0x100B8 (0xB8): +4 byte válido, +5 byte nº de políticas,
           entradas desde +8 (paso 0x230 en v2, 0x2C en v1): +0x00 policyId, +0x10/+0x1C/+0x28 valores
           (mínimo, por defecto y máximo)
    Status v2 0x20558 (0x558 bytes): +4 byte nº de entradas; entradas desde +8 (paso 0x154):
           +0x00 policyId, +0x08 flags (bit0 = 0), +0x0C potencia (porcentaje * 1000)

Solo Windows. FakeOCApi simula un driver para las pruebas.
"""
from __future__ import annotations

import ctypes
import struct
from ctypes import c_void_p

from led_nvapi import LedError, NvapiLib

ID_PSTATES20_GET = 0x6FF81213
ID_PSTATES20_SET = 0x0F4DAE6B
ID_PWR_INFO = 0x34206D86
ID_PWR_GET = 0x70916171
ID_PWR_SET = 0xAD95F5ED

DOM_CORE, DOM_MEM = 0, 4

PS_V3, PS_V3_SIZE = 0x31CF8, 0x1CF8
PS_V1, PS_V1_SIZE = 0x11C94, 0x1C94
PSTATE_STRIDE, CLOCK_STRIDE, PSTATE_BASE = 0x1C8, 0x2C, 0x14

PWR_INFO_V2, PWR_INFO_V2_SIZE, PWR_INFO_V2_STRIDE = 0x208C8, 0x8C8, 0x230
PWR_INFO_V1, PWR_INFO_V1_SIZE, PWR_INFO_V1_STRIDE = 0x100B8, 0xB8, 0x2C
PWR_STATUS_V2, PWR_STATUS_SIZE, PWR_STATUS_STRIDE = 0x20558, 0x558, 0x154

STATUS_INCOMPATIBLE_VERSION = -9


def _u32(buf, off):
    return struct.unpack_from("<I", buf, off)[0]


def _s32(buf, off):
    return struct.unpack_from("<i", buf, off)[0]


def _byte(buf, off):
    return buf.raw[off]


def _buf(size, version):
    b = ctypes.create_string_buffer(size)          # relleno de ceros
    struct.pack_into("<I", b, 0, version)
    return b


# --------------------------------------------------------------------------- #
#  Acceso real a nvapi64.dll
# --------------------------------------------------------------------------- #
class NvapiOCApi(NvapiLib):
    def __init__(self):
        super().__init__()
        sig = (c_void_p, c_void_p)
        self._ps_get = self._fn(ID_PSTATES20_GET, *sig)
        self._ps_set = self._fn(ID_PSTATES20_SET, *sig)
        self._pw_info = self._fn(ID_PWR_INFO, *sig, optional=True)
        self._pw_get = self._fn(ID_PWR_GET, *sig, optional=True)
        self._pw_set = self._fn(ID_PWR_SET, *sig, optional=True)
        gpus = self.gpus()
        if not gpus:
            raise LedError("NVAPI no encontró ninguna GPU NVIDIA.")
        self.handle = gpus[0][0]

    def ps_get(self, buf):  return self._ps_get(self.handle, ctypes.byref(buf))
    def ps_set(self, buf):  return self._ps_set(self.handle, ctypes.byref(buf))

    def _pw(self, fn, buf):
        if fn is None:
            raise LedError("Este driver no expone el control de potencia de NVAPI.")
        return fn(self.handle, ctypes.byref(buf))

    def pw_info(self, buf):  return self._pw(self._pw_info, buf)
    def pw_get(self, buf):   return self._pw(self._pw_get, buf)
    def pw_set(self, buf):   return self._pw(self._pw_set, buf)


# --------------------------------------------------------------------------- #
#  Lógica de alto nivel
# --------------------------------------------------------------------------- #
class OcNvapi:
    def __init__(self, api):
        self.api = api

    # -- Pstates20 -------------------------------------------------------------
    def _read_pstates(self):
        for ver, size in ((PS_V3, PS_V3_SIZE), (PS_V1, PS_V1_SIZE)):
            buf = _buf(size, ver)
            st = self.api.ps_get(buf)
            if st == STATUS_INCOMPATIBLE_VERSION:
                continue
            self.api.check(st, "NvAPI_GPU_GetPstates20")
            return buf
        raise LedError("El driver rechazó las versiones de Pstates20 conocidas.")

    def _clock_offset(self, buf, domain):
        """Posición del reloj `domain` dentro del P-state 0 (como hace VelocityX)."""
        if not _u32(buf, 4) & 1:
            raise LedError("El driver marca los P-states como no editables (overclock bloqueado).")
        n_p, n_c = min(_u32(buf, 8), 16), min(_u32(buf, 0xC), 8)
        for p in range(n_p):
            po = PSTATE_BASE + p * PSTATE_STRIDE
            if _u32(buf, po) != 0:
                continue
            for c in range(n_c):
                co = po + 8 + c * CLOCK_STRIDE
                if _u32(buf, co) == domain:
                    return co
        return None

    def get_offset(self, domain: int):
        """Offset actual en MHz, o None si el dominio no existe."""
        buf = self._read_pstates()
        co = self._clock_offset(buf, domain)
        return None if co is None else int(_s32(buf, co + 0xC) / 1000)

    def get_range(self, domain: int):
        """(mínimo, máximo) permitido en MHz, o None si el dominio no existe."""
        buf = self._read_pstates()
        co = self._clock_offset(buf, domain)
        if co is None:
            return None
        return int(_s32(buf, co + 0x10) / 1000), int(_s32(buf, co + 0x14) / 1000)

    def set_offset(self, domain: int, mhz: int) -> int:
        buf = _buf(PS_V3_SIZE, PS_V1)                 # VelocityX usa v1 y reserva 0x1CF8 bytes
        struct.pack_into("<I", buf, 0x08, 1)          # numPstates
        struct.pack_into("<I", buf, 0x0C, 1)          # numClocks
        struct.pack_into("<I", buf, 0x10, 0)          # numBaseVoltages
        struct.pack_into("<I", buf, PSTATE_BASE, 0)   # pstate 0
        struct.pack_into("<I", buf, 0x1C, domain)     # clock[0].domainId
        struct.pack_into("<i", buf, 0x28, int(mhz) * 1000)  # clock[0].delta (kHz)
        self.api.check(self.api.ps_set(buf), "NvAPI_GPU_SetPstates20")
        got = self.get_offset(domain)
        if got is not None and got != int(mhz):
            raise LedError(f"El driver aceptó el cambio pero quedó en {got} MHz (pedido: {int(mhz)} MHz).")
        return got if got is not None else int(mhz)

    # -- Potencia -----------------------------------------------------------------
    def _power_info(self):
        for ver, size, stride in ((PWR_INFO_V2, PWR_INFO_V2_SIZE, PWR_INFO_V2_STRIDE),
                                  (PWR_INFO_V1, PWR_INFO_V1_SIZE, PWR_INFO_V1_STRIDE)):
            buf = _buf(size, ver)
            st = self.api.pw_info(buf)
            if st == STATUS_INCOMPATIBLE_VERSION:
                continue
            self.api.check(st, "NvAPI_GPU_ClientPowerPoliciesGetInfo")
            if not _byte(buf, 4):
                raise LedError("El driver no reporta políticas de potencia.")
            return buf, stride
        raise LedError("El driver rechazó las versiones de power policies conocidas.")

    def power_range(self):
        """(mínimo, por defecto, máximo) en % de la potencia base, política 0."""
        buf, stride = self._power_info()
        n = _byte(buf, 5)
        for i in range(min(n, 4)):
            e = 8 + i * stride
            if _u32(buf, e) == 0 and _u32(buf, e + 8) == 0:
                vals = sorted(_u32(buf, e + o) // 1000 for o in (0x10, 0x1C, 0x28))
                return vals[0], vals[1], vals[2]
        raise LedError("No se encontró la política de potencia total de la GPU.")

    def _status_buf(self, count_ids):
        buf = _buf(PWR_STATUS_SIZE, PWR_STATUS_V2)
        buf[4] = bytes([len(count_ids)])
        for i, pid in enumerate(count_ids):
            struct.pack_into("<I", buf, 8 + i * PWR_STATUS_STRIDE, pid)
        return buf

    def get_power_pct(self) -> int:
        info, stride = self._power_info()
        n = min(_byte(info, 5), 4)
        ids = [_u32(info, 8 + i * stride) for i in range(n)]
        buf = self._status_buf(ids)
        self.api.check(self.api.pw_get(buf), "NvAPI_GPU_ClientPowerPoliciesGetStatus")
        for i in range(n):
            e = 8 + i * PWR_STATUS_STRIDE
            if _u32(buf, e) == 0:
                return _u32(buf, e + 0xC) // 1000
        raise LedError("No se pudo leer la potencia actual.")

    def set_power_pct(self, pct: float) -> int:
        lo, _, hi = self.power_range()
        pct = int(round(max(lo, min(hi, pct))))
        buf = _buf(PWR_STATUS_SIZE, PWR_STATUS_V2)
        buf[4] = b"\x01"                                        # una entrada
        struct.pack_into("<I", buf, 8, 0)                       # policyId 0
        struct.pack_into("<I", buf, 0x10, _u32(buf, 0x10) & ~1)  # flags bit0 = 0
        struct.pack_into("<I", buf, 0x14, pct * 1000)           # potencia
        self.api.check(self.api.pw_set(buf), "NvAPI_GPU_ClientPowerPoliciesSetStatus")
        return pct


# --------------------------------------------------------------------------- #
#  Driver simulado (pruebas): valida las posiciones exactas que usa VelocityX
# --------------------------------------------------------------------------- #
class FakeOCApi:
    def __init__(self, v3_supported=True, editable=True, mem_ranges=(-500, 1000), core_ranges=(-300, 300),
                 power=(50, 100, 120)):
        self.v3_supported, self.editable = v3_supported, editable
        self.delta = {DOM_CORE: 0, DOM_MEM: 0}
        self.ranges = {DOM_CORE: core_ranges, DOM_MEM: mem_ranges}
        self.power_cfg, self.power_pct = power, 100
        self.set_calls = []

    @staticmethod
    def check(st, what):
        if st != 0:
            raise LedError(f"{what} falló ({st})")

    def ps_get(self, buf):
        ver = _u32(buf, 0)
        if ver == PS_V3 and not self.v3_supported:
            return STATUS_INCOMPATIBLE_VERSION
        assert ver in (PS_V3, PS_V1)
        struct.pack_into("<I", buf, 4, 1 if self.editable else 0)
        struct.pack_into("<III", buf, 8, 1, 2, 0)                   # 1 P-state, 2 relojes
        po = PSTATE_BASE
        struct.pack_into("<I", buf, po, 0)
        for c, dom in enumerate((DOM_CORE, DOM_MEM)):
            co = po + 8 + c * CLOCK_STRIDE
            struct.pack_into("<IIIiii", buf, co, dom, 1, 1, self.delta[dom] * 1000,
                             self.ranges[dom][0] * 1000, self.ranges[dom][1] * 1000)
        return 0

    def ps_set(self, buf):
        assert _u32(buf, 0) == PS_V1, "VelocityX escribe con la versión 0x11C94"
        assert (_u32(buf, 8), _u32(buf, 0xC), _u32(buf, 0x10), _u32(buf, PSTATE_BASE)) == (1, 1, 0, 0)
        dom, delta = _u32(buf, 0x1C), _s32(buf, 0x28)
        assert dom in self.delta and delta % 1000 == 0
        lo, hi = self.ranges[dom]
        if not lo * 1000 <= delta <= hi * 1000:
            return -5
        self.delta[dom] = delta // 1000
        self.set_calls.append((dom, delta // 1000))
        return 0

    def _info_version_ok(self, ver):
        return ver == PWR_INFO_V2

    def pw_info(self, buf):
        ver = _u32(buf, 0)
        if not self._info_version_ok(ver):
            return STATUS_INCOMPATIBLE_VERSION
        buf[4], buf[5] = b"\x01", b"\x01"
        lo, de, hi = self.power_cfg
        struct.pack_into("<I", buf, 8, 0)
        for off, v in zip((0x10, 0x1C, 0x28), (lo, de, hi)):       # posiciones que lee VelocityX
            struct.pack_into("<I", buf, 8 + off, v * 1000)
        return 0

    def pw_get(self, buf):
        assert _u32(buf, 0) == PWR_STATUS_V2 and buf[4] == b"\x01" and _u32(buf, 8) == 0
        struct.pack_into("<I", buf, 8 + 0xC, self.power_pct * 1000)
        return 0

    def pw_set(self, buf):
        assert _u32(buf, 0) == PWR_STATUS_V2 and buf[4] == b"\x01" and _u32(buf, 8) == 0
        assert _u32(buf, 0x10) & 1 == 0
        self.power_pct = _u32(buf, 0x14) // 1000
        return 0
