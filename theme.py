"""
theme.py - Sistema de diseño de GPU PY.

Todos los colores, radios, espaciados y fuentes salen de aquí para que la interfaz sea consistente.
"""
from __future__ import annotations

from dataclasses import dataclass

import customtkinter as ctk


@dataclass(frozen=True, slots=True)
class Palette:
    bg: str = "#0B0F17"            # fondo de la ventana
    sidebar: str = "#0F1420"
    surface: str = "#141B29"       # tarjetas
    surface2: str = "#1B2436"      # elementos elevados / hover
    border: str = "#25304A"
    track: str = "#222C42"         # pistas de sliders, medidores
    text: str = "#E8EDF7"
    muted: str = "#8793AB"
    accent: str = "#4F8CFF"
    accent_hover: str = "#6BA0FF"
    accent2: str = "#22D3EE"
    ok: str = "#34D399"
    warn: str = "#FBBF24"
    danger: str = "#F43F5E"
    violet: str = "#A78BFA"
    on_accent: str = "#06101F"     # texto sobre el color de acento


C = Palette()

RADIUS = 16        # tarjetas
RADIUS_SM = 10     # botones, campos
PAD = 20           # margen de página
GAP = 14           # separación entre tarjetas
FONT_FAMILY = "Segoe UI"

_fonts: dict[tuple[int, str], ctk.CTkFont] = {}


def font(size: int = 13, weight: str = "normal") -> ctk.CTkFont:
    """Fuente cacheada (CTkFont necesita que ya exista la ventana raíz)."""
    key = (size, weight)
    if key not in _fonts:
        _fonts[key] = ctk.CTkFont(family=FONT_FAMILY, size=size, weight=weight)
    return _fonts[key]


def setup() -> None:
    ctk.set_appearance_mode("dark")
    ctk.set_default_color_theme("blue")
