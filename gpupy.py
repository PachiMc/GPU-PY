"""
GPU PY - Monitor, overclock, ventiladores y LED para GPUs NVIDIA (RTX 3070 PNY XLR8 y similares).

Uso:
    python gpupy.py                  abre la interfaz (ejecuta como Administrador para aplicar cambios)
    python gpupy.py --demo           GPU simulada, sin necesidad de tarjeta
    python gpupy.py --list           lista los perfiles guardados
    python gpupy.py --apply Gaming   aplica un perfil y sale
    python gpupy.py --reset          restablece todo a valores de fábrica
    python gpupy.py --led "#ff0066"  color fijo del LED (también: off | default)
    python gpupy.py --led-info       controlador LED detectado
    python gpupy.py --i2c-scan       diagnóstico de solo lectura del LED por I2C (guarda led_report.txt)
    python gpupy.py --oc-info        qué overclock permite tu GPU (solo lectura)
    python gpupy.py --autostart on|off|status   iniciar con Windows
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

import autostart
import install
from core import (APP_NAME, ProfileStore, apply_led_state, apply_profile_to_backend, default_led_state,
                  i2c_scan_text, make_led, oc_info_text)
from gpu_backend import BackendError, DemoBackend, NvmlBackend
from led_nvapi import LedError


def fatal(msg: str, gui: bool) -> None:
    if gui:                                                   # ventana mínima con el estilo de la app
        import customtkinter as ctk

        import theme
        from theme import C, font
        from widgets import button
        theme.setup()
        root = ctk.CTk()
        root.title(APP_NAME)
        root.configure(fg_color=C.surface)
        ctk.CTkLabel(root, text=APP_NAME, font=font(20, "bold"), text_color=C.text).pack(padx=28, pady=(24, 4), anchor="w")
        ctk.CTkLabel(root, text=msg, font=font(13), text_color=C.muted, wraplength=420, justify="left").pack(
            padx=28, pady=(0, 18), anchor="w")
        button(root, "Cerrar", root.destroy, "primary", width=120).pack(padx=28, pady=(0, 24), anchor="e")
        root.mainloop()
    else:
        print("ERROR:", msg, file=sys.stderr)
    sys.exit(1)


def main(argv: list[str] | None = None) -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--demo", action="store_true", help="usar una GPU simulada")
    ap.add_argument("--startup", action="store_true", help="(interno) arranque automático con Windows")
    ap.add_argument("--apply", metavar="PERFIL", help="aplicar un perfil y salir")
    ap.add_argument("--reset", action="store_true", help="restablecer valores de fábrica y salir")
    ap.add_argument("--list", action="store_true", help="listar perfiles y salir")
    ap.add_argument("--led", metavar="COLOR", help="LED: #RRGGBB, off o default")
    ap.add_argument("--led-info", action="store_true", help="mostrar el controlador LED detectado")
    ap.add_argument("--i2c-scan", action="store_true", help="diagnóstico de solo lectura del LED por I2C")
    ap.add_argument("--oc-info", action="store_true", help="mostrar qué overclock permite tu GPU")
    ap.add_argument("--autostart", choices=("on", "off", "status"), help="gestionar el inicio con Windows")
    ap.add_argument("--install", action="store_true", help="copiar el .exe a una carpeta permanente y salir")
    a = ap.parse_args(argv)

    if a.list:
        print("\n".join(ProfileStore().names()) or "(sin perfiles)")
        return

    if a.autostart:
        try:
            match a.autostart:
                case "on":
                    autostart.enable()
                    print("Inicio con Windows: ACTIVADO")
                case "off":
                    autostart.disable()
                    print("Inicio con Windows: DESACTIVADO")
                case _:
                    print("Inicio con Windows:", "ACTIVADO" if autostart.is_enabled() else "desactivado")
        except autostart.AutostartError as e:
            fatal(str(e), gui=False)
        return

    if a.install:
        try:
            dst = install.install()
        except install.InstallError as e:
            fatal(str(e), gui=False)
        print(f"Instalado en {dst}")
        return

    if a.i2c_scan:
        try:
            text = i2c_scan_text(a.demo)
        except LedError as e:
            fatal(str(e), gui=False)
        print(text)
        try:
            path = Path(__file__).resolve().with_name("led_report.txt")
            path.write_text(text + "\n", encoding="utf-8")
            print(f"\n(Informe guardado en {path})")
        except OSError:
            pass
        return

    led, led_err = make_led(a.demo)

    # ---- comandos de LED independientes del resto ------------------------------
    if a.led_info or a.led:
        if led is None:
            fatal(led_err, gui=False)
        if a.led_info:
            print(led.describe())
        if a.led:
            try:
                key = a.led.strip().lower()
                st = default_led_state()
                st["mode"] = key if key in ("off", "default") else "static"
                if st["mode"] == "static":
                    st["color"] = a.led
                apply_led_state(led, st)
                print("LED OK")
            except LedError as e:
                fatal(str(e), gui=False)
        if not (a.apply or a.reset):
            return

    cli = bool(a.apply or a.reset or a.oc_info)
    try:
        be = DemoBackend() if a.demo else NvmlBackend()
    except BackendError as e:
        fatal(str(e), gui=not cli)

    if a.oc_info:
        print(oc_info_text(be))
        be.close()
        return

    if cli:
        if a.reset:
            errs = be.reset_all()
        else:
            store = ProfileStore()
            if a.apply not in store.data:
                fatal(f"No existe el perfil «{a.apply}». Perfiles: {', '.join(store.names()) or '(ninguno)'}", False)
            prof = store.data[a.apply]
            errs = apply_profile_to_backend(be, prof, led)
            if prof.get("fan_mode") == "curve":
                print("Nota: la curva de ventiladores solo funciona con la interfaz abierta.")
        print("\n".join(errs) if errs else "OK")
        be.close()
        sys.exit(1 if errs else 0)

    from ui import App                                       # import tardío: la CLI no necesita la GUI
    App(be, led, led_err, startup=a.startup).mainloop()


if __name__ == "__main__":
    main()
