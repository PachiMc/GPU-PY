"""
led_i2c.py - Control del LED por I2C (controladores "PALIT" de las PNY XLR8 / Palit).

Cuando NVAPI no reporta zonas de iluminación, VelocityX 1.4 habla directamente con el
controlador de LED de la tarjeta usando NvAPI_I2CWriteEx / NvAPI_I2CReadEx (puerto I2C 1).
Todo lo de este archivo está deducido del código de VelocityX.exe:

Estructura NV_I2C_INFO_V3 (0x40 bytes, versión 0x30040): displayMask=0, bIsDDCPort=0,
regAddrSize=1, i2cSpeed=0xFFFF, i2cSpeedKhz=0, portId=1, bIsPortIdSet=1.

Controlador NUEVO  (dirección de 8 bits 0x92)
    identificación: regs 0xF6..0xFA = "PALIT"; dígitos ASCII en 0xFC,0xFD,0xFE y byte 0xF5
                    ID = (dígitos como decimal) * 10 + (reg[0xF5] & 0x1F)
    color fijo   : reg 0x60 = 1; 0x6C,0x6D,0x6E = R,G,B; 0x6F = brillo %
    buzón        : reg 0xE0 <- [0]                              parar efecto
                   reg 0xE0 <- [1, dir, vel, 0x1F]              ciclo (Cycle)
                   reg 0xE0 <- [2, dir, vel, 0, R, G, B, 100, 5] estroboscópico (Strobe)
    respiración  : parar, y escribir 22 bytes en los regs 0x60..0x75:
                   [0x22, 0x01, t_lo, t_hi, t_lo, t_hi, 100,0, 100,0, 0,0,
                    A.R, A.G, A.B, A.brillo, B.R, B.G, B.B, B.brillo, 0, 0]
Controlador ANTIGUO (dirección de 8 bits 0x10)
    identificación: regs 0x07..0x0B = "PALIT"
    color fijo   : reg 0x03 <- [R, G, B, 0xFF]

Seguridad: solo se escriben los registros anteriores (lista blanca). VelocityX también habla con
otros chips (p. ej. 0x40, probablemente el regulador de voltaje); este módulo NUNCA los toca.
La identificación es de solo lectura.
"""
from __future__ import annotations

import ctypes
from ctypes import POINTER, Structure, c_int32, c_uint8, c_uint32, c_uint64, c_void_p

from led_nvapi import LedError, NvapiLib

ID_I2C_WRITE = 0x283AC65A
ID_I2C_READ = 0x4D7B0709
I2C_INFO_VERSION = 0x30040
I2C_PORT = 1

DEV_NEW = 0x92          # 7 bits: 0x49
DEV_OLD = 0x10          # 7 bits: 0x08
REG_MAILBOX = 0xE0
IMG_FIRST, IMG_LAST = 0x60, 0x75

# Rangos contiguos [primero, último] que VelocityX escribe en cada dispositivo
ALLOWED_RANGES = {
    DEV_NEW: [(IMG_FIRST, IMG_LAST), (REG_MAILBOX, REG_MAILBOX + 8)],
    DEV_OLD: [(0x03, 0x06)],
}


class I2CInfoV3(Structure):
    _fields_ = [("version", c_uint32), ("displayMask", c_uint32),
                ("bIsDDCPort", c_uint8), ("i2cDevAddress", c_uint8),
                ("pbI2cRegAddress", POINTER(c_uint8)), ("regAddrSize", c_uint32),
                ("pbData", POINTER(c_uint8)), ("cbSize", c_uint32),
                ("i2cSpeed", c_uint32), ("i2cSpeedKhz", c_int32),
                ("portId", c_uint8), ("bIsPortIdSet", c_uint32)]


# Mismos desplazamientos que en el código de VelocityX (+0x09 dirección, +0x10 reg, +0x20 datos...)
assert ctypes.sizeof(I2CInfoV3) == 0x40
assert (I2CInfoV3.i2cDevAddress.offset, I2CInfoV3.pbI2cRegAddress.offset, I2CInfoV3.regAddrSize.offset,
        I2CInfoV3.pbData.offset, I2CInfoV3.cbSize.offset, I2CInfoV3.i2cSpeed.offset,
        I2CInfoV3.i2cSpeedKhz.offset, I2CInfoV3.portId.offset, I2CInfoV3.bIsPortIdSet.offset) == \
    (0x09, 0x10, 0x18, 0x20, 0x28, 0x2C, 0x30, 0x34, 0x38)


# --------------------------------------------------------------------------- #
#  Bus real (NVAPI)
# --------------------------------------------------------------------------- #
class NvapiBus(NvapiLib):
    def __init__(self):
        super().__init__()
        self._w = self._fn(ID_I2C_WRITE, c_void_p, POINTER(I2CInfoV3), POINTER(c_uint64))
        self._r = self._fn(ID_I2C_READ, c_void_p, POINTER(I2CInfoV3), POINTER(c_uint64))
        gpus = self.gpus()
        if not gpus:
            raise LedError("NVAPI no encontró ninguna GPU NVIDIA.")
        self.handle, self.gpu_name = gpus[0]

    def _info(self, dev, reg_buf, data_buf):
        i = I2CInfoV3()
        i.version, i.displayMask, i.bIsDDCPort, i.i2cDevAddress = I2C_INFO_VERSION, 0, 0, dev
        i.pbI2cRegAddress = ctypes.cast(reg_buf, POINTER(c_uint8))
        i.regAddrSize = 1
        i.pbData = ctypes.cast(data_buf, POINTER(c_uint8))
        i.cbSize = len(data_buf)
        i.i2cSpeed, i.i2cSpeedKhz = 0xFFFF, 0
        i.portId, i.bIsPortIdSet = I2C_PORT, 1
        return i

    def read(self, dev: int, reg: int) -> int:
        reg_buf, data = (c_uint8 * 1)(reg), (c_uint8 * 1)(0)
        info = self._info(dev, reg_buf, data)
        self.check(self._r(self.handle, ctypes.byref(info), ctypes.byref(c_uint64(1))),
                   f"I2C lectura 0x{dev:02X}[0x{reg:02X}]")
        return data[0]

    def write(self, dev: int, reg: int, payload: bytes):
        reg_buf, data = (c_uint8 * 1)(reg), (c_uint8 * len(payload)).from_buffer_copy(payload)
        info = self._info(dev, reg_buf, data)
        self.check(self._w(self.handle, ctypes.byref(info), ctypes.byref(c_uint64(1))),
                   f"I2C escritura 0x{dev:02X}[0x{reg:02X}]")


# --------------------------------------------------------------------------- #
#  Bus simulado (pruebas / --demo)
# --------------------------------------------------------------------------- #
class FakeBus:
    gpu_name = "NVIDIA GeForce RTX 3070 (SIMULADA, I2C)"

    def __init__(self, digits=b"120", rev=2, legacy=False, absent=False):
        self.mem = {DEV_NEW: {}, DEV_OLD: {}}
        self.log = []
        self.absent = absent
        if not absent and not legacy:
            for i, c in enumerate(b"PALIT"):
                self.mem[DEV_NEW][0xF6 + i] = c
            for i, c in enumerate(digits):
                self.mem[DEV_NEW][0xFC + i] = c
            self.mem[DEV_NEW][0xF5] = rev
        if legacy:
            for i, c in enumerate(b"PALIT"):
                self.mem[DEV_OLD][0x07 + i] = c
            for i, c in enumerate(b"2.0"):
                self.mem[DEV_OLD][0x0C + i] = c

    def read(self, dev, reg):
        if self.absent or dev not in self.mem or not any(self.mem[dev]):
            raise LedError(f"I2C lectura 0x{dev:02X}[0x{reg:02X}] falló: sin respuesta")
        return self.mem[dev].get(reg, 0)

    def write(self, dev, reg, payload):
        self.log.append((dev, reg, bytes(payload)))
        for i, b in enumerate(payload):
            self.mem.setdefault(dev, {})[reg + i] = b


# --------------------------------------------------------------------------- #
#  Reglas por ID (copiadas de la lógica de VelocityX)
# --------------------------------------------------------------------------- #
def still_supported(i: int) -> bool:
    """Color fijo por registros 0x60/0x6C-0x6F (función 0x186d0 de VelocityX)."""
    if i >= 1000 and i % 10 in (0, 1):
        return True
    if 11 <= i <= 999:
        return False
    if i > 1199 and i % 10 == 2:
        return True
    if i <= 1999:
        return False
    return i % 10 in (3, 7)


def still_needs_stop(i: int) -> bool:
    return (1199 < i <= 1999 and i % 10 == 2) or (i >= 2000 and i % 10 in (3, 7))


def effects_supported(i: int) -> bool:
    """Respiración / ciclo / estroboscópico (funciones 0x18c40 y 0x18d80)."""
    if i <= 2 or i == 10:
        return False
    return not (i >= 1000 and i % 10 in (0, 1))


def gray_only(i: int) -> bool:
    """IDs que terminan en 1 (>= 1000) son de un solo color: VelocityX usa el canal G para los tres."""
    return i >= 1000 and i % 10 == 1


# --------------------------------------------------------------------------- #
#  Controlador de alto nivel
# --------------------------------------------------------------------------- #
class PalitLed:
    """Misma interfaz que IllumLed para poder usarse desde la GUI."""

    def __init__(self, bus):
        self.bus = bus
        self.gpu_name = getattr(bus, "gpu_name", "GPU NVIDIA")
        self.protocol, self.id, self.regs = self._identify()
        self.num_zones = 1 if self.protocol else 0

    # -- identificación (solo lectura) ----------------------------------------
    def _read_block(self, dev, first, n):
        out = []
        for i in range(n):
            try:
                out.append(self.bus.read(dev, first + i))
            except LedError:
                return None
        return out

    def _identify(self):
        r = self._read_block(DEV_NEW, 0xF0, 16)
        if r and bytes(r[6:11]) == b"PALIT":
            digits = (r[12] & 0xF) * 100 + (r[13] & 0xF) * 10 + (r[14] & 0xF)
            return "palit92", digits * 10 + (r[5] & 0x1F), r
        r = self._read_block(DEV_OLD, 0x00, 16)
        if r and bytes(r[7:12]) == b"PALIT":
            return "palit10", 0, r
        return None, -1, None

    @property
    def supports_effects(self) -> bool:
        return self.protocol == "palit92" and effects_supported(self.id)

    def describe(self) -> str:
        if self.protocol == "palit92":
            return (f"{self.gpu_name}: controlador I2C PALIT (0x92), ID {self.id} → "
                    f"color fijo: {'sí' if still_supported(self.id) else 'no'}, "
                    f"efectos: {'sí' if effects_supported(self.id) else 'no'}")
        if self.protocol == "palit10":
            return f"{self.gpu_name}: controlador I2C PALIT antiguo (0x10) → color fijo"
        return f"{self.gpu_name}: no se encontró un controlador LED PALIT por I2C."

    # -- escritura con lista blanca -------------------------------------------
    def _w(self, dev, reg, data):
        last = reg + len(data) - 1
        if not any(lo <= reg and last <= hi for lo, hi in ALLOWED_RANGES.get(dev, [])):
            raise LedError(f"Escritura I2C bloqueada por seguridad: 0x{dev:02X}[0x{reg:02X}]")
        self.bus.write(dev, reg, bytes(data))

    def _need(self):
        if not self.protocol:
            raise LedError("No se detectó un controlador LED compatible por I2C.")

    @staticmethod
    def _rgb(rgb):
        if len(rgb) != 3 or any(not 0 <= c <= 255 for c in rgb):
            raise LedError("El color debe tener 3 componentes entre 0 y 255.")
        return tuple(int(c) for c in rgb)

    def _stop(self):
        self._w(DEV_NEW, REG_MAILBOX, [0])
        self._w(DEV_NEW, 0x60, [0])

    # -- API pública ----------------------------------------------------------
    def set_static(self, rgb, brightness: int = 100, zones=None):
        self._need()
        r, g, b = self._rgb(rgb)
        br = max(0, min(100, int(brightness)))
        if self.protocol == "palit10":
            k = br / 100
            self._w(DEV_OLD, 0x03, [round(r * k), round(g * k), round(b * k), 0xFF])
            return
        if not still_supported(self.id):
            raise LedError(f"VelocityX no define color fijo para este controlador (ID {self.id}). "
                           "Prueba un efecto o envíame el resultado de --i2c-scan.")
        if gray_only(self.id):
            r = g = b = g
        if still_needs_stop(self.id):
            self._w(DEV_NEW, REG_MAILBOX, [0])
        self._w(DEV_NEW, 0x60, [1])
        self._w(DEV_NEW, 0x6C, [r])
        self._w(DEV_NEW, 0x6D, [g])
        self._w(DEV_NEW, 0x6E, [b])
        self._w(DEV_NEW, 0x6F, [br])

    def set_off(self, zones=None):
        self.set_static((0, 0, 0), 100, zones)

    def _need_effects(self):
        self._need()
        if not self.supports_effects:
            raise LedError(f"Esta tarjeta (ID {self.id}) no admite efectos según VelocityX.")

    def set_breathing(self, rgb, rgb2=(0, 0, 0), period_ms: int = 3000, zones=None):
        self._need_effects()
        a, b = self._rgb(rgb), self._rgb(rgb2)
        t = max(100, min(30000, int(period_ms) // 2))
        img = bytearray(22)
        img[0], img[1] = 0x22, 0x01
        img[2:6] = bytes([t & 0xFF, t >> 8, t & 0xFF, t >> 8])
        img[6:10] = bytes([100, 0, 100, 0])
        img[12:16] = bytes([*a, 100 if any(a) else 0])
        img[16:20] = bytes([*b, 100 if any(b) else 0])
        self._stop()
        for k, v in enumerate(img):                       # VelocityX escribe byte a byte
            self._w(DEV_NEW, IMG_FIRST + k, [v])

    def set_cycle(self, speed: int = 50, reverse: bool = False):
        self._need_effects()
        self._w(DEV_NEW, REG_MAILBOX, [1, 1 if reverse else 0, int(speed) & 0xFF, 0x1F])

    def set_strobe(self, rgb, speed: int = 50, reverse: bool = False):
        self._need_effects()
        r, g, b = self._rgb(rgb)
        self._w(DEV_NEW, REG_MAILBOX, [2, 1 if reverse else 0, int(speed) & 0xFF, 0, r, g, b, 100, 5])

    def restore_default(self):
        self._need()
        if self.protocol == "palit92" and effects_supported(self.id):
            self._stop()
        else:
            raise LedError("Este controlador no tiene función de restauración; reinicia el PC "
                           "para volver al color original.")


# --------------------------------------------------------------------------- #
#  Informe de diagnóstico (solo lectura)
# --------------------------------------------------------------------------- #
def scan_report(bus, led: PalitLed | None = None) -> str:
    led = led or PalitLed(bus)
    lines = [f"GPU: {led.gpu_name}"]

    def dump(dev, first, n, title):
        vals, errs = [], 0
        for i in range(n):
            try:
                vals.append(f"{bus.read(dev, first + i):02X}")
            except LedError:
                vals.append("??")
                errs += 1
        lines.append(f"{title}: " + " ".join(vals) + ("   (sin respuesta)" if errs == n else ""))

    dump(DEV_NEW, 0xF0, 16, "I2C 0x92 regs 0xF0-0xFF")
    dump(DEV_OLD, 0x00, 16, "I2C 0x10 regs 0x00-0x0F")
    lines.append("→ " + led.describe())
    return "\n".join(lines)
