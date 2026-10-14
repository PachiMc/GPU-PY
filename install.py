"""
install.py - Auto-instalación de GPU PY en una carpeta permanente.

El .exe es portable, pero la tarea de inicio con Windows apunta a la ruta
exacta del ejecutable: si se ejecuta desde Descargas y esa carpeta se vacía,
el inicio automático se rompe. Este módulo copia el exe a una ubicación fija
(%LOCALAPPDATA%\\GPU PY) para que eso no pase.
"""
from __future__ import annotations

import os
import shutil
import sys
import tempfile
from pathlib import Path

INSTALLED_NAME = "GPU-PY.exe"


class InstallError(Exception):
    pass


def is_frozen() -> bool:
    """True si corremos como .exe de PyInstaller."""
    return getattr(sys, "frozen", False)


def current_exe() -> Path:
    return Path(sys.executable).resolve()


def install_dir() -> Path:
    base = Path(os.environ.get("LOCALAPPDATA") or Path.home())
    return base / "GPU PY"


def installed_exe() -> Path:
    return install_dir() / INSTALLED_NAME


def is_installed() -> bool:
    try:
        return is_frozen() and current_exe() == installed_exe().resolve()
    except OSError:
        return False


def should_suggest_install() -> bool:
    """True si el exe está en un lugar típicamente temporal (Descargas, Temp...)."""
    if not is_frozen() or is_installed():
        return False
    try:
        parent = current_exe().parent
    except OSError:
        return False
    names = {parent.name.lower(), parent.parent.name.lower()}
    if names & {"descargas", "downloads", "temp", "tmp", "escritorio", "desktop"}:
        return True
    try:
        if str(parent).lower().startswith(tempfile.gettempdir().lower()):
            return True
    except Exception:
        pass
    return False


def install() -> Path:
    """Copia el exe en ejecución a la carpeta permanente. Devuelve la ruta destino."""
    if not is_frozen():
        raise InstallError("La instalación solo aplica al .exe (estás ejecutando el código fuente).")
    dst = installed_exe()
    dst.parent.mkdir(parents=True, exist_ok=True)
    if is_installed():
        return dst
    try:
        shutil.copy2(sys.executable, dst)
    except OSError as e:
        raise InstallError(f"No se pudo copiar a {dst}: {e}")
    return dst
