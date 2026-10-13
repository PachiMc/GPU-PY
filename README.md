# GPU PY

Monitor, overclock, ventiladores y LED/RGB para tu **RTX 3070 PNY XLR8** (y otras GeForce), como alternativa
en Python a PNY VelocityX. Interfaz moderna con CustomTkinter.

## Instalación (Windows)

1. Instala **Python 3.10 o superior** desde python.org (marca *Add python.exe to PATH*).
2. En esta carpeta: `pip install -r requirements.txt`
3. Prueba sin riesgo con una GPU simulada: `python gpupy.py --demo`
4. Uso real: doble clic en **run_as_admin.bat** (los cambios requieren Administrador).

Para un `.exe` con icono, ejecuta **build_exe.bat**; saldrá `dist\GPU PY.exe`.

## Iniciar con Windows

En **Ajustes** activa *Iniciar GPU PY al iniciar sesión* (hazlo con la app abierta como Administrador).

- Crea una tarea del Programador de tareas con **máximos privilegios**: Windows no pide permiso cada vez.
- Opcional: *Iniciar minimizado* y *Perfil al iniciar* (aplica overclock, ventiladores y LED automáticamente).
- Si el driver tarda en estar listo, GPU PY reintenta aplicar el perfil hasta 3 veces.
- Sin interfaz: `python gpupy.py --autostart on|off|status`.

## Funciones

| Función | Detalle |
|---|---|
| Monitor | temperatura, relojes, ventilador, potencia, uso, VRAM y motivo de throttling, con historial |
| Overclock | offsets de núcleo y memoria, límite de potencia, techo de reloj; por NVML o, si no está disponible, por **NVAPI** como VelocityX |
| Ventiladores | automático, fijo o curva personalizada con vista previa (a ≥ 88 °C fuerza 100 %) |
| LED / RGB | color fijo, respiración, apagado y, según tu controlador, ciclo y estrobo; por NVAPI o I2C como VelocityX |
| Perfiles | guarda y aplica todo con un clic; se pueden aplicar desde la consola |

## Línea de comandos

```
python gpupy.py --list                  perfiles guardados
python gpupy.py --apply Gaming          aplica un perfil y sale
python gpupy.py --reset                 valores de fábrica
python gpupy.py --led "#ff0066"         color fijo (también: off | default)
python gpupy.py --led-info              controlador LED detectado
python gpupy.py --i2c-scan              diagnóstico de solo lectura del LED (guarda led_report.txt)
python gpupy.py --oc-info               qué overclock permite tu GPU (solo lectura)
```

Los perfiles y ajustes se guardan en `%APPDATA%\GPU PY` (se migran solos los de la versión anterior).

## Seguridad y notas

- Los cambios de overclock y LED **no son permanentes**: se pierden al reiniciar el PC. Por eso existe el perfil de inicio.
- Sube los offsets poco a poco (núcleo de 15 en 15 MHz, memoria de 50 en 50) y prueba la estabilidad.
- El LED por I2C solo escribe los registros exactos que usa VelocityX (lista blanca) y nunca toca otros chips.
- Control de ventiladores: en muchas GeForce con Windows el driver puede rechazarlo; la app lo avisa y los deja en automático.
- Al cerrar GPU PY, el ventilador siempre vuelve a automático.
- La escala de velocidad de ciclo/estrobo es el valor crudo del controlador (1–100).

## Estructura

- `gpupy.py` – punto de entrada y línea de comandos
- `ui.py` – ventana principal y páginas · `widgets.py` – componentes · `theme.py` – sistema de diseño · `icons.py` – iconos
- `core.py` – perfiles, ajustes y lógica de aplicación · `autostart.py` – inicio con Windows
- `gpu_backend.py` (NVML) · `oc_nvapi.py` · `led_nvapi.py` · `led_i2c.py` – acceso al hardware
