//! Actualizaciones desde los releases de GitHub, e instalación como `/Applications/Telar.app`.
//!
//! Flujo: `check` consulta el último release; `install` descarga el paquete a una carpeta
//! temporal junto al binario (mismo disco, para que el reemplazo sea un `rename` atómico),
//! verifica su SHA-256, comprueba que el binario nuevo arranque y reemplaza al actual. La
//! carpeta temporal se borra siempre, salga bien o mal: no quedan descargas ni versiones viejas.
//!
//! Sin dependencias: usa `curl`, `shasum`, `tar` y `codesign` del sistema.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::Value;
use winit::event_loop::EventLoopProxy;

use crate::UserEvent;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO: &str = "cbustosm02/telar-terminal";
/// Nombre del paquete de cada release (binario universal: Apple Silicon e Intel).
const ASSET_SUFFIX: &str = "-macos-universal.tar.gz";
const TMP_PREFIX: &str = ".telar-update-";

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub sha_url: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    /// Bytes descargados y total.
    Downloading(String, u64, u64),
    Applying(String),
    /// Listo: falta reiniciar.
    Ready(String),
    Error(String),
}

impl Status {
    /// Progreso de 0 a 1 (para la barra), si hay algo en curso.
    pub fn progress(&self) -> Option<f32> {
        match self {
            Status::Downloading(_, done, total) if *total > 0 => Some((*done as f32 / *total as f32).min(1.0) * 0.9),
            Status::Downloading(..) => Some(0.0),
            Status::Applying(_) => Some(0.95),
            _ => None,
        }
    }
}

fn send(proxy: &EventLoopProxy<UserEvent>, status: Status) {
    let _ = proxy.send_event(UserEvent::Update(status));
}

/// `x.y.z` → números, para comparar. Ignora una `v` inicial y sufijos (`-beta`).
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches('v');
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next()??, it.next().unwrap_or(Some(0))?))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse_version(candidate), parse_version(current)), (Some(a), Some(b)) if a > b)
}

/// Del JSON de `releases/latest`, el release con su paquete y su checksum.
fn release_from_json(v: &Value) -> Result<Release, String> {
    let tag = v["tag_name"].as_str().ok_or("el release no tiene tag")?;
    let assets = v["assets"].as_array().ok_or("el release no tiene archivos")?;
    let pkg = assets
        .iter()
        .find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(ASSET_SUFFIX)))
        .ok_or_else(|| format!("el release {tag} no tiene un paquete *{ASSET_SUFFIX}"))?;
    let name = pkg["name"].as_str().unwrap_or_default();
    let sha = assets
        .iter()
        .find(|a| a["name"].as_str() == Some(&format!("{name}.sha256")))
        .ok_or_else(|| format!("el release {tag} no tiene {name}.sha256"))?;
    Ok(Release {
        version: tag.trim_start_matches('v').to_string(),
        url: pkg["browser_download_url"].as_str().unwrap_or_default().to_string(),
        sha_url: sha["browser_download_url"].as_str().unwrap_or_default().to_string(),
        size: pkg["size"].as_u64().unwrap_or(0),
    })
}

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("/usr/bin/curl")
        .args(["-fsSL", "--max-time", "30", "-H", "User-Agent: telar-updater"])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("no se pudo consultar GitHub: {}", err.trim()));
    }
    Ok(out.stdout)
}

/// Último release publicado y si es más nuevo que esta versión.
pub fn latest() -> Result<Status, String> {
    let body = curl(&[&format!("https://api.github.com/repos/{REPO}/releases/latest")])?;
    let r = release_from_json(&serde_json::from_slice::<Value>(&body).map_err(|e| e.to_string())?)?;
    Ok(if is_newer(&r.version, VERSION) { Status::Available(r) } else { Status::UpToDate })
}

/// Busca el último release (en un hilo; el resultado llega como `UserEvent::Update`).
pub fn check(proxy: EventLoopProxy<UserEvent>) {
    send(&proxy, Status::Checking);
    std::thread::spawn(move || send(&proxy, latest().unwrap_or_else(Status::Error)));
}

/// Binario en ejecución, sin enlaces simbólicos.
fn current_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::fs::canonicalize(&exe).map_err(|e| e.to_string())
}

/// Una compilación de desarrollo (`target/…`) no se reemplaza: se actualiza con git y cargo.
pub fn dev_build(exe: &Path) -> bool {
    exe.components().any(|c| c.as_os_str() == "target")
}

/// La `.app` que contiene al binario, si corre desde una.
fn app_bundle(exe: &Path) -> Option<&Path> {
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app"))
}

/// Borra restos de una actualización interrumpida (p. ej. si se cortó la luz).
pub fn cleanup_leftovers() {
    let Some(dir) = current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for e in entries.flatten() {
        if e.file_name().to_string_lossy().starts_with(TMP_PREFIX) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Descarga y aplica `release` (en un hilo, informando el progreso).
pub fn install(proxy: EventLoopProxy<UserEvent>, release: Release) {
    std::thread::spawn(move || {
        let result = install_sync(&release, &|s| send(&proxy, s));
        send(&proxy, match result {
            Ok(()) => Status::Ready(release.version.clone()),
            Err(e) => Status::Error(e),
        });
    });
}

/// Descarga y aplica `release` sobre el binario en ejecución; `progress` recibe el avance.
pub fn install_sync(release: &Release, progress: &dyn Fn(Status)) -> Result<(), String> {
    let exe = current_exe()?;
    if dev_build(&exe) {
        return Err(format!(
            "Es una compilación de desarrollo ({}): actualiza con git pull y cargo build, o instala con telar install",
            exe.display()
        ));
    }
    install_to(&exe, release, progress)
}

/// Aplica `release` sobre el binario `exe`. La carpeta temporal se borra siempre.
fn install_to(exe: &Path, release: &Release, progress: &dyn Fn(Status)) -> Result<(), String> {
    let tmp = exe.parent().unwrap_or(Path::new("/tmp")).join(format!("{TMP_PREFIX}{}", std::process::id()));
    let result = apply(progress, release, exe, &tmp);
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

fn apply(progress: &dyn Fn(Status), r: &Release, exe: &Path, tmp: &Path) -> Result<(), String> {
    std::fs::create_dir_all(tmp).map_err(|e| format!("no se pudo crear {}: {e}", tmp.display()))?;
    let pkg = tmp.join("telar.tar.gz");

    // Descarga, informando el avance según el tamaño del archivo.
    let mut child = Command::new("/usr/bin/curl")
        .args(["-fsSL", "--max-time", "600", "-H", "User-Agent: telar-updater", "-o"])
        .arg(&pkg)
        .arg(&r.url)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl: {e}"))?;
    loop {
        let done = std::fs::metadata(&pkg).map(|m| m.len()).unwrap_or(0);
        progress(Status::Downloading(r.version.clone(), done, r.size));
        match child.try_wait() {
            Ok(Some(st)) if st.success() => break,
            Ok(Some(_)) => return Err("falló la descarga".into()),
            Ok(None) => std::thread::sleep(Duration::from_millis(120)),
            Err(e) => return Err(e.to_string()),
        }
    }
    progress(Status::Applying(r.version.clone()));

    // Integridad: el SHA-256 publicado junto al paquete.
    let expected = String::from_utf8_lossy(&curl(&[&r.sha_url])?).split_whitespace().next().unwrap_or_default().to_lowercase();
    let out = Command::new("/usr/bin/shasum").args(["-a", "256"]).arg(&pkg).output().map_err(|e| e.to_string())?;
    let actual = String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap_or_default().to_lowercase();
    if expected.len() != 64 || expected != actual {
        return Err("el paquete descargado no coincide con su SHA-256".into());
    }

    let st = Command::new("/usr/bin/tar").arg("-xzf").arg(&pkg).arg("-C").arg(tmp).status().map_err(|e| e.to_string())?;
    if !st.success() {
        return Err("no se pudo descomprimir el paquete".into());
    }
    let new = tmp.join("telar");
    // El binario nuevo tiene que arrancar y ser la versión anunciada.
    let out = Command::new(&new).arg("--version").output().map_err(|e| format!("el binario nuevo no arranca: {e}"))?;
    let reported = String::from_utf8_lossy(&out.stdout);
    if !reported.contains(&r.version) {
        return Err(format!("el binario nuevo informa «{}» en vez de {}", reported.trim(), r.version));
    }

    // Reemplazo atómico: el proceso actual sigue usando el binario viejo hasta reiniciar.
    std::fs::rename(&new, exe).map_err(|e| format!("no se pudo reemplazar {}: {e}", exe.display()))?;
    if let Some(app) = app_bundle(exe) {
        sign(app);
    }
    Ok(())
}

/// Firma ad hoc (sin certificado): macOS exige firma en Apple Silicon y la del paquete
/// deja de valer al cambiar el binario.
fn sign(path: &Path) {
    let _ = Command::new("/usr/bin/codesign").args(["--force", "--sign", "-"]).arg(path).stderr(Stdio::null()).status();
}

/// Reinicia Telar cuando este proceso termine (la sesión ya quedó guardada al salir).
pub fn relaunch() {
    let Ok(exe) = current_exe() else { return };
    let target = match app_bundle(&exe) {
        Some(app) => format!("/usr/bin/open {}", shell_quote(&app.display().to_string())),
        None => format!("{} >/dev/null 2>&1", shell_quote(&exe.display().to_string())),
    };
    let script = format!("while kill -0 {} 2>/dev/null; do sleep 0.1; done; {target}", std::process::id());
    let _ = Command::new("/bin/sh")
        .args(["-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn user_app_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("Applications").join("Telar.app")
}

/// `/Applications/Telar.app` (la ve el Finder en «Aplicaciones»); si no se puede escribir
/// ahí (usuario sin permisos de administrador), `~/Applications/Telar.app`.
pub fn app_path() -> PathBuf {
    let system = Path::new("/Applications");
    let writable = std::fs::metadata(system).is_ok_and(|m| !m.permissions().readonly())
        && Command::new("/bin/test").arg("-w").arg(system).status().is_ok_and(|s| s.success());
    if writable { system.join("Telar.app") } else { user_app_path() }
}

/// Crea (o actualiza) `/Applications/Telar.app` con una copia de este binario.
/// Devuelve la ruta del binario dentro de la app.
pub fn install_app() -> Result<PathBuf, String> {
    let exe = current_exe()?;
    let app = app_path();
    let macos = app.join("Contents").join("MacOS");
    let dest = macos.join("telar");
    std::fs::create_dir_all(&macos).map_err(|e| e.to_string())?;
    std::fs::write(app.join("Contents").join("Info.plist"), info_plist()).map_err(|e| e.to_string())?;
    if exe != dest {
        // Copia a un temporal y rename: si Telar.app está abierta, sigue funcionando.
        let tmp = macos.join(".telar-new");
        std::fs::copy(&exe, &tmp).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
    }
    sign(&app);
    // Una copia de antes en ~/Applications ya no hace falta.
    let old = user_app_path();
    if old != app && old.exists() {
        let _ = std::fs::remove_dir_all(&old);
    }
    Ok(dest)
}

fn info_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Telar</string>
  <key>CFBundleDisplayName</key><string>Telar</string>
  <key>CFBundleIdentifier</key><string>dev.cbustos.telar</string>
  <key>CFBundleExecutable</key><string>telar</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>{VERSION}</string>
  <key>CFBundleVersion</key><string>{VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
"#
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn comparar_versiones() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(is_newer("0.1.10", "0.1.9"), "numérico, no alfabético");
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(!is_newer("basura", "0.1.0"));
        assert!(is_newer("0.2", "0.1.5"), "sin parche cuenta como .0");
        assert!(!is_newer("0.2.0-beta", "0.2.0"));
    }

    fn asset(name: &str, size: u64) -> Value {
        json!({ "name": name, "size": size, "browser_download_url": format!("https://x/{name}") })
    }

    #[test]
    fn release_con_paquete_y_checksum() {
        let v = json!({
            "tag_name": "v0.2.0",
            "assets": [asset("telar-0.2.0-macos-universal.tar.gz.sha256", 90), asset("telar-0.2.0-macos-universal.tar.gz", 7_000_000)],
        });
        let r = release_from_json(&v).unwrap();
        assert_eq!(r.version, "0.2.0");
        assert_eq!(r.size, 7_000_000);
        assert!(r.url.ends_with(".tar.gz") && r.sha_url.ends_with(".sha256"));
    }

    #[test]
    fn release_incompleto() {
        assert!(release_from_json(&json!({ "tag_name": "v1.0.0", "assets": [] })).is_err());
        let sin_sha = json!({ "tag_name": "v1.0.0", "assets": [asset("telar-1.0.0-macos-universal.tar.gz", 1)] });
        assert!(release_from_json(&sin_sha).unwrap_err().contains("sha256"));
    }

    #[test]
    fn progreso() {
        assert_eq!(Status::Downloading("1".into(), 50, 100).progress(), Some(0.45));
        assert_eq!(Status::Downloading("1".into(), 0, 0).progress(), Some(0.0));
        assert_eq!(Status::Idle.progress(), None);
    }

    #[test]
    fn rutas() {
        assert!(dev_build(Path::new("/Users/x/Documents/telar/target/release/telar")));
        assert!(!dev_build(Path::new("/Users/x/Applications/Telar.app/Contents/MacOS/telar")));
        let exe = Path::new("/Users/x/Applications/Telar.app/Contents/MacOS/telar");
        assert_eq!(app_bundle(exe), Some(Path::new("/Users/x/Applications/Telar.app")));
        assert_eq!(app_bundle(Path::new("/usr/local/bin/telar")), None);
        assert_eq!(shell_quote("a'b"), r"'a'\''b'");
    }

    /// Arma un release local (file://) cuyo «binario» es un script que informa `reports`.
    fn release_local(dir: &Path, version: &str, reports: &str, bad_sha: bool) -> Release {
        let pkg_dir = dir.join("pkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        let bin = pkg_dir.join("telar");
        std::fs::write(&bin, format!("#!/bin/sh\necho telar {reports}\n")).unwrap();
        Command::new("/bin/chmod").arg("755").arg(&bin).status().unwrap();
        let pkg = dir.join("telar.tar.gz");
        assert!(Command::new("/usr/bin/tar").arg("-czf").arg(&pkg).arg("-C").arg(&pkg_dir).arg("telar").status().unwrap().success());
        let out = Command::new("/usr/bin/shasum").args(["-a", "256"]).arg(&pkg).output().unwrap();
        let mut sha = String::from_utf8_lossy(&out.stdout).to_string();
        if bad_sha {
            sha = format!("{}{}", "0".repeat(64), &sha[64..]);
        }
        let sha_file = dir.join("telar.tar.gz.sha256");
        std::fs::write(&sha_file, sha).unwrap();
        Release {
            version: version.into(),
            url: format!("file://{}", pkg.display()),
            sha_url: format!("file://{}", sha_file.display()),
            size: std::fs::metadata(&pkg).unwrap().len(),
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("telar-prueba-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("app")).unwrap();
        d
    }

    fn leftovers(app: &Path) -> Vec<String> {
        std::fs::read_dir(app).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(TMP_PREFIX)).collect()
    }

    #[test]
    fn aplica_la_actualizacion_y_no_deja_basura() {
        let dir = temp_dir("ok");
        let exe = dir.join("app").join("telar");
        std::fs::write(&exe, "viejo").unwrap();
        let release = release_local(&dir, "9.9.9", "9.9.9", false);
        let steps = std::cell::RefCell::new(Vec::new());
        install_to(&exe, &release, &|s| steps.borrow_mut().push(s)).unwrap();
        let out = Command::new(&exe).arg("--version").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "telar 9.9.9");
        assert!(leftovers(&dir.join("app")).is_empty(), "no queda la carpeta temporal");
        let steps = steps.borrow();
        assert!(matches!(steps.first(), Some(Status::Downloading(..))));
        assert!(matches!(steps.last(), Some(Status::Applying(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checksum_malo_no_toca_nada() {
        let dir = temp_dir("sha");
        let exe = dir.join("app").join("telar");
        std::fs::write(&exe, "viejo").unwrap();
        let release = release_local(&dir, "9.9.9", "9.9.9", true);
        let err = install_to(&exe, &release, &|_| {}).unwrap_err();
        assert!(err.contains("SHA-256"), "{err}");
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "viejo");
        assert!(leftovers(&dir.join("app")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn binario_con_otra_version_no_se_instala() {
        let dir = temp_dir("ver");
        let exe = dir.join("app").join("telar");
        std::fs::write(&exe, "viejo").unwrap();
        let release = release_local(&dir, "9.9.9", "1.0.0", false);
        assert!(install_to(&exe, &release, &|_| {}).unwrap_err().contains("9.9.9"));
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "viejo");
        assert!(leftovers(&dir.join("app")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plist_con_version() {
        assert!(info_plist().contains(&format!("<string>{VERSION}</string>")));
    }
}
