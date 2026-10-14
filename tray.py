"""
tray.py - Icono de GPU PY en la bandeja del sistema (tray) con pystray.

La ventana vive en el hilo principal (tkinter). El icono corre en un hilo
aparte; todo lo que toque la interfaz se agenda con `root.after(0, ...)`
desde los callbacks del menú.
"""
from __future__ import annotations

import threading
from pathlib import Path
from typing import Callable

from core import APP_NAME, resource_path


def _load_image():
    """PIL.Image para el icono (o None si no hay Pillow/recursos)."""
    try:
        from PIL import Image
        png = Path(resource_path("assets/gpupy.png"))
        if png.exists():
            return Image.open(png).convert("RGBA").resize((64, 64))
    except Exception:
        pass
    try:
        import icons
        return icons.app_icon(64)
    except Exception:
        return None


class TrayController:
    """Muestra/oculta el icono de la bandeja. No toca tkinter directamente."""

    def __init__(self, root, on_show: Callable[[], None],
                 on_apply_profile: Callable[[], None], on_quit: Callable[[], None]):
        self._root = root
        self._on_show_cb, self._on_apply_cb, self._on_quit_cb = on_show, on_apply_profile, on_quit
        self._icon = None
        self._thread: threading.Thread | None = None

    @property
    def running(self) -> bool:
        return self._icon is not None

    def start(self) -> bool:
        """Arranca el icono. Devuelve False si pystray no está disponible."""
        if self._icon is not None:
            return True
        try:
            import pystray
        except ImportError:
            return False
        image = _load_image()
        if image is None:
            return False
        menu = pystray.Menu(
            pystray.MenuItem("Mostrar GPU PY", lambda _i, _it: self._root.after(0, self._on_show_cb),
                             default=True),
            pystray.MenuItem("Aplicar perfil de inicio",
                             lambda _i, _it: self._root.after(0, self._on_apply_cb)),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem("Salir", lambda _i, _it: self._root.after(0, self._on_quit_cb)),
        )
        self._icon = pystray.Icon("gpupy", image, APP_NAME, menu)
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()
        return True

    def _run(self) -> None:
        try:
            self._icon.run()
        except Exception:
            pass

    def stop(self) -> None:
        icon, thread = self._icon, self._thread
        self._icon, self._thread = None, None
        if icon is not None:
            try:
                icon.stop()
            except Exception:
                pass
        if thread is not None and thread.is_alive():
            thread.join(timeout=3)

    def notify(self, message: str, title: str = APP_NAME) -> None:
        if self._icon is not None:
            try:
                self._icon.notify(message, title)
            except Exception:
                pass
