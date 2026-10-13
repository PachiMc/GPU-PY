# GPU PY

Monitor, overclock, ventiladores y LED/RGB para GPUs NVIDIA (RTX 3070 PNY XLR8 y similares),
como alternativa en Python a PNY VelocityX. Interfaz moderna con CustomTkinter.

## 📦 Download

**GPU PY.exe** - Ejecutable listo para usar (un solo archivo, con icono, solicita Administración al abrir).
[Download GPU PY.exe](https://github.com/username/GPU-PY/releases/latest) *(reemplazar con enlace real)*

## 🚀 Installation (Windows)

1. Instala **Python 3.10 o superior** desde python.org (marca *Add python.exe to PATH*).
2. En esta carpeta: `pip install -r requirements.txt`
3. Prueba sin riesgo con una GPU simulada: `python gpupy.py --demo`
4. Uso real: doble clic en **run_as_admin.bat** (los cambios requieren Administrador).

Para compilar tu propio ejecutable, ejecuta **build_exe.bat**; esto generará `dist\GPU PY.exe`.

## 🔧 Functions

| Function | Detail |
|---|---|
| Monitor | temperature, clocks, fan, power, usage, VRAM and throttling reason, with history |
| Overclock | core and memory offsets, power limit, clock ceiling; via NVML or, if unavailable, via **NVAPI** like VelocityX |
| Fans | automatic, fixed or custom curve with preview (at ≥ 88 °C forces 100%) |
| LED / RGB | fixed color, breathing, off and, depending on your controller, strobe and stroboscopic; via NVAPI or I2C like VelocityX |
| Profiles | save and apply everything with one click; can be applied from the command line |

## ⌨️ Command Line

```
python gpupy.py --list                  List saved profiles
python gpupy.py --apply Gaming          Apply a profile and exit
python gpupy.py --reset                 Reset to factory defaults
python gpupy.py --led "#ff0066"         Fixed LED color (also: off | default)
python gpupy.py --led-info              Detect LED controller
python gpupy.py --i2c-scan              Read-only LED I2C diagnosis (saves led_report.txt)
python gpupy.py --oc-info               What overclock your GPU allows (read-only)
```

Profiles and settings are saved in `%APPDATA%\GPU PY` (previous version settings are auto-migrated).

## ⚠️ Security and Notes

- Overclock and LED changes **are not permanent**: they are lost on PC restart. This is why the startup profile exists.
- Increase offsets gradually (core by 15 MHz, memory by 50) and test for stability.
- LED via I2C only writes the exact registers used by VelocityX (whitelist) and never touches other chips.
- Fan control: on many GeForce with Windows the driver may reject it; the app warns and sets fans to automatic.
- When closing GPU PY, the fan always returns to automatic.
- Cycle/strobe speed scale is the raw controller value (1–100).

## 📁 Structure

- `gpupy.py` – entry point and CLI
- `ui.py` – main window and pages · `widgets.py` – components · `theme.py` – design system · `icons.py` – icons
- `core.py` – profiles, settings and application logic · `autostart.py` – Windows startup
- `gpu_backend.py` (NVML) · `oc_nvapi.py` · `led_nvapi.py` · `led_i2c.py` – hardware access

## 🛠 Build

```batch
python -m PyInstaller --noconfirm --onefile --windowed --uac-admin --name "GPU PY" ^
  --icon assets\gpupy.ico --add-data "assets;assets" --collect-all customtkinter gpupy.py
```
