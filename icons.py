"""
icons.py - Iconos vectoriales dibujados con Pillow (con supermuestreo para bordes suaves).

No requiere fuentes de iconos ni archivos externos.
"""
from __future__ import annotations

import math
from collections.abc import Callable

from PIL import Image, ImageDraw

SS = 4  # factor de supermuestreo


def _hex(c: str, a: int = 255) -> tuple[int, int, int, int]:
    c = c.lstrip("#")
    return int(c[0:2], 16), int(c[2:4], 16), int(c[4:6], 16), a


def _canvas(size: int) -> tuple[Image.Image, ImageDraw.ImageDraw, float]:
    img = Image.new("RGBA", (size * SS, size * SS), (0, 0, 0, 0))
    return img, ImageDraw.Draw(img), size * SS / 24          # u = píxeles por unidad de una rejilla de 24


# Cada icono se dibuja sobre una rejilla de 24x24 -------------------------------------------------
def _monitor(d, u, col):
    d.arc((3 * u, 5 * u, 21 * u, 23 * u), 180, 360, fill=col, width=int(2.2 * u))
    d.line((12 * u, 14 * u, 16.5 * u, 9 * u), fill=col, width=int(2.2 * u))
    d.ellipse((10.4 * u, 12.4 * u, 13.6 * u, 15.6 * u), fill=col)
    d.line((4 * u, 20 * u, 20 * u, 20 * u), fill=col, width=int(2 * u))


def _bolt(d, u, col):
    pts = [(13.5, 2), (5, 13.5), (11, 13.5), (10, 22), (19, 10), (13, 10)]
    d.polygon([(x * u, y * u) for x, y in pts], fill=col)


def _fan(d, u, col):
    for a in (-90, 30, 150):
        d.pieslice((2 * u, 2 * u, 22 * u, 22 * u), a, a + 52, fill=col)
    d.ellipse((9.2 * u, 9.2 * u, 14.8 * u, 14.8 * u), fill=col)
    d.ellipse((11 * u, 11 * u, 13 * u, 13 * u), fill=(0, 0, 0, 0))


def _led(d, u, col):
    d.ellipse((7.5 * u, 7.5 * u, 16.5 * u, 16.5 * u), fill=col)
    for k in range(8):
        a = math.radians(k * 45)
        d.line((12 * u + 7.6 * u * math.cos(a), 12 * u + 7.6 * u * math.sin(a),
                12 * u + 10.6 * u * math.cos(a), 12 * u + 10.6 * u * math.sin(a)), fill=col, width=int(1.9 * u))


def _profiles(d, u, col):
    for y, kx in ((6, 8), (12, 16), (18, 11)):
        d.line((3 * u, y * u, 21 * u, y * u), fill=col, width=int(1.8 * u))
        d.ellipse(((kx - 2.6) * u, (y - 2.6) * u, (kx + 2.6) * u, (y + 2.6) * u), fill=col)
        d.ellipse(((kx - 1.0) * u, (y - 1.0) * u, (kx + 1.0) * u, (y + 1.0) * u), fill=(0, 0, 0, 0))


def _settings(d, u, col):
    for k in range(8):
        a = math.radians(k * 45)
        d.line((12 * u + 6.5 * u * math.cos(a), 12 * u + 6.5 * u * math.sin(a),
                12 * u + 10.6 * u * math.cos(a), 12 * u + 10.6 * u * math.sin(a)), fill=col, width=int(3.4 * u))
    d.ellipse((4.5 * u, 4.5 * u, 19.5 * u, 19.5 * u), fill=col)
    d.ellipse((8.6 * u, 8.6 * u, 15.4 * u, 15.4 * u), fill=(0, 0, 0, 0))


_ICONS: dict[str, Callable] = {
    "monitor": _monitor, "bolt": _bolt, "fan": _fan, "led": _led, "profiles": _profiles, "settings": _settings,
}


def icon(name: str, size: int = 22, color: str = "#8793AB") -> Image.Image:
    img, d, u = _canvas(size)
    _ICONS[name](d, u, _hex(color))
    return img.resize((size, size), Image.LANCZOS)


# Icono de la aplicación ---------------------------------------------------------------------------
def app_icon(size: int = 256) -> Image.Image:
    """Cuadrado redondeado con degradado azul→cian y un chip de GPU."""
    S = size * 2
    grad = Image.new("RGBA", (S, S))
    a, b = _hex("#4F8CFF"), _hex("#22D3EE")
    px = grad.load()
    for y in range(S):
        for x in range(S):
            t = (x + y) / (2 * S - 2)
            px[x, y] = tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3)) + (255,)
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, S - 1, S - 1), radius=int(S * 0.22), fill=255)
    out = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    out.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(out)
    dark, w = _hex("#08111F"), int(S * 0.05)
    c0, c1 = S * 0.27, S * 0.73
    d.rounded_rectangle((c0, c0, c1, c1), radius=int(S * 0.07), outline=dark, width=w)
    for k in range(3):                                   # patillas del chip
        p = S * (0.37 + k * 0.13)
        d.line((p, S * 0.17, p, c0), fill=dark, width=w)
        d.line((p, c1, p, S * 0.83), fill=dark, width=w)
        d.line((S * 0.17, p, c0, p), fill=dark, width=w)
        d.line((c1, p, S * 0.83, p), fill=dark, width=w)
    bolt = [(0.53, 0.35), (0.42, 0.52), (0.50, 0.52), (0.47, 0.66), (0.60, 0.46), (0.52, 0.46)]
    d.polygon([(x * S, y * S) for x, y in bolt], fill=dark)
    return out.resize((size, size), Image.LANCZOS)


def save_ico(path: str) -> None:
    app_icon(256).save(path, format="ICO", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
