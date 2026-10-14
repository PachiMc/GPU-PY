"""
ui.py - Ventana principal de GPU PY (CustomTkinter): barra lateral + páginas.
"""
from __future__ import annotations

import os
import subprocess
import sys
import tkinter as tk
from collections import deque

import customtkinter as ctk

import autostart
import icons
import theme
import tray
from core import (APP_NAME, DEFAULT_CURVE, SAFETY_TEMP, VERSION, ProfileStore, Settings, apply_led_state,
                  apply_profile_to_backend, data_dir, default_led_state, default_profile, interpolate,
                  resource_path)
from gpu_backend import MIN_FAN_PCT, BackendError, is_admin
from led_nvapi import LedError
from theme import C, GAP, PAD, RADIUS_SM, font
from widgets import (PRESET_COLORS, Card, CurvePreview, Gauge, HistoryChart, LabeledSlider, Notice, PageHeader,
                     SwitchRow, Toast, ask, button, checkbox, ctk_image, entry, mix, option_menu, pick_color, pill,
                     readable_on, segmented)

NAV = [("monitor", "Monitor", "monitor"), ("oc", "Overclock", "bolt"), ("fan", "Ventiladores", "fan"),
       ("led", "LED / RGB", "led"), ("profiles", "Perfiles", "profiles"), ("settings", "Ajustes", "settings")]

FAN_LABELS = {"auto": "Automático", "manual": "Manual", "curve": "Curva"}
LED_LABELS = {"keep": "No cambiar", "static": "Color fijo", "breathing": "Respiración", "off": "Apagado",
              "cycle": "Ciclo", "strobe": "Estrobo"}


def _invert(d: dict) -> dict:
    return {v: k for k, v in d.items()}


class App(ctk.CTk):
    def __init__(self, be, led=None, led_error: str | None = None, startup: bool = False):
        theme.setup()
        super().__init__()
        self.be, self.L = be, be.limits
        self.led, self.led_error = led, led_error
        self.store, self.settings = ProfileStore(), Settings.load()
        self.risk_ack = False
        self.fan_supported = self.L.num_fans > 0 or be.demo
        self.last_fan: int | None = None
        self.hist = {k: deque(maxlen=120) for k in ("temp", "util", "power")}
        self._startup_mode, self._startup_job = startup, None

        self.title(APP_NAME)
        self.geometry("1120x760")
        self.minsize(1020, 700)
        self.configure(fg_color=C.bg)
        self._set_icon()

        self._make_vars()
        self.columnconfigure(1, weight=1)
        self.rowconfigure(0, weight=1)
        self._build_sidebar()
        self.stage = ctk.CTkFrame(self, fg_color="transparent")
        self.stage.grid(row=0, column=1, sticky="nsew")
        self.stage.rowconfigure(0, weight=1)
        self.stage.columnconfigure(0, weight=1)
        self.pages: dict[str, ctk.CTkScrollableFrame] = {}
        for key, builder in (("monitor", self._page_monitor), ("oc", self._page_oc), ("fan", self._page_fan),
                             ("led", self._page_led), ("profiles", self._page_profiles),
                             ("settings", self._page_settings)):
            page = ctk.CTkScrollableFrame(self.stage, fg_color="transparent", corner_radius=0,
                                          scrollbar_button_color=C.track, scrollbar_button_hover_color=C.border)
            page.grid(row=0, column=0, sticky="nsew")
            builder(page)
            self.pages[key] = page
        self.toast = Toast(self)
        self.show("monitor")
        self.load_current_values()

        self.protocol("WM_DELETE_WINDOW", self.on_close)
        self._in_tray = False
        self._tray = tray.TrayController(self, self._show_from_tray, self._apply_from_tray,
                                         self._quit_from_tray)
        self.bind("<Unmap>", self._on_unmap)
        if startup:
            if self.settings.start_minimized:
                if self.settings.tray_enabled:
                    self._hide_to_tray(silent=True)
                else:
                    self.iconify()
            self._startup_job = self.after(1500, self.run_startup)
        self._poll_job = self.after(300, self.poll)

    # ------------------------------------------------------------------------
    #  Infraestructura
    # ------------------------------------------------------------------------
    def _set_icon(self) -> None:
        try:
            ico = resource_path("assets/gpupy.ico")
            if os.name == "nt" and ico.exists():
                self.iconbitmap(str(ico))
                self.after(300, lambda: self.iconbitmap(str(ico)))     # CTk repone su icono tras 200 ms
            else:
                from PIL import ImageTk
                self._icon_photo = ImageTk.PhotoImage(icons.app_icon(64))
                self.iconphoto(True, self._icon_photo)
        except Exception:
            pass

    def _make_vars(self) -> None:
        L = self.L
        self.power = tk.IntVar(value=int(L.power_default_w))
        self.core, self.mem = tk.IntVar(value=0), tk.IntVar(value=0)
        self.lock_on, self.lock_mhz = tk.IntVar(value=0), tk.IntVar(value=int(L.max_core_clock))
        self.fan_mode, self.fan_manual = tk.StringVar(value="auto"), tk.IntVar(value=50)
        self.curve_vars = [(tk.StringVar(value=str(t)), tk.StringVar(value=str(p))) for t, p in DEFAULT_CURVE]
        d = default_led_state()
        self.led_mode = tk.StringVar(value=d["mode"])
        self.led_c1, self.led_c2 = tk.StringVar(value=d["color"]), tk.StringVar(value=d["color2"])
        self.led_bright, self.led_period = tk.IntVar(value=d["brightness"]), tk.IntVar(value=d["period"])
        self.led_speed, self.led_reverse = tk.IntVar(value=d["speed"]), tk.IntVar(value=0)
        self.led_zone_vars: list[tk.IntVar] = []
        self.sel_profile = tk.StringVar(value="")
        self.pname = tk.StringVar(value="")

    def notify(self, message: str, kind: str = "ok") -> None:
        if getattr(self, "_in_tray", False):
            self._tray.notify(message)
        else:
            self.toast.show(message, kind)

    def alert(self, title: str, message: str) -> None:
        ask(self, title, message, ok="Entendido", cancel=None)

    # ------------------------------------------------------------------------
    #  Barra lateral
    # ------------------------------------------------------------------------
    def _build_sidebar(self) -> None:
        sb = ctk.CTkFrame(self, width=236, corner_radius=0, fg_color=C.sidebar)
        sb.grid(row=0, column=0, sticky="ns")
        sb.grid_propagate(False)
        sb.rowconfigure(2, weight=1)

        brand = ctk.CTkFrame(sb, fg_color="transparent")
        brand.grid(row=0, column=0, sticky="w", padx=22, pady=(26, 22))
        logo = ctk_image(icons.app_icon(96), (38, 38))
        ctk.CTkLabel(brand, text="", image=logo).pack(side="left")
        ctk.CTkLabel(brand, text="GPU", font=font(22, "bold"), text_color=C.text).pack(side="left", padx=(10, 0))
        ctk.CTkLabel(brand, text="PY", font=font(22, "bold"), text_color=C.accent).pack(side="left", padx=(6, 0))

        nav = ctk.CTkFrame(sb, fg_color="transparent")
        nav.grid(row=1, column=0, sticky="ew", padx=14)
        self.nav_buttons: dict[str, ctk.CTkButton] = {}
        self._nav_icons: dict[str, tuple[ctk.CTkImage, ctk.CTkImage]] = {}
        for key, label, ico in NAV:
            off = ctk_image(icons.icon(ico, 44, C.muted), (22, 22))
            on = ctk_image(icons.icon(ico, 44, C.accent), (22, 22))
            self._nav_icons[key] = (off, on)
            b = ctk.CTkButton(nav, text=f"  {label}", image=off, anchor="w", height=46, corner_radius=12,
                              fg_color="transparent", hover_color=C.surface2, text_color=C.muted,
                              font=font(14, "bold"), command=lambda k=key: self.show(k))
            b.pack(fill="x", pady=2)
            self.nav_buttons[key] = b

        foot = ctk.CTkFrame(sb, fg_color="transparent")
        foot.grid(row=3, column=0, sticky="ew", padx=22, pady=(10, 22))
        ctk.CTkLabel(foot, text=self.be.name.replace("NVIDIA ", ""), font=font(13, "bold"), text_color=C.text,
                     wraplength=190, justify="left", anchor="w").pack(fill="x")
        ctk.CTkLabel(foot, text=f"Driver {self.be.driver}  ·  v{VERSION}", font=font(11), text_color=C.muted,
                     anchor="w").pack(fill="x", pady=(0, 10))
        admin = is_admin() or self.be.demo
        pill(foot, "Administrador" if admin else "Sin permisos", C.ok if admin else C.warn).pack(anchor="w")
        if self.be.demo:
            pill(foot, "Modo demo", C.violet).pack(anchor="w", pady=(6, 0))

    def show(self, key: str) -> None:
        for k, page in self.pages.items():
            page.grid() if k == key else page.grid_remove()
        for k, b in self.nav_buttons.items():
            active = k == key
            b.configure(fg_color=mix(C.accent, C.sidebar, 0.86) if active else "transparent",
                        text_color=C.accent if active else C.muted, image=self._nav_icons[k][1 if active else 0])
        self.current_page = key

    # ------------------------------------------------------------------------
    #  Página: Monitor
    # ------------------------------------------------------------------------
    def _header(self, page, title: str, subtitle: str = "") -> None:
        PageHeader(page, title, subtitle).pack(fill="x", padx=PAD, pady=(PAD, GAP))

    def _page_monitor(self, page) -> None:
        self._header(page, "Monitor", "Estado de tu GPU en tiempo real.")
        L = self.L
        card = Card(page)
        card.pack(fill="x", padx=PAD, pady=(0, GAP))
        row = card.content
        for i in range(4):
            row.columnconfigure(i, weight=1)
        self.g_temp = Gauge(row, "Temperatura", "°C", 100, warn=(75, 85), color=C.accent2)
        self.g_core = Gauge(row, "Núcleo", "MHz", L.max_core_clock, color=C.accent)
        self.g_mem = Gauge(row, "Memoria", "MHz", L.max_mem_clock + 1000, color=C.violet)
        self.g_fan = Gauge(row, "Ventilador", "%", 100, color=C.ok)
        for i, g in enumerate((self.g_temp, self.g_core, self.g_mem, self.g_fan)):
            g.grid(row=0, column=i, padx=4)

        low = ctk.CTkFrame(page, fg_color="transparent")
        low.pack(fill="x", padx=PAD, pady=(0, PAD))
        low.columnconfigure(0, weight=1)
        left = Card(low, "Historial", "Últimos 2 minutos")
        left.grid(row=0, column=0, sticky="nsew", padx=(0, GAP))
        self.chart = HistoryChart(left.content, {"Temperatura": (self.hist["temp"], C.accent2),
                                                 "Uso GPU": (self.hist["util"], C.accent),
                                                 "Potencia": (self.hist["power"], C.warn)}, size=(520, 190))
        self.chart.pack(anchor="w")
        legend = ctk.CTkFrame(left.content, fg_color="transparent")
        legend.pack(anchor="w", pady=(6, 0))
        for name, color in (("Temperatura °C", C.accent2), ("Uso GPU %", C.accent), ("Potencia % del límite", C.warn)):
            ctk.CTkLabel(legend, text="●", text_color=color, font=font(12)).pack(side="left")
            ctk.CTkLabel(legend, text=name + "   ", text_color=C.muted, font=font(12)).pack(side="left")

        right = Card(low, "Detalles")
        right.grid(row=0, column=1, sticky="nsew")
        self.stat = {k: tk.StringVar(value="--") for k in ("power", "util", "vram")}
        for label, key in (("Potencia", "power"), ("Uso de GPU", "util"), ("VRAM", "vram")):
            ctk.CTkLabel(right.content, text=label, font=font(12), text_color=C.muted, anchor="w").pack(fill="x")
            ctk.CTkLabel(right.content, textvariable=self.stat[key], font=font(15, "bold"), text_color=C.text,
                         anchor="w").pack(fill="x", pady=(0, 2))
            bar = ctk.CTkProgressBar(right.content, height=6, progress_color=C.accent, fg_color=C.track)
            bar.set(0)
            bar.pack(fill="x", pady=(0, 12))
            setattr(self, f"bar_{key}", bar)
        ctk.CTkLabel(right.content, text="Estado", font=font(12), text_color=C.muted, anchor="w").pack(fill="x")
        self.state_box = ctk.CTkFrame(right.content, fg_color="transparent")
        self.state_box.pack(fill="x", pady=(4, 0))
        self._set_state_pill("Sin limitaciones", C.ok)

    def _set_state_pill(self, text: str, color: str) -> None:
        for w in self.state_box.winfo_children():
            w.destroy()
        pill(self.state_box, text, color).pack(anchor="w")

    # ------------------------------------------------------------------------
    #  Página: Overclock
    # ------------------------------------------------------------------------
    def _slider_for(self, parent, title, hint, var, lo, hi, step, unit, color=C.accent, signed=False):
        disabled = lo >= hi
        if disabled:
            hi = lo + step
        s = LabeledSlider(parent, title, hint, var, int(lo), int(hi), step, unit, color, signed)
        if disabled:
            s.slider.configure(state="disabled")
        return s

    def _page_oc(self, page) -> None:
        self._header(page, "Overclock", "Ajusta potencia y frecuencias. Los cambios se pierden al reiniciar el PC.")
        L = self.L
        if not L.oc_supported and not self.be.demo:
            Notice(page, "No se pudo acceder a los offsets de reloj (ni por NVML ni por NVAPI).\n"
                         f"Motivo: {getattr(self.be, 'oc_error', None) or 'desconocido'}\n"
                         "Ejecuta  python gpupy.py --oc-info  y envíame el resultado.", "warn").pack(
                fill="x", padx=PAD, pady=(0, GAP))
        elif getattr(self.be, "oc_source", None) == "nvapi":
            Notice(page, "Overclock vía NVAPI, la misma vía que usa VelocityX.", "ok").pack(
                fill="x", padx=PAD, pady=(0, GAP))

        card = Card(page, "Potencia y frecuencias")
        card.pack(fill="x", padx=PAD, pady=(0, GAP))
        c = card.content
        for s in (self._slider_for(c, "Límite de potencia", f"Rango del driver: {L.power_min_w:.0f}–{L.power_max_w:.0f} W",
                                   self.power, int(L.power_min_w), int(L.power_max_w), 1, " W", C.warn),
                  self._slider_for(c, "Offset del núcleo", "Súbelo de 15 en 15 MHz y prueba la estabilidad",
                                   self.core, L.core_off_min, L.core_off_max, 5, " MHz", C.accent, True),
                  self._slider_for(c, "Offset de la memoria", "Escala del driver (igual que VelocityX). Sube de 50 en 50",
                                   self.mem, L.mem_off_min, L.mem_off_max, 10, " MHz", C.violet, True)):
            s.pack(fill="x", pady=(0, 18))

        lock = Card(page, "Techo de reloj", "Útil para undervolt: limita la frecuencia máxima del núcleo.")
        lock.pack(fill="x", padx=PAD, pady=(0, GAP))
        SwitchRow(lock.content, "Activar techo de reloj", "", self.lock_on).pack(fill="x", pady=(0, 14))
        self._slider_for(lock.content, "Reloj máximo del núcleo", "", self.lock_mhz, 800,
                         max(900, L.max_core_clock), 15, " MHz", C.accent2).pack(fill="x")

        bar = ctk.CTkFrame(page, fg_color="transparent")
        bar.pack(fill="x", padx=PAD, pady=(0, PAD))
        button(bar, "Aplicar", self.apply_oc, "primary", width=140).pack(side="left")
        button(bar, "Valores de fábrica", self.reset_all, "secondary", width=170).pack(side="left", padx=10)

    # ------------------------------------------------------------------------
    #  Página: Ventiladores
    # ------------------------------------------------------------------------
    def _page_fan(self, page) -> None:
        self._header(page, "Ventiladores", "Control automático, fijo o con una curva personalizada.")
        if not self.fan_supported:
            Notice(page, "No se detectaron ventiladores controlables por NVML.", "warn").pack(
                fill="x", padx=PAD, pady=(0, GAP))

        mode = Card(page, "Modo")
        mode.pack(fill="x", padx=PAD, pady=(0, GAP))
        self.fan_label = tk.StringVar(value=FAN_LABELS["auto"])
        segmented(mode.content, list(FAN_LABELS.values()), self.fan_label, self._fan_mode_changed).pack(anchor="w")

        manual = Card(page, "Velocidad fija")
        manual.pack(fill="x", padx=PAD, pady=(0, GAP))
        LabeledSlider(manual.content, "Velocidad manual", "", self.fan_manual, MIN_FAN_PCT, 100, 1, " %",
                      C.ok).pack(fill="x")

        curve = Card(page, "Curva personalizada", f"A ≥ {SAFETY_TEMP} °C el ventilador se fuerza al 100 % "
                                                   "y al cerrar GPU PY vuelve a automático.")
        curve.pack(fill="x", padx=PAD, pady=(0, GAP))
        body = curve.content
        self.curve_preview = CurvePreview(body, (440, 230))
        self.curve_preview.pack(side="left", anchor="n")
        grid = ctk.CTkFrame(body, fg_color="transparent")
        grid.pack(side="left", padx=(24, 0), anchor="n")
        for col, txt in ((1, "Temperatura"), (3, "Velocidad")):
            ctk.CTkLabel(grid, text=txt, font=font(11, "bold"), text_color=C.muted).grid(row=0, column=col, columnspan=2)
        for i, (tv, pv) in enumerate(self.curve_vars):
            ctk.CTkLabel(grid, text=f"Punto {i + 1}", font=font(12), text_color=C.muted).grid(
                row=i + 1, column=0, padx=(0, 10), pady=4)
            entry(grid, tv, 64).grid(row=i + 1, column=1)
            ctk.CTkLabel(grid, text="°C", font=font(12), text_color=C.muted).grid(row=i + 1, column=2, padx=(4, 14))
            entry(grid, pv, 64).grid(row=i + 1, column=3)
            ctk.CTkLabel(grid, text="%", font=font(12), text_color=C.muted).grid(row=i + 1, column=4, padx=4)
            tv.trace_add("write", lambda *_: self._refresh_curve())
            pv.trace_add("write", lambda *_: self._refresh_curve())
        self._refresh_curve()
        button(page, "Aplicar", self.apply_fan, "primary", width=140).pack(anchor="w", padx=PAD, pady=(0, PAD))

    def _fan_mode_changed(self, label: str) -> None:
        self.fan_mode.set(_invert(FAN_LABELS)[label])

    def _refresh_curve(self, marker: float | None = None) -> None:
        if hasattr(self, "curve_preview"):
            self.curve_preview.update_curve(self.get_curve(), marker)

    def get_curve(self) -> list[tuple[int, int]]:
        pts = []
        for tv, pv in self.curve_vars:
            try:
                t, p = int(tv.get()), int(pv.get())
            except ValueError:
                continue
            pts.append((max(20, min(95, t)), max(MIN_FAN_PCT, min(100, p))))
        return sorted(pts) if len(pts) >= 2 else list(DEFAULT_CURVE)

    # ------------------------------------------------------------------------
    #  Página: LED / RGB
    # ------------------------------------------------------------------------
    def _page_led(self, page) -> None:
        self._header(page, "LED / RGB", "Iluminación de tu tarjeta con el mismo mecanismo que VelocityX.")
        effects = bool(getattr(self.led, "supports_effects", False))
        if self.led:
            Notice(page, self.led.describe(), "ok" if self.led.num_zones else "warn").pack(
                fill="x", padx=PAD, pady=(0, GAP))
        else:
            Notice(page, f"LED no disponible: {self.led_error or 'sin información'}\n"
                         "Ejecuta  python gpupy.py --i2c-scan  y envíame el archivo led_report.txt.",
                   "warn").pack(fill="x", padx=PAD, pady=(0, GAP))

        mode = Card(page, "Efecto")
        mode.pack(fill="x", padx=PAD, pady=(0, GAP))
        keys = ["keep", "static", "breathing", "off"] + (["cycle", "strobe"] if effects else [])
        self.led_label = tk.StringVar(value=LED_LABELS[self.led_mode.get()])
        segmented(mode.content, [LED_LABELS[k] for k in keys], self.led_label,
                  lambda lab: self.led_mode.set(_invert(LED_LABELS)[lab])).pack(anchor="w")

        colors = Card(page, "Colores")
        colors.pack(fill="x", padx=PAD, pady=(0, GAP))
        row = ctk.CTkFrame(colors.content, fg_color="transparent")
        row.pack(fill="x")
        self.sw1 = self._swatch(row, "Color A", self.led_c1)
        self.sw2 = self._swatch(row, "Color B (respiración)", self.led_c2)
        quick = ctk.CTkFrame(colors.content, fg_color="transparent")
        quick.pack(fill="x", pady=(14, 0))
        ctk.CTkLabel(quick, text="Rápidos", font=font(12), text_color=C.muted).pack(side="left", padx=(0, 10))
        for hx in PRESET_COLORS:
            ctk.CTkButton(quick, text="", width=32, height=32, corner_radius=8, fg_color=hx, hover_color=hx,
                          border_width=1, border_color=C.border,
                          command=lambda c=hx: (self.led_c1.set(c), self._paint_swatches())).pack(side="left", padx=3)

        params = Card(page, "Parámetros")
        params.pack(fill="x", padx=PAD, pady=(0, GAP))
        LabeledSlider(params.content, "Brillo", "", self.led_bright, 0, 100, 1, " %", C.warn).pack(fill="x", pady=(0, 16))
        LabeledSlider(params.content, "Ciclo de respiración", "", self.led_period, 1000, 10000, 100, " ms",
                      C.accent2).pack(fill="x")
        if effects:
            LabeledSlider(params.content, "Velocidad de ciclo / estrobo", "Valor crudo del controlador (1–100)",
                          self.led_speed, 1, 100, 1, "", C.violet).pack(fill="x", pady=(16, 0))
            checkbox(params.content, "Invertir dirección", self.led_reverse).pack(anchor="w", pady=(12, 0))

        n = self.led.num_zones if self.led else 0
        if n > 1:
            zones = Card(page, "Zonas")
            zones.pack(fill="x", padx=PAD, pady=(0, GAP))
            zrow = ctk.CTkFrame(zones.content, fg_color="transparent")
            zrow.pack(fill="x")
            for i in range(n):
                v = tk.IntVar(value=1)
                self.led_zone_vars.append(v)
                checkbox(zrow, f"Zona {i}", v).pack(side="left", padx=(0, 18))

        bar = ctk.CTkFrame(page, fg_color="transparent")
        bar.pack(fill="x", padx=PAD, pady=(0, PAD))
        button(bar, "Aplicar LED", self.apply_led, "primary", width=150).pack(side="left")
        button(bar, "Restaurar original", self.led_default, "secondary", width=180).pack(side="left", padx=10)
        self._paint_swatches()

    def _swatch(self, parent, title: str, var: tk.StringVar) -> ctk.CTkButton:
        box = ctk.CTkFrame(parent, fg_color="transparent")
        box.pack(side="left", padx=(0, 28))
        ctk.CTkLabel(box, text=title, font=font(12), text_color=C.muted, anchor="w").pack(fill="x")
        b = ctk.CTkButton(box, text="", width=190, height=56, corner_radius=RADIUS_SM, border_width=1,
                          border_color=C.border, font=font(14, "bold"), command=lambda: self._pick(var))
        b.pack(pady=(4, 0))
        b.var = var
        return b

    def _paint_swatches(self) -> None:
        for b in (self.sw1, self.sw2):
            c = b.var.get()
            b.configure(fg_color=c, hover_color=c, text=c.upper(), text_color=readable_on(c))

    def _pick(self, var: tk.StringVar) -> None:
        chosen = pick_color(self, var.get())
        if chosen:
            var.set(chosen)
            self._paint_swatches()

    def get_led_state(self) -> dict:
        zones = [i for i, v in enumerate(self.led_zone_vars) if v.get()] if self.led_zone_vars else None
        return dict(mode=self.led_mode.get(), color=self.led_c1.get(), color2=self.led_c2.get(),
                    brightness=self.led_bright.get(), period=self.led_period.get(), zones=zones,
                    speed=self.led_speed.get(), reverse=bool(self.led_reverse.get()))

    def set_led_state(self, st: dict) -> None:
        mode = st.get("mode", "keep")
        self.led_mode.set(mode)
        if hasattr(self, "led_label"):
            self.led_label.set(LED_LABELS.get(mode, LED_LABELS["keep"]))
        self.led_c1.set(st.get("color", "#ff2d55"))
        self.led_c2.set(st.get("color2", "#000000"))
        self.led_bright.set(int(st.get("brightness", 100)))
        self.led_period.set(int(st.get("period", 3000)))
        self.led_speed.set(int(st.get("speed", 50)))
        self.led_reverse.set(int(bool(st.get("reverse", False))))
        if hasattr(self, "sw1"):
            self._paint_swatches()

    def apply_led(self) -> None:
        if not self.led:
            self.alert("LED", "El control de LED no está disponible:\n" + (self.led_error or ""))
            return
        st = self.get_led_state()
        if st["mode"] == "keep":
            self.notify("Elige un efecto distinto de «No cambiar» para aplicar.", "warn")
            return
        if st["zones"] == []:
            self.notify("Selecciona al menos una zona.", "warn")
            return
        try:
            apply_led_state(self.led, st)
            self.notify("LED aplicado.", "ok")
        except LedError as e:
            self.alert("LED", str(e))

    def led_default(self) -> None:
        if not self.led:
            return self.apply_led()
        try:
            self.led.restore_default()
            self.notify("LED restaurado a los valores originales.", "ok")
        except LedError as e:
            self.alert("LED", str(e))

    # ------------------------------------------------------------------------
    #  Página: Perfiles
    # ------------------------------------------------------------------------
    def _page_profiles(self, page) -> None:
        self._header(page, "Perfiles", "Guarda tus ajustes de overclock, ventiladores y LED y cárgalos con un clic.")
        row = ctk.CTkFrame(page, fg_color="transparent")
        row.pack(fill="x", padx=PAD, pady=(0, PAD))
        row.columnconfigure(0, weight=1)
        row.columnconfigure(1, weight=1)

        left = Card(row, "Guardados")
        left.grid(row=0, column=0, sticky="nsew", padx=(0, GAP))
        self.profile_list = ctk.CTkScrollableFrame(left.content, fg_color=C.surface2, corner_radius=RADIUS_SM, height=260,
                                                   scrollbar_button_color=C.track)
        self.profile_list.pack(fill="both", expand=True)

        right = Card(row, "Detalles")
        right.grid(row=0, column=1, sticky="nsew")
        self.profile_info = ctk.CTkLabel(right.content, text="Selecciona un perfil de la lista.", font=font(13),
                                         text_color=C.muted, justify="left", anchor="w", wraplength=330)
        self.profile_info.pack(fill="x", pady=(0, 16))
        ctk.CTkLabel(right.content, text="Nombre", font=font(12), text_color=C.muted, anchor="w").pack(fill="x")
        entry(right.content, self.pname, 280, "Ej. Gaming", "left").pack(anchor="w", pady=(4, 14))
        button(right.content, "Guardar ajustes actuales", self.save_profile, "primary").pack(fill="x", pady=3)
        button(right.content, "Cargar y aplicar", self.load_profile, "secondary").pack(fill="x", pady=3)
        button(right.content, "Eliminar", self.delete_profile, "danger").pack(fill="x", pady=3)
        self.refresh_profiles()

    def describe_profile(self, p: dict) -> str:
        led = (p.get("led") or {}).get("mode", "keep")
        fan = {"auto": "automático", "manual": f"fijo {p.get('fan_manual', 50)} %", "curve": "curva"}.get(
            p.get("fan_mode", "auto"), "automático")
        lock = f" · techo {p['lock_mhz']} MHz" if p.get("lock_on") else ""
        return (f"Potencia {int(p['power_w'])} W\nNúcleo {p['core_off']:+d} MHz · Memoria {p['mem_off']:+d} MHz{lock}\n"
                f"Ventiladores: {fan}\nLED: {LED_LABELS.get(led, led)}")

    def refresh_profiles(self) -> None:
        for w in self.profile_list.winfo_children():
            w.destroy()
        names = self.store.names()
        if not names:
            ctk.CTkLabel(self.profile_list, text="Aún no hay perfiles.", font=font(12),
                         text_color=C.muted).pack(pady=20)
        for n in names:
            active = n == self.sel_profile.get()
            ctk.CTkButton(self.profile_list, text=n, anchor="w", height=38, corner_radius=8,
                          fg_color=mix(C.accent, C.surface2, 0.7) if active else "transparent",
                          hover_color=C.track, text_color=C.text, font=font(13, "bold"),
                          command=lambda n=n: self.select_profile(n)).pack(fill="x", pady=2, padx=4)
        if hasattr(self, "startup_menu"):
            self.startup_menu.configure(values=["Ninguno"] + names)

    def select_profile(self, name: str) -> None:
        self.sel_profile.set(name)
        self.pname.set(name)
        self.profile_info.configure(text=self.describe_profile(self.store.data[name]), text_color=C.text)
        self.refresh_profiles()

    def save_profile(self) -> None:
        name = self.pname.get().strip() or self.sel_profile.get()
        if not name:
            self.notify("Escribe un nombre para el perfil.", "warn")
            return
        self.store.data[name] = self.get_state()
        self.store.save()
        self.select_profile(name)
        self.notify(f"Perfil «{name}» guardado.", "ok")

    def load_profile(self) -> None:
        name = self.sel_profile.get()
        if not name or name not in self.store.data:
            self.notify("Selecciona un perfil de la lista.", "warn")
            return
        self.set_state(self.store.data[name])
        self.apply_oc(silent=True)
        self.apply_fan(silent=True)
        if self.led and self.led_mode.get() != "keep":
            self.apply_led()
        self.notify(f"Perfil «{name}» aplicado.", "ok")

    def delete_profile(self) -> None:
        name = self.sel_profile.get()
        if name and ask(self, "Eliminar perfil", f"¿Eliminar el perfil «{name}»? No se puede deshacer.",
                        ok="Eliminar", danger=True):
            del self.store.data[name]
            self.store.save()
            if self.settings.startup_profile == name:
                self.settings.startup_profile = None
                self.settings.save()
                self.startup_choice.set("Ninguno")
            self.sel_profile.set("")
            self.profile_info.configure(text="Selecciona un perfil de la lista.", text_color=C.muted)
            self.refresh_profiles()

    # ------------------------------------------------------------------------
    #  Página: Ajustes (inicio con Windows)
    # ------------------------------------------------------------------------
    def _page_settings(self, page) -> None:
        self._header(page, "Ajustes", "Inicio con el sistema e información de la aplicación.")
        sup = autostart.is_supported()

        start = Card(page, "Inicio con Windows",
                     "GPU PY crea una tarea del Programador de tareas con privilegios elevados, "
                     "así Windows no pide permiso de administrador cada vez.")
        start.pack(fill="x", padx=PAD, pady=(0, GAP))
        self.autostart_var = tk.IntVar(value=int(autostart.is_enabled()) if sup else 0)
        row = SwitchRow(start.content, "Iniciar GPU PY al iniciar sesión",
                        "Se abre unos segundos después de entrar a Windows." if sup else
                        "Solo disponible en Windows.", self.autostart_var, self._toggle_autostart)
        row.pack(fill="x", pady=(0, 16))
        if not sup:
            row.switch.configure(state="disabled")
        self.minimized_var = tk.IntVar(value=int(self.settings.start_minimized))
        SwitchRow(start.content, "Iniciar minimizado",
                  "Arranca en la barra de tareas o la bandeja, sin abrir la ventana.", self.minimized_var,
                  self._save_settings).pack(fill="x", pady=(0, 16))
        self.tray_var = tk.IntVar(value=int(self.settings.tray_enabled))
        SwitchRow(start.content, "Bandeja del sistema (tray)",
                  "Minimizar y la X llevan a la bandeja; clic derecho para salir.", self.tray_var,
                  self._save_settings).pack(fill="x", pady=(0, 16))

        prof = ctk.CTkFrame(start.content, fg_color="transparent")
        prof.pack(fill="x")
        texts = ctk.CTkFrame(prof, fg_color="transparent")
        texts.pack(side="left", fill="x", expand=True)
        ctk.CTkLabel(texts, text="Perfil al iniciar", font=font(14, "bold"), text_color=C.text, anchor="w").pack(fill="x")
        ctk.CTkLabel(texts, text="Aplica automáticamente overclock, ventiladores y LED.", font=font(12),
                     text_color=C.muted, anchor="w").pack(fill="x")
        self.startup_choice = tk.StringVar(value=self.settings.startup_profile or "Ninguno")
        self.startup_menu = option_menu(prof, ["Ninguno"] + self.store.names(), self.startup_choice,
                                        lambda _v: self._save_settings(), 190)
        self.startup_menu.pack(side="right")
        button(start.content, "Aplicar perfil de inicio ahora", lambda: self.run_startup(manual=True),
               "secondary").pack(anchor="w", pady=(16, 0))

        info = Card(page, "Información")
        info.pack(fill="x", padx=PAD, pady=(0, PAD))
        rows = [("Versión", f"{APP_NAME} {VERSION}"), ("GPU", self.be.name), ("Driver", self.be.driver),
                ("Permisos", "Administrador" if (is_admin() or self.be.demo) else "Usuario estándar"),
                ("Datos", str(data_dir()))]
        for k, v in rows:
            r = ctk.CTkFrame(info.content, fg_color="transparent")
            r.pack(fill="x", pady=3)
            ctk.CTkLabel(r, text=k, font=font(12), text_color=C.muted, width=90, anchor="w").pack(side="left")
            ctk.CTkLabel(r, text=v, font=font(13), text_color=C.text, anchor="w", wraplength=560,
                         justify="left").pack(side="left")
        button(info.content, "Abrir carpeta de datos", self.open_data_dir, "secondary").pack(anchor="w", pady=(14, 0))

    def _save_settings(self) -> None:
        self.settings.start_minimized = bool(self.minimized_var.get())
        self.settings.tray_enabled = bool(self.tray_var.get())
        choice = self.startup_choice.get()
        self.settings.startup_profile = None if choice == "Ninguno" else choice
        self.settings.save()
        if not self.settings.tray_enabled and self._in_tray:
            self._show_from_tray()                                     # no quedarse oculto sin icono
        self.notify("Ajustes guardados.", "ok")

    def _toggle_autostart(self) -> None:
        want = bool(self.autostart_var.get())
        try:
            autostart.enable() if want else autostart.disable()
            self.notify("GPU PY se iniciará con Windows." if want else "Inicio con Windows desactivado.", "ok")
        except autostart.AutostartError as e:
            self.autostart_var.set(0 if want else 1)
            self.alert("Inicio con Windows", str(e))

    def open_data_dir(self) -> None:
        path = str(data_dir())
        try:
            if os.name == "nt":
                os.startfile(path)                                       # type: ignore[attr-defined]
            else:
                subprocess.Popen(["xdg-open", path])
        except OSError:
            self.notify(path, "info")

    # ------------------------------------------------------------------------
    #  Estado <-> widgets
    # ------------------------------------------------------------------------
    def get_state(self) -> dict:
        return dict(power_w=self.power.get(), core_off=self.core.get(), mem_off=self.mem.get(),
                    lock_on=bool(self.lock_on.get()), lock_mhz=self.lock_mhz.get(),
                    fan_mode=self.fan_mode.get(), fan_manual=self.fan_manual.get(),
                    curve=[list(p) for p in self.get_curve()], led=self.get_led_state())

    def set_state(self, p: dict) -> None:
        self.power.set(int(p["power_w"]))
        self.core.set(int(p["core_off"]))
        self.mem.set(int(p["mem_off"]))
        self.lock_on.set(int(bool(p.get("lock_on"))))
        self.lock_mhz.set(int(p.get("lock_mhz", self.L.max_core_clock)))
        mode = p.get("fan_mode", "auto")
        self.fan_mode.set(mode)
        if hasattr(self, "fan_label"):
            self.fan_label.set(FAN_LABELS.get(mode, FAN_LABELS["auto"]))
        self.fan_manual.set(int(p.get("fan_manual", 50)))
        for (tv, pv), (t, pc) in zip(self.curve_vars, p.get("curve", DEFAULT_CURVE)):
            tv.set(str(t))
            pv.set(str(pc))
        self.set_led_state(p.get("led") or default_led_state())

    def load_current_values(self) -> None:
        try:
            self.core.set(self.be.get_core_offset())
            self.mem.set(self.be.get_mem_offset())
        except Exception:
            pass

    # ------------------------------------------------------------------------
    #  Acciones
    # ------------------------------------------------------------------------
    def confirm_risk(self) -> bool:
        if self.risk_ack or self.be.demo:
            return True
        self.risk_ack = ask(
            self, "Antes de continuar",
            "Modificar frecuencias, potencia o ventiladores puede causar inestabilidad (pantallazos, cierres "
            "de juegos) y, si se abusa, reducir la vida útil de la GPU.\n\nHazlo de forma gradual y vigila las "
            "temperaturas. Reiniciar el PC revierte estos cambios.", ok="Entiendo, continuar")
        return self.risk_ack

    def apply_oc(self, silent: bool = False) -> None:
        if not self.confirm_risk():
            return
        p = self.get_state()
        p["fan_mode"], p["led"] = "keep", None           # esta página no toca ventiladores ni LED
        errs = apply_profile_to_backend(self.be, p)
        if errs:
            self.alert("Algunos ajustes no se aplicaron", "\n\n".join(errs))
        elif not silent:
            self.notify("Overclock aplicado correctamente.", "ok")

    def apply_fan(self, silent: bool = False) -> None:
        if not self.confirm_risk():
            return
        self.last_fan = None
        try:
            match self.fan_mode.get():
                case "auto":
                    self.be.fan_auto()
                    msg = "Ventiladores en automático."
                case "manual":
                    self.be.set_fan(self.fan_manual.get())
                    msg = f"Ventiladores fijos al {self.fan_manual.get()} %."
                case _:
                    msg = "Curva activa mientras GPU PY esté abierto."
            if not silent:
                self.notify(msg, "ok")
        except BackendError as e:
            self._fan_failed(str(e))

    def _fan_failed(self, msg: str) -> None:
        self.fan_mode.set("auto")
        self.fan_label.set(FAN_LABELS["auto"])
        self.fan_supported = False
        self.alert("Ventiladores", f"{msg}\n\nEn muchas GeForce con Windows el driver no permite este control.")

    def reset_all(self) -> None:
        errs = self.be.reset_all()
        self.set_state(default_profile(self.be))
        self.last_fan = None
        self.notify("Todo restablecido a valores de fábrica." if not errs else
                    "Restablecido (algunos ajustes no son compatibles).", "ok" if not errs else "warn")

    # ------------------------------------------------------------------------
    #  Arranque con Windows
    # ------------------------------------------------------------------------
    def run_startup(self, attempt: int = 0, manual: bool = False) -> None:
        name = self.settings.startup_profile
        if not name or name not in self.store.data:
            if manual:
                self.notify("Elige primero un perfil de inicio.", "warn")
            return
        prof = self.store.data[name]
        self.set_state(prof)
        errs = apply_profile_to_backend(self.be, prof, self.led)
        if errs and attempt < 3 and not manual:                          # el driver puede tardar en estar listo
            self.after(8000, lambda: self.run_startup(attempt + 1))
            return
        self.notify(f"Perfil «{name}» aplicado." if not errs else "Perfil aplicado con errores: " + errs[0],
                    "ok" if not errs else "warn")

    # ------------------------------------------------------------------------
    #  Bandeja del sistema (tray)
    # ------------------------------------------------------------------------
    def _on_unmap(self, _event=None) -> None:
        if self.settings.tray_enabled and not self._in_tray and self.state() == "iconic":
            self._hide_to_tray()

    def _hide_to_tray(self, silent: bool = False) -> None:
        if not self.settings.tray_enabled or not self._tray.start():
            self.iconify()                                             # sin tray: minimizar clásico
            return
        self._in_tray = True
        self.withdraw()
        if not silent:
            self._tray.notify("GPU PY sigue en la bandeja: clic para mostrar, clic derecho para salir.")

    def _show_from_tray(self) -> None:
        self._tray.stop()
        self._in_tray = False
        self.deiconify()
        self.lift()
        self.focus_force()

    def _apply_from_tray(self) -> None:
        self.run_startup(manual=True)

    def _quit_from_tray(self) -> None:
        self._tray.stop()
        self._in_tray = False
        self._exit_app()

    # ------------------------------------------------------------------------
    #  Telemetría
    # ------------------------------------------------------------------------
    def poll(self) -> None:
        hidden = self._in_tray
        try:
            t = self.be.read()
        except Exception as e:
            self.notify(f"Error leyendo la GPU: {e}", "error")
            self._poll_job = self.after(5000 if hidden else 2000, self.poll)
            return
        self.hist["temp"].append(t.temp)
        self.hist["util"].append(t.util_gpu)
        self.hist["power"].append(100 * t.power_w / max(1, self.L.power_default_w))

        if not hidden:                                                 # ventana visible: todo el refresco
            self.g_temp.set(t.temp)
            self.g_core.set(t.core_clock)
            self.g_mem.set(t.mem_clock)
            self.g_fan.set(t.fan_pct)

            limit = self.power.get() or self.L.power_default_w
            if self.current_page == "monitor":
                self.chart.redraw()
            self.stat["power"].set(f"{t.power_w:.0f} W  /  {limit} W")
            self.stat["util"].set(f"{t.util_gpu} %")
            self.stat["vram"].set(f"{t.vram_used_mb} / {t.vram_total_mb} MB")
            self.bar_power.set(min(1, t.power_w / max(1, limit)))
            self.bar_util.set(t.util_gpu / 100)
            self.bar_vram.set(t.vram_used_mb / max(1, t.vram_total_mb))
            self._set_state_pill(t.throttle or "Sin limitaciones", C.warn if t.throttle else C.ok)

        if self.fan_mode.get() == "curve" and self.fan_supported:      # la curva sigue activa en el tray
            target = 100 if t.temp >= SAFETY_TEMP else interpolate(self.get_curve(), t.temp)
            if self.last_fan is None or abs(target - self.last_fan) >= 3 or target == 100:
                try:
                    self.be.set_fan(target)
                    self.last_fan = target
                except BackendError as e:
                    self._fan_failed(str(e))
        if not hidden and self.current_page == "fan":
            self._refresh_curve(t.temp)
        self._poll_job = self.after(5000 if hidden else 1000, self.poll)   # en el tray: 5 s, bajo consumo

    def on_close(self) -> None:
        if self.settings.tray_enabled:
            self._hide_to_tray()                                       # la X minimiza a la bandeja
        else:
            self._exit_app()

    def _exit_app(self) -> None:
        for job in (self._poll_job, self._startup_job):                  # sin tareas colgando al cerrar
            if job:
                try:
                    self.after_cancel(job)
                except Exception:
                    pass
        try:
            if self.fan_mode.get() != "auto":
                self.be.fan_auto()                                       # nunca dejes el ventilador en manual
        except Exception:
            pass
        self.be.close()
        self.destroy()
