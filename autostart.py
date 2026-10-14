"""
autostart.py - Iniciar GPU PY con Windows mediante el Programador de tareas.

Se usa una tarea al iniciar sesión con "máximos privilegios" para que Windows NO pida permiso de
administrador cada vez (hace falta para aplicar overclock y LED). La tarea se define por XML porque
las tareas creadas con parámetros de schtasks se detienen solas a las 72 h y no arrancan con batería.
"""
from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from collections.abc import Callable
from pathlib import Path
from xml.sax.saxutils import escape

from core import APP_NAME

TASK_NAME = APP_NAME
Runner = Callable[..., subprocess.CompletedProcess]


class AutostartError(Exception):
    pass


def is_supported() -> bool:
    return os.name == "nt"


def launch_command(exe_override: str | Path | None = None) -> tuple[str, str, str]:
    """(ejecutable, argumentos, carpeta de trabajo) con los que arranca la tarea."""
    if exe_override is not None:                                     # p. ej. copia recién instalada
        exe = Path(exe_override)
        return str(exe), "--startup", str(exe.parent)
    if getattr(sys, "frozen", False):                                   # .exe de PyInstaller
        exe = Path(sys.executable)
        return str(exe), "--startup", str(exe.parent)
    script = Path(__file__).resolve().with_name("gpupy.py")             # no depende de la carpeta actual
    py = Path(sys.executable)
    pyw = py.with_name("pythonw.exe")                                    # sin ventana de consola
    return str(pyw if pyw.exists() else py), f'"{script}" --startup', str(script.parent)


def build_task_xml(exe: str, args: str, workdir: str, user: str, delay_s: int = 20) -> str:
    e = escape
    return f"""<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Inicia {e(APP_NAME)} al iniciar sesión (overclock, LED y ventiladores).</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{e(user)}</UserId>
      <Delay>PT{int(delay_s)}S</Delay>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{e(user)}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{e(exe)}</Command>
      <Arguments>{e(args)}</Arguments>
      <WorkingDirectory>{e(workdir)}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"""


def _run(cmd: list[str], runner: Runner) -> subprocess.CompletedProcess:
    kw = {"capture_output": True, "text": True}
    if os.name == "nt":
        kw["creationflags"] = 0x08000000                                  # CREATE_NO_WINDOW
    return runner(cmd, **kw)


def current_user() -> str:
    dom, usr = os.environ.get("USERDOMAIN", ""), os.environ.get("USERNAME", "")
    return f"{dom}\\{usr}" if dom else usr


def is_enabled(runner: Runner = subprocess.run) -> bool:
    if not is_supported():
        return False
    try:
        return _run(["schtasks", "/Query", "/TN", TASK_NAME], runner).returncode == 0
    except OSError:
        return False


def enable(runner: Runner = subprocess.run, delay_s: int = 20,
           exe_override: str | Path | None = None) -> None:
    if not is_supported():
        raise AutostartError("El inicio con el sistema solo está disponible en Windows.")
    exe, args, workdir = launch_command(exe_override)
    xml = build_task_xml(exe, args, workdir, current_user(), delay_s)
    with tempfile.NamedTemporaryFile("w", suffix=".xml", delete=False, encoding="utf-16") as f:
        f.write(xml)
        path = f.name
    try:
        r = _run(["schtasks", "/Create", "/TN", TASK_NAME, "/XML", path, "/F"], runner)
    except OSError as e:
        raise AutostartError(f"No se pudo ejecutar schtasks: {e}")
    finally:
        try:
            os.unlink(path)
        except OSError:
            pass
    if r.returncode != 0:
        msg = (r.stderr or r.stdout or "").strip()
        if "denied" in msg.lower() or "denegado" in msg.lower():
            msg = "Acceso denegado. Abre GPU PY como Administrador para activar el inicio con Windows."
        raise AutostartError(msg or "schtasks devolvió un error.")


def disable(runner: Runner = subprocess.run) -> None:
    if not is_supported():
        return
    r = _run(["schtasks", "/Delete", "/TN", TASK_NAME, "/F"], runner)
    if r.returncode != 0 and is_enabled(runner):
        msg = (r.stderr or r.stdout or "").strip()
        if "denied" in msg.lower() or "denegado" in msg.lower():
            msg = "Acceso denegado. Abre GPU PY como Administrador."
        raise AutostartError(msg or "No se pudo eliminar la tarea.")
