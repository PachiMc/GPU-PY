@echo off
REM Abre GPU PY con permisos de Administrador (necesarios para aplicar cambios).
cd /d "%~dp0"
powershell -NoProfile -Command "Start-Process -FilePath pythonw -ArgumentList 'gpupy.py' -WorkingDirectory '%~dp0' -Verb RunAs"
