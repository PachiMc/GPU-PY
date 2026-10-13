"""
widgets.py - Componentes visuales reutilizables de GPU PY.

Todo usa los tokens de theme.py. Los medidores y gráficas se dibujan con Pillow con supermuestreo
(bordes suaves) y se muestran como CTkImage.
"""
from __future__ import annotations

import colorsys
import math
import tkinter as tk
from collections import deque
from collections.abc import Callable, Sequence
from functools import lru_cache

import customtkinter as ctk
from PIL import Image, ImageDraw, ImageFont

from theme import C, GAP, RADIUS, RADIUS_SM, font

# --------------------------------------------------------------------------- #
#  Utilidades de color e imagen
# --------------------------------------------------------------------------- #


def rgb(c: str) -> tuple[int, int, int]:
    c = c.lstrip("#")
    return int(c[0:2], 16), int(c[2:4], 16), int(c[4:6], 16)


def hexc(r: float, g: float, b: float) -> str:
    return "#{:02x}{:02x}{:02x}".format(*(max(0, min(255, round(x))) for x in (r, g, b)))


def mix(a: str, b: str, t: float) -> str:
    """Mezcla el color `a` con `b` (t=0 → a, t=1 → b)."""
    ra, rb = rgb(a), rgb(b)
    return hexc(*(ra[i] + (rb[i] - ra[i]) * t for i in range(3)))


def readable_on(bg: str) -> str:
    r, g, b = rgb(bg)
    return "#0B0F17" if (0.299 * r + 0.587 * g + 0.114 * b) > 150 else "#FFFFFF"


@lru_cache(maxsize=16)
def pil_font(size: int) -> ImageFont.ImageFont:
    for name in ("segoeui.ttf", "SegoeUI.ttf", "arial.ttf", "DejaVuSans.ttf", "LiberationSans-Regular.ttf"):
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            continue
    return ImageFont.load_default(size)


def ctk_image(img: Image.Image, size: tuple[int, int]) -> ctk.CTkImage:
    return ctk.CTkImage(light_image=img, dark_image=img, size=size)


# --------------------------------------------------------------------------- #
#  Bloques básicos
# --------------------------------------------------------------------------- #
class Card(ctk.CTkFrame):
    """Tarjeta con borde sutil. Pon el contenido en `card.content`."""

    def __init__(self, master, title: str | None = None, subtitle: str | None = None, **kw):
        super().__init__(master, fg_color=C.surface, corner_radius=RADIUS, border_width=1,
                         border_color=C.border, **kw)
        if title:
            ctk.CTkLabel(self, text=title, font=font(15, "bold"), text_color=C.text, anchor="w").pack(
                fill="x", padx=20, pady=(16, 0))
        if subtitle:
            ctk.CTkLabel(self, text=subtitle, font=font(12), text_color=C.muted, anchor="w",
                         justify="left", wraplength=640).pack(fill="x", padx=20, pady=(2, 0))
        self.content = ctk.CTkFrame(self, fg_color="transparent")
        self.content.pack(fill="both", expand=True, padx=20, pady=(16 if subtitle else 12, 18))


def button(master, text: str, command: Callable | None = None, kind: str = "secondary",
           width: int = 0, image: ctk.CTkImage | None = None) -> ctk.CTkButton:
    styles = {
        "primary": dict(fg_color=C.accent, hover_color=C.accent_hover, text_color=C.on_accent, border_width=0),
        "secondary": dict(fg_color=C.surface2, hover_color=C.track, text_color=C.text, border_width=1,
                          border_color=C.border),
        "ghost": dict(fg_color="transparent", hover_color=C.surface2, text_color=C.muted, border_width=0),
        "danger": dict(fg_color=mix(C.danger, C.surface, 0.8), hover_color=mix(C.danger, C.surface, 0.6),
                       text_color=C.danger, border_width=0),
    }
    kw = styles[kind]
    if width:
        kw["width"] = width
    return ctk.CTkButton(master, text=text, command=command, font=font(13, "bold"), corner_radius=RADIUS_SM,
                         height=38, image=image, **kw)


def pill(master, text: str, color: str) -> ctk.CTkLabel:
    return ctk.CTkLabel(master, text=text, font=font(12, "bold"), text_color=color, corner_radius=999,
                        fg_color=mix(color, C.surface, 0.82), height=26, padx=12)


class PageHeader(ctk.CTkFrame):
    def __init__(self, master, title: str, subtitle: str = ""):
        super().__init__(master, fg_color="transparent")
        ctk.CTkLabel(self, text=title, font=font(26, "bold"), text_color=C.text, anchor="w").pack(fill="x")
        if subtitle:
            ctk.CTkLabel(self, text=subtitle, font=font(13), text_color=C.muted, anchor="w",
                         justify="left", wraplength=760).pack(fill="x", pady=(2, 0))


class Notice(ctk.CTkFrame):
    """Banner informativo (ok / warn / error / info)."""

    def __init__(self, master, text: str, kind: str = "info"):
        color = {"ok": C.ok, "warn": C.warn, "error": C.danger, "info": C.accent}[kind]
        super().__init__(master, fg_color=mix(color, C.surface, 0.86), corner_radius=RADIUS_SM,
                         border_width=1, border_color=mix(color, C.surface, 0.55))
        ctk.CTkLabel(self, text=text, font=font(12), text_color=color, justify="left", anchor="w",
                     wraplength=720).pack(fill="x", padx=14, pady=10)


class LabeledSlider(ctk.CTkFrame):
    """Título + valor a la derecha + pista de ayuda + slider con paso fijo."""

    def __init__(self, master, title: str, hint: str, var: tk.IntVar, lo: int, hi: int, step: int = 1,
                 unit: str = "", color: str = C.accent, signed: bool = False):
        super().__init__(master, fg_color="transparent")
        self.var, self.lo, self.hi, self.step, self.unit, self.signed = var, lo, hi, step, unit, signed
        top = ctk.CTkFrame(self, fg_color="transparent")
        top.pack(fill="x")
        ctk.CTkLabel(top, text=title, font=font(14, "bold"), text_color=C.text, anchor="w").pack(side="left")
        self.value = ctk.CTkLabel(top, text="", font=font(14, "bold"), text_color=color, anchor="e")
        self.value.pack(side="right")
        if hint:
            ctk.CTkLabel(self, text=hint, font=font(12), text_color=C.muted, anchor="w", justify="left",
                         wraplength=640).pack(fill="x")
        steps = max(1, (hi - lo) // step)
        self.slider = ctk.CTkSlider(self, from_=lo, to=hi, number_of_steps=steps, command=self._moved,
                                    progress_color=color, button_color="#FFFFFF", button_hover_color="#DDE6F7",
                                    fg_color=C.track, height=18)
        self.slider.pack(fill="x", pady=(8, 0))
        var.trace_add("write", lambda *_: self._sync())
        self._sync()

    def _moved(self, raw: float) -> None:
        v = int(round(raw / self.step) * self.step)
        v = max(self.lo, min(self.hi, v))
        if v != self.var.get():
            self.var.set(v)

    def _sync(self) -> None:
        try:
            v = self.var.get()
        except tk.TclError:
            return
        if abs(self.slider.get() - v) > 0.5:
            self.slider.set(v)
        self.value.configure(text=f"{v:+d}{self.unit}" if self.signed else f"{v}{self.unit}")


class SwitchRow(ctk.CTkFrame):
    def __init__(self, master, title: str, hint: str, var: tk.IntVar | tk.BooleanVar,
                 command: Callable | None = None):
        super().__init__(master, fg_color="transparent")
        texts = ctk.CTkFrame(self, fg_color="transparent")
        texts.pack(side="left", fill="x", expand=True)
        ctk.CTkLabel(texts, text=title, font=font(14, "bold"), text_color=C.text, anchor="w").pack(fill="x")
        if hint:
            ctk.CTkLabel(texts, text=hint, font=font(12), text_color=C.muted, anchor="w", justify="left",
                         wraplength=520).pack(fill="x")
        self.switch = ctk.CTkSwitch(self, text="", variable=var, onvalue=1, offvalue=0, command=command,
                                    progress_color=C.accent, button_color="#FFFFFF", button_hover_color="#DDE6F7",
                                    fg_color=C.track, width=46)
        self.switch.pack(side="right", padx=(12, 0))


def segmented(master, values: Sequence[str], variable: tk.StringVar, command: Callable | None = None):
    return ctk.CTkSegmentedButton(master, values=list(values), variable=variable, command=command,
                                  font=font(13, "bold"), corner_radius=RADIUS_SM, height=38,
                                  selected_color=C.accent, selected_hover_color=C.accent_hover,
                                  unselected_color=C.surface2, unselected_hover_color=C.track,
                                  fg_color=C.surface2, text_color=C.text, text_color_disabled=C.muted)


def option_menu(master, values: Sequence[str], variable: tk.StringVar, command: Callable | None = None,
                width: int = 200):
    return ctk.CTkOptionMenu(master, values=list(values), variable=variable, command=command, width=width,
                             height=36, font=font(13), dropdown_font=font(13), corner_radius=RADIUS_SM,
                             fg_color=C.surface2, button_color=C.track, button_hover_color=C.border,
                             dropdown_fg_color=C.surface2, dropdown_hover_color=C.track,
                             dropdown_text_color=C.text, text_color=C.text)


def entry(master, variable: tk.StringVar | None = None, width: int = 70, placeholder: str = "",
          justify: str = "center") -> ctk.CTkEntry:
    return ctk.CTkEntry(master, textvariable=variable, width=width, height=34, font=font(13),
                        corner_radius=RADIUS_SM, fg_color=C.surface2, border_color=C.border, text_color=C.text,
                        placeholder_text=placeholder, justify=justify)


def checkbox(master, text: str, variable: tk.IntVar, command: Callable | None = None) -> ctk.CTkCheckBox:
    return ctk.CTkCheckBox(master, text=text, variable=variable, command=command, font=font(13),
                           text_color=C.text, fg_color=C.accent, hover_color=C.accent_hover,
                           border_color=C.border, checkmark_color=C.on_accent, corner_radius=6,
                           checkbox_width=20, checkbox_height=20)


# --------------------------------------------------------------------------- #
#  Medidor circular animado
# --------------------------------------------------------------------------- #
def render_gauge(size: int, frac: float, color: str, track: str = C.track, ss: int = 3) -> Image.Image:
    S = size * ss
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    w = int(S * 0.085)
    pad = w // 2 + int(S * 0.035)
    box = (pad, pad, S - pad, S - pad)
    cx = cy = S / 2
    rc = (S - 2 * pad) / 2 - w / 2          # radio del centro del trazo

    def cap(angle: float, fill: str) -> None:
        a = math.radians(angle)
        x, y = cx + rc * math.cos(a), cy + rc * math.sin(a)
        d.ellipse((x - w / 2, y - w / 2, x + w / 2, y + w / 2), fill=fill)

    d.arc(box, 135, 45, fill=track, width=w)
    cap(135, track)
    cap(45, track)
    frac = max(0.0, min(1.0, frac))
    if frac > 0.004:
        end = 135 + 270 * frac
        d.arc(box, 135, end % 360, fill=color, width=w)
        cap(135, color)
        cap(end, color)
    return img.resize((size, size), Image.LANCZOS)


class Gauge(ctk.CTkFrame):
    def __init__(self, master, label: str, unit: str, vmax: float, size: int = 168, color: str = C.accent,
                 warn: tuple[float, float] | None = None):
        super().__init__(master, fg_color="transparent")
        self.size, self.vmax, self.color, self.warn = size, max(1.0, float(vmax)), color, warn
        self._cur = self._target = 0.0
        self._job: str | None = None
        self.canvas = ctk.CTkLabel(self, text="", width=size, height=size, fg_color="transparent")
        self.canvas.pack()
        self.value = ctk.CTkLabel(self, text="--", font=font(30, "bold"), text_color=C.text, fg_color="transparent")
        self.value.place(relx=0.5, y=size * 0.45, anchor="center")
        ctk.CTkLabel(self, text=unit, font=font(13), text_color=C.muted, fg_color="transparent").place(
            relx=0.5, y=size * 0.64, anchor="center")
        ctk.CTkLabel(self, text=label.upper(), font=font(11, "bold"), text_color=C.muted,
                     fg_color="transparent").place(relx=0.5, y=size * 0.9, anchor="center")
        self._paint()

    def _tone(self, v: float) -> str:
        if self.warn:
            return C.danger if v >= self.warn[1] else C.warn if v >= self.warn[0] else self.color
        return self.color

    def _paint(self) -> None:
        img = render_gauge(self.size, self._cur / self.vmax, self._tone(self._cur))
        self._img = ctk_image(img, (self.size, self.size))       # referencia viva
        self.canvas.configure(image=self._img)
        self.value.configure(text=f"{self._target:.0f}")

    def set(self, value: float) -> None:
        self._target = float(value)
        if self._job is None:
            self._tick()

    def _tick(self) -> None:
        delta = self._target - self._cur
        if abs(delta) < self.vmax * 0.002:
            self._cur, self._job = self._target, None
        else:
            self._cur += delta * 0.28
            self._job = self.after(33, self._tick)
        self._paint()


# --------------------------------------------------------------------------- #
#  Gráficas
# --------------------------------------------------------------------------- #
class HistoryChart(ctk.CTkLabel):
    """Líneas con relleno suave, escala fija 0-100."""

    def __init__(self, master, series: dict[str, tuple[deque, str]], size: tuple[int, int] = (560, 190)):
        super().__init__(master, text="", fg_color="transparent")
        self.series, self.dims = series, size
        self.redraw()

    def redraw(self) -> None:
        w, h = self.dims
        ss = 2
        W, H = w * ss, h * ss
        img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
        d = ImageDraw.Draw(img)
        l, r, t, b = 34 * ss, W - 8 * ss, 8 * ss, H - 10 * ss
        f = pil_font(10 * ss)
        for v in (0, 25, 50, 75, 100):
            y = b - (b - t) * v / 100
            d.line((l, y, r, y), fill=rgb(C.track) + (255,), width=ss)
            d.text((l - 8 * ss, y), str(v), fill=rgb(C.muted) + (255,), font=f, anchor="rm")
        for dq, color in self.series.values():
            if len(dq) < 2:
                continue
            step = (r - l) / (dq.maxlen - 1)
            pts = [(l + i * step, b - (b - t) * max(0, min(100, v)) / 100) for i, v in enumerate(dq)]
            fill = Image.new("RGBA", (W, H), (0, 0, 0, 0))
            ImageDraw.Draw(fill).polygon(pts + [(pts[-1][0], b), (pts[0][0], b)], fill=rgb(color) + (34,))
            img.alpha_composite(fill)
            d.line(pts, fill=rgb(color) + (255,), width=int(2.4 * ss), joint="curve")
        out = img.resize((w, h), Image.LANCZOS)
        self._img = ctk_image(out, (w, h))
        self.configure(image=self._img)


class CurvePreview(ctk.CTkLabel):
    """Vista previa de la curva de ventiladores (20-95 °C → 0-100 %)."""

    def __init__(self, master, size: tuple[int, int] = (420, 220)):
        super().__init__(master, text="", fg_color="transparent")
        self.dims, self.points, self.marker = size, [], None
        self.redraw()

    def update_curve(self, points: Sequence[tuple[int, int]], marker: float | None = None) -> None:
        self.points, self.marker = sorted(points), marker
        self.redraw()

    def redraw(self) -> None:
        w, h = self.dims
        ss = 2
        W, H = w * ss, h * ss
        img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
        d = ImageDraw.Draw(img)
        l, r, t, b = 38 * ss, W - 12 * ss, 10 * ss, H - 24 * ss
        f = pil_font(10 * ss)

        def X(temp): return l + (r - l) * (temp - 20) / 75
        def Y(pct): return b - (b - t) * pct / 100

        for pct in (0, 25, 50, 75, 100):
            d.line((l, Y(pct), r, Y(pct)), fill=rgb(C.track) + (255,), width=ss)
            d.text((l - 8 * ss, Y(pct)), f"{pct}%", fill=rgb(C.muted) + (255,), font=f, anchor="rm")
        for temp in (20, 40, 60, 80):
            d.text((X(temp), b + 12 * ss), f"{temp}°", fill=rgb(C.muted) + (255,), font=f, anchor="mm")
        if self.marker is not None:
            x = X(max(20, min(95, self.marker)))
            d.line((x, t, x, b), fill=rgb(C.warn) + (200,), width=ss)
        if len(self.points) >= 2:
            pts = [(X(tp), Y(pc)) for tp, pc in self.points]
            line = [(l, pts[0][1])] + pts + [(r, pts[-1][1])]
            fill = Image.new("RGBA", (W, H), (0, 0, 0, 0))
            ImageDraw.Draw(fill).polygon(line + [(r, b), (l, b)], fill=rgb(C.ok) + (40,))
            img.alpha_composite(fill)
            d.line(line, fill=rgb(C.ok) + (255,), width=int(2.6 * ss), joint="curve")
            for x, y in pts:
                d.ellipse((x - 5 * ss, y - 5 * ss, x + 5 * ss, y + 5 * ss), fill=rgb(C.surface) + (255,),
                          outline=rgb(C.ok) + (255,), width=int(2 * ss))
        out = img.resize((w, h), Image.LANCZOS)
        self._img = ctk_image(out, (w, h))
        self.configure(image=self._img)


# --------------------------------------------------------------------------- #
#  Aviso flotante y diálogos propios (sin ventanas nativas)
# --------------------------------------------------------------------------- #
class Toast(ctk.CTkFrame):
    COLORS = {"ok": C.ok, "warn": C.warn, "error": C.danger, "info": C.accent}

    def __init__(self, master):
        super().__init__(master, fg_color=C.surface2, corner_radius=RADIUS_SM, border_width=1,
                         border_color=C.border)
        self.bar = ctk.CTkFrame(self, width=4, corner_radius=2, fg_color=C.ok)
        self.bar.pack(side="left", fill="y", padx=(10, 0), pady=10)
        self.label = ctk.CTkLabel(self, text="", font=font(13), text_color=C.text, justify="left",
                                  wraplength=430, anchor="w")
        self.label.pack(side="left", padx=(12, 16), pady=12)
        self._job: str | None = None

    def show(self, message: str, kind: str = "ok", ms: int = 3800) -> None:
        self.bar.configure(fg_color=self.COLORS.get(kind, C.accent))
        self.label.configure(text=message)
        self.place(relx=1.0, rely=1.0, x=-24, y=-24, anchor="se")
        self.lift()
        if self._job:
            self.after_cancel(self._job)
        self._job = self.after(ms, self.hide)

    def hide(self) -> None:
        self.place_forget()
        self._job = None


def _center(dlg: ctk.CTkToplevel, parent) -> None:
    dlg.update_idletasks()
    w, h = dlg.winfo_reqwidth(), dlg.winfo_reqheight()
    x = parent.winfo_rootx() + (parent.winfo_width() - w) // 2
    y = parent.winfo_rooty() + (parent.winfo_height() - h) // 3
    dlg.geometry(f"{w}x{h}+{max(0, x)}+{max(0, y)}")


def ask(parent, title: str, message: str, ok: str = "Aceptar", cancel: str | None = "Cancelar",
        danger: bool = False) -> bool:
    """Diálogo modal con el estilo de la app. Devuelve True si se aceptó."""
    dlg = ctk.CTkToplevel(parent)
    dlg.title(title)
    dlg.configure(fg_color=C.surface)
    dlg.resizable(False, False)
    result = {"ok": False}

    def close(value: bool) -> None:
        result["ok"] = value
        dlg.destroy()

    ctk.CTkLabel(dlg, text=title, font=font(17, "bold"), text_color=C.text, anchor="w").pack(
        fill="x", padx=24, pady=(22, 4))
    ctk.CTkLabel(dlg, text=message, font=font(13), text_color=C.muted, justify="left", anchor="w",
                 wraplength=400).pack(fill="x", padx=24, pady=(0, 18))
    row = ctk.CTkFrame(dlg, fg_color="transparent")
    row.pack(fill="x", padx=24, pady=(0, 22))
    button(row, ok, lambda: close(True), "danger" if danger else "primary", width=120).pack(side="right")
    if cancel:
        button(row, cancel, lambda: close(False), "secondary", width=110).pack(side="right", padx=(0, 10))
    dlg.protocol("WM_DELETE_WINDOW", lambda: close(False))
    dlg.transient(parent)
    _center(dlg, parent)
    dlg.after(120, lambda: dlg.winfo_exists() and dlg.grab_set())
    parent.wait_window(dlg)
    return result["ok"]


PRESET_COLORS = ["#ff2d55", "#ff9500", "#ffd60a", "#30d158", "#00c7be", "#0a84ff", "#bf5af2", "#ffffff"]


def pick_color(parent, initial: str = "#ff2d55", title: str = "Elige un color") -> str | None:
    """Selector de color propio (matiz, saturación, brillo, #hex y colores rápidos)."""
    dlg = ctk.CTkToplevel(parent)
    dlg.title(title)
    dlg.configure(fg_color=C.surface)
    dlg.resizable(False, False)
    out: dict[str, str | None] = {"v": None}
    h, s, v = colorsys.rgb_to_hsv(*(x / 255 for x in rgb(initial)))
    hv, sv, vv = tk.DoubleVar(value=h), tk.DoubleVar(value=s), tk.DoubleVar(value=v)
    hx = tk.StringVar(value=initial)
    busy = {"on": False}

    ctk.CTkLabel(dlg, text=title, font=font(17, "bold"), text_color=C.text, anchor="w").pack(
        fill="x", padx=24, pady=(22, 10))
    preview = ctk.CTkFrame(dlg, height=64, corner_radius=RADIUS_SM, fg_color=initial)
    preview.pack(fill="x", padx=24)

    def current() -> str:
        return hexc(*(c * 255 for c in colorsys.hsv_to_rgb(hv.get(), sv.get(), vv.get())))

    def refresh(*_):
        if busy["on"]:
            return
        c = current()
        preview.configure(fg_color=c)
        hx.set(c)

    for label, var in (("Matiz", hv), ("Saturación", sv), ("Brillo", vv)):
        row = ctk.CTkFrame(dlg, fg_color="transparent")
        row.pack(fill="x", padx=24, pady=(10, 0))
        ctk.CTkLabel(row, text=label, font=font(12, "bold"), text_color=C.muted, width=80, anchor="w").pack(side="left")
        ctk.CTkSlider(row, from_=0, to=1, variable=var, command=refresh, progress_color=C.accent,
                      button_color="#FFFFFF", fg_color=C.track, width=300).pack(side="left", fill="x", expand=True)

    def from_hex(*_):
        t = hx.get().strip()
        if len(t) == 7 and t.startswith("#"):
            try:
                hh, ss_, vvv = colorsys.rgb_to_hsv(*(x / 255 for x in rgb(t)))
            except ValueError:
                return
            busy["on"] = True
            hv.set(hh), sv.set(ss_), vv.set(vvv)
            busy["on"] = False
            preview.configure(fg_color=t)

    ent = entry(dlg, hx, width=110)
    ent.pack(anchor="w", padx=24, pady=(14, 0))
    ent.bind("<KeyRelease>", from_hex)

    quick = ctk.CTkFrame(dlg, fg_color="transparent")
    quick.pack(fill="x", padx=24, pady=(12, 0))
    for c in PRESET_COLORS:
        ctk.CTkButton(quick, text="", width=30, height=30, corner_radius=8, fg_color=c, hover_color=c,
                      border_width=1, border_color=C.border,
                      command=lambda c=c: (hx.set(c), from_hex())).pack(side="left", padx=(0, 6))

    def done(ok: bool) -> None:
        out["v"] = current() if ok else None
        dlg.destroy()

    row = ctk.CTkFrame(dlg, fg_color="transparent")
    row.pack(fill="x", padx=24, pady=(18, 22))
    button(row, "Aceptar", lambda: done(True), "primary", width=110).pack(side="right")
    button(row, "Cancelar", lambda: done(False), "secondary", width=110).pack(side="right", padx=(0, 10))
    dlg.protocol("WM_DELETE_WINDOW", lambda: done(False))
    dlg.transient(parent)
    _center(dlg, parent)
    dlg.after(120, lambda: dlg.winfo_exists() and dlg.grab_set())
    parent.wait_window(dlg)
    return out["v"]
