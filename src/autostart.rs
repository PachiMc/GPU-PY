//! Iniciar GPU PY con Windows mediante el Programador de tareas.
//!
//! Se usa una tarea al iniciar sesión con "máximos privilegios" para que Windows NO pida permiso de
//! administrador cada vez (hace falta para aplicar overclock y LED). La tarea se define por XML porque
//! las tareas creadas con parámetros de schtasks se detienen solas a las 72 h y no arrancan con batería.

use crate::core::APP_NAME;
use crate::error::{HwError, HwResult};
use std::path::{Path, PathBuf};

pub const TASK_NAME: &str = APP_NAME;

/// Resultado de ejecutar un comando externo.
#[derive(Debug, Clone, Default)]
pub struct CmdResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub type Runner<'a> = &'a dyn Fn(&[String]) -> std::io::Result<CmdResult>;

pub fn is_supported() -> bool {
    cfg!(windows)
}

/// (ejecutable, argumentos, carpeta de trabajo) con los que arranca la tarea.
pub fn launch_command(exe_override: Option<&Path>) -> (String, String, String) {
    let exe: PathBuf = exe_override.map(Path::to_path_buf).or_else(|| std::env::current_exe().ok()).unwrap_or_default();
    let dir = exe.parent().map(|p| p.display().to_string()).unwrap_or_default();
    (exe.display().to_string(), "--startup".to_string(), dir)
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn build_task_xml(exe: &str, args: &str, workdir: &str, user: &str, delay_s: u32) -> String {
    let (e, name) = (escape, escape(APP_NAME));
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Inicia {name} al iniciar sesión (overclock, LED y ventiladores).</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
      <Delay>PT{delay_s}S</Delay>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
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
      <Command>{exe}</Command>
      <Arguments>{args}</Arguments>
      <WorkingDirectory>{workdir}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#,
        user = e(user),
        exe = e(exe),
        args = e(args),
        workdir = e(workdir),
    )
}

pub fn current_user() -> String {
    let dom = std::env::var("USERDOMAIN").unwrap_or_default();
    let usr = std::env::var("USERNAME").unwrap_or_default();
    if dom.is_empty() { usr } else { format!("{dom}\\{usr}") }
}

/// Ejecuta un comando sin abrir ventana de consola (Windows).
pub fn system_runner(cmd: &[String]) -> std::io::Result<CmdResult> {
    let mut c = std::process::Command::new(&cmd[0]);
    c.args(&cmd[1..]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let o = c.output()?;
    Ok(CmdResult {
        code: o.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
    })
}

fn cmd(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

fn denied(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("denied") || m.contains("denegado")
}

pub fn is_enabled() -> bool {
    is_enabled_with(&system_runner)
}

pub fn is_enabled_with(run: Runner) -> bool {
    is_supported() && run(&cmd(&["schtasks", "/Query", "/TN", TASK_NAME])).is_ok_and(|r| r.code == 0)
}

pub fn enable(delay_s: u32, exe_override: Option<&Path>) -> HwResult<()> {
    enable_with(&system_runner, delay_s, exe_override, is_supported())
}

pub fn enable_with(run: Runner, delay_s: u32, exe_override: Option<&Path>, supported: bool) -> HwResult<()> {
    if !supported {
        return Err(HwError::new("El inicio con el sistema solo está disponible en Windows."));
    }
    let (exe, args, workdir) = launch_command(exe_override);
    let xml = build_task_xml(&exe, &args, &workdir, &current_user(), delay_s);

    // schtasks espera el XML en UTF-16 LE con BOM
    let path = std::env::temp_dir().join(format!("gpupy_task_{}.xml", std::process::id()));
    let bytes: Vec<u8> =
        std::iter::once(0xFEFFu16).chain(xml.encode_utf16()).flat_map(|u| u.to_le_bytes()).collect();
    std::fs::write(&path, bytes).map_err(|e| HwError::new(format!("No se pudo escribir el XML de la tarea: {e}")))?;
    let result = run(&cmd(&["schtasks", "/Create", "/TN", TASK_NAME, "/XML", &path.display().to_string(), "/F"]));
    let _ = std::fs::remove_file(&path);

    let r = result.map_err(|e| HwError::new(format!("No se pudo ejecutar schtasks: {e}")))?;
    if r.code != 0 {
        let raw = if r.stderr.trim().is_empty() { &r.stdout } else { &r.stderr };
        let mut msg = raw.trim().to_string();
        if denied(&msg) {
            msg = "Acceso denegado. Abre GPU PY como Administrador para activar el inicio con Windows.".into();
        }
        return Err(HwError::new(if msg.is_empty() { "schtasks devolvió un error.".to_string() } else { msg }));
    }
    Ok(())
}

pub fn disable() -> HwResult<()> {
    disable_with(&system_runner, is_supported())
}

pub fn disable_with(run: Runner, supported: bool) -> HwResult<()> {
    if !supported {
        return Ok(());
    }
    let r = run(&cmd(&["schtasks", "/Delete", "/TN", TASK_NAME, "/F"]))
        .map_err(|e| HwError::new(format!("No se pudo ejecutar schtasks: {e}")))?;
    if r.code != 0 && is_enabled_with(run) {
        let raw = if r.stderr.trim().is_empty() { &r.stdout } else { &r.stderr };
        let mut msg = raw.trim().to_string();
        if denied(&msg) {
            msg = "Acceso denegado. Abre GPU PY como Administrador.".into();
        }
        return Err(HwError::new(if msg.is_empty() { "No se pudo eliminar la tarea.".to_string() } else { msg }));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn xml_escapes_and_contains_fields() {
        let x = build_task_xml(r"C:\A&B\GPU-PY.exe", "--startup", r"C:\A&B", r"PC\Ana<1>", 20);
        assert!(x.contains(r"<Command>C:\A&amp;B\GPU-PY.exe</Command>"));
        assert!(x.contains("<UserId>PC\\Ana&lt;1&gt;</UserId>"));
        assert!(x.contains("<Delay>PT20S</Delay>"));
        assert!(x.contains("<RunLevel>HighestAvailable</RunLevel>"));
    }

    #[test]
    fn enable_reports_denied_in_spanish() {
        let run = |_: &[String]| Ok(CmdResult { code: 1, stderr: "ERROR: Access is denied.".into(), ..Default::default() });
        let e = enable_with(&run, 20, Some(Path::new("/x/GPU-PY.exe")), true).unwrap_err();
        assert!(e.0.starts_with("Acceso denegado"));
    }

    #[test]
    fn enable_passes_xml_file_to_schtasks() {
        let seen = RefCell::new(vec![]);
        let run = |c: &[String]| {
            seen.borrow_mut().push(c.to_vec());
            Ok(CmdResult::default())
        };
        enable_with(&run, 5, Some(Path::new("/x/GPU-PY.exe")), true).unwrap();
        let c = &seen.borrow()[0];
        assert_eq!(&c[..4], ["schtasks", "/Create", "/TN", TASK_NAME]);
        assert_eq!(c[4], "/XML");
        assert_eq!(c.last().unwrap(), "/F");
    }

    #[test]
    fn unsupported_platform() {
        let run = |_: &[String]| Ok(CmdResult::default());
        assert!(enable_with(&run, 5, None, false).is_err());
        assert!(disable_with(&run, false).is_ok());
    }
}
