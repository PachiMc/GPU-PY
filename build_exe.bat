@echo off
REM Genera "GPU PY.exe" (un solo archivo, con icono, pide Administrador al abrir).
cd /d "%~dp0"
python -m pip install -r requirements.txt pyinstaller
python -m PyInstaller --noconfirm --onefile --windowed --uac-admin --name "GPU PY" ^
  --icon assets\gpupy.ico --add-data "assets;assets" --collect-all customtkinter gpupy.py
echo.
echo Listo: dist\GPU PY.exe
pause
