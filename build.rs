//! Compila la interfaz Slint y, en Windows, embebe icono + manifiesto (UAC).

fn main() {
    let mut config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    // Solo para `examples/screenshot.rs`: incrusta fuentes para poder renderizar sin ventana.
    println!("cargo:rerun-if-env-changed=GPUPY_SCREENSHOT");
    if std::env::var_os("GPUPY_SCREENSHOT").is_some() {
        config = config.embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
    }
    slint_build::compile_with_config("ui/app.slint", config).expect("error compilando la interfaz Slint");

    println!("cargo:rerun-if-changed=assets/gpupy.ico");
    println!("cargo:rerun-if-changed=assets/gpupy.png");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/gpupy.ico");
        res.set("ProductName", "GPU PY");
        res.set("FileDescription", "GPU PY - Monitor, overclock, ventiladores y LED para GPUs NVIDIA");
        // Igual que el `--uac-admin` de PyInstaller: el .exe de release pide Administrador al abrir
        // (hace falta para aplicar overclock y LED). En debug no, para poder usar `cargo run`.
        if std::env::var("PROFILE").as_deref() == Ok("release") {
            res.set_manifest(
                r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security><requestedPrivileges>
      <requestedExecutionLevel level="requireAdministrator" uiAccess="false"/>
    </requestedPrivileges></security>
  </trustInfo>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>"#,
            );
        }
        res.compile().expect("no se pudo embeber el recurso de Windows");
    }
}
