//! Cuentas de Claude Code: guardar varias y cambiar la sesión activa entre ellas, como
//! hace cswap (claude-swap, MIT), del que se adaptan el acceso al Llavero y los bloqueos.
//!
//! Claude Code guarda su sesión en dos lugares:
//! - la credencial OAuth, en el Llavero (servicio `Claude Code-credentials`, cuenta `$USER`);
//! - la identidad (correo, organización), en `~/.claude.json` › `oauthAccount`.
//!
//! Cambiar de cuenta es poner la credencial y la identidad de otra. Lo delicado:
//! - **El refresh token rota.** Cada vez que Claude Code renueva el token, el anterior deja
//!   de servir. Una copia guardada vieja de la cuenta activa queda inválida y obliga a
//!   iniciar sesión otra vez. Por eso, antes de cambiar, se guarda la credencial viva (la
//!   última generación), y al refrescar la cuenta se mantiene al día la copia de la activa.
//! - **Claude Code abierto puede renovar el token a mitad del cambio** y pisarlo con el de
//!   la cuenta vieja. Se toman los mismos bloqueos que usa Claude Code para renovar y para
//!   escribir `~/.claude.json` (directorios de `proper-lockfile`): con ellos tomados, su
//!   renovación espera y, al releer, ve la credencial nueva.
//! - **Un fallo a la mitad** no deja una mezcla: si no se puede escribir `~/.claude.json`,
//!   se vuelve a poner la credencial anterior.
//!
//! Las copias van en el Llavero (servicio `Telar · cuentas de Claude`, cuenta = clave de
//! la cuenta) con el `oauthAccount`; el índice sin secretos, en
//! `~/.config/telar/cuentas-claude.json`. El Llavero se usa con `/usr/bin/security`, como
//! Claude Code: el secreto va por la entrada estándar (no en los argumentos del proceso) y
//! creador y lector son el mismo programa, así que macOS no pide permiso.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

const SECURITY: &str = "/usr/bin/security";
/// Ítem del Llavero con la sesión activa de Claude Code.
const LIVE_SERVICE: &str = "Claude Code-credentials";
/// Ítems del Llavero con las copias de Telar.
const STORE_SERVICE: &str = "Telar · cuentas de Claude";
/// Código de salida de `security` cuando el ítem no existe.
const NOT_FOUND: i32 = 44;

/// Una cuenta guardada (sin secretos).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    /// `correo|organización`: el mismo correo puede tener una cuenta personal y una de equipo.
    pub key: String,
    pub email: String,
    pub org: Option<String>,
    pub plan: Option<String>,
}

/// Identidad de la sesión activa (de `~/.claude.json`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub key: String,
    pub email: String,
    pub org: Option<String>,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

/// Carpeta de configuración de Claude Code (`~/.claude`, o `CLAUDE_CONFIG_DIR`).
fn config_home() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".claude"))
}

/// `~/.claude.json` (o `$CLAUDE_CONFIG_DIR/.claude.json`).
fn global_config() -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir).join(".claude.json"),
        None => home().join(".claude.json"),
    }
}

fn index_path() -> PathBuf {
    home().join(".config").join("telar").join("cuentas-claude.json")
}

/// Cuenta del Llavero de la sesión activa: la misma que usa Claude Code (`$USER`).
fn keychain_user() -> String {
    std::env::var("USER").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| {
        // SAFETY: getpwuid devuelve un puntero a memoria estática o NULL.
        unsafe {
            let pw = libc::getpwuid(libc::geteuid());
            if pw.is_null() {
                "claude-code-user".into()
            } else {
                std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned()
            }
        }
    })
}

pub fn key_of(email: &str, org_uuid: Option<&str>) -> String {
    format!("{}|{}", email.to_lowercase(), org_uuid.unwrap_or(""))
}

// ── Llavero ─────────────────────────────────────────────────────────────────────

fn kc_get(service: &str, account: &str) -> Result<Option<String>, String> {
    let out = Command::new(SECURITY)
        .args(["find-generic-password", "-a", account, "-w", "-s", service])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("no se pudo usar el Llavero: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(Some(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string())),
        Some(NOT_FOUND) => Ok(None),
        _ => Err(format!("el Llavero no respondió: {}", String::from_utf8_lossy(&out.stderr).trim())),
    }
}

/// Comillas para una línea de `security -i` (la vuelve a partir como una shell).
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn kc_set(service: &str, account: &str, value: &str) -> Result<(), String> {
    let hex: String = value.bytes().map(|b| format!("{b:02x}")).collect();
    let line = format!("add-generic-password -U -a {} -s {} -X {hex}\n", quote(account), quote(service));
    // `security -i` lee líneas de hasta 4096 bytes; una más larga se cortaría y dañaría el
    // ítem: en ese caso (raro) va en los argumentos.
    let out = if line.len() <= 4096 - 64 {
        let mut child = Command::new(SECURITY)
            .arg("-i")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("no se pudo usar el Llavero: {e}"))?;
        child.stdin.take().map(|mut i| i.write_all(line.as_bytes())).transpose().map_err(|e| e.to_string())?;
        child.wait_with_output().map_err(|e| e.to_string())?
    } else {
        Command::new(SECURITY)
            .args(["add-generic-password", "-U", "-a", account, "-s", service, "-X", &hex])
            .output()
            .map_err(|e| format!("no se pudo usar el Llavero: {e}"))?
    };
    // `security -i` sale con 0 aunque falle el comando: el error queda en la salida.
    let err = String::from_utf8_lossy(&out.stderr);
    if out.status.success() && !err.contains("error") {
        Ok(())
    } else {
        Err(format!("no se pudo guardar en el Llavero: {}", err.trim()))
    }
}

fn kc_delete(service: &str, account: &str) -> Result<(), String> {
    let out = Command::new(SECURITY)
        .args(["delete-generic-password", "-a", account, "-s", service])
        .output()
        .map_err(|e| format!("no se pudo usar el Llavero: {e}"))?;
    match out.status.code() {
        Some(0) | Some(NOT_FOUND) => Ok(()),
        _ => Err(format!("no se pudo borrar del Llavero: {}", String::from_utf8_lossy(&out.stderr).trim())),
    }
}

// ── Bloqueos de Claude Code ─────────────────────────────────────────────────────

/// Bloqueo de `proper-lockfile`: un directorio (crearlo es atómico). Se suelta al salir.
struct DirLock(PathBuf);

impl Drop for DirLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

/// Toma el bloqueo `path`. Uno más viejo que `stale` es de un proceso que murió y se
/// quita; si no, se espera hasta `timeout` (Claude Code lo tiene lo que dura una
/// renovación o una escritura, menos de unos segundos).
fn lock(path: &Path, stale: Duration, timeout: Duration) -> Result<DirLock, String> {
    let start = Instant::now();
    loop {
        match std::fs::create_dir(path) {
            Ok(()) => return Ok(DirLock(path.to_path_buf())),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let age = std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok());
                if age.is_some_and(|a| a > stale) {
                    let _ = std::fs::remove_dir(path);
                    continue;
                }
                if start.elapsed() > timeout {
                    return Err(format!("Claude Code tiene tomado {} y no lo suelta; intenta de nuevo en un momento", path.display()));
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(e) => return Err(format!("no se pudo tomar {}: {e}", path.display())),
        }
    }
}

/// Los tres bloqueos, en el orden de Claude Code: renovación (`.oauth_refresh.lock`, y el
/// heredado `~/.claude.lock`) y escritura de `~/.claude.json`.
fn claude_locks() -> Result<Vec<DirLock>, String> {
    let conf = config_home();
    let legacy = conf.parent().map(|p| p.join(format!("{}.lock", conf.file_name().map_or("".into(), |n| n.to_string_lossy())))).unwrap_or_default();
    let timeout = Duration::from_secs(9);
    let mut held = Vec::new();
    if conf.is_dir() {
        held.push(lock(&conf.join(".oauth_refresh.lock"), Duration::from_secs(60), timeout)?);
    }
    held.push(lock(&legacy, Duration::from_secs(60), timeout)?);
    let mut config_lock = global_config().into_os_string();
    config_lock.push(".lock");
    held.push(lock(Path::new(&config_lock), Duration::from_secs(10), timeout)?);
    Ok(held)
}

// ── Sesión activa ───────────────────────────────────────────────────────────────

fn read_config() -> Result<Value, String> {
    let text = std::fs::read_to_string(global_config()).map_err(|e| format!("no se pudo leer ~/.claude.json: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("~/.claude.json no es JSON válido: {e}"))
}

/// Escritura atómica de `~/.claude.json` (temporal y renombrar), con sus permisos.
fn write_config(v: &Value) -> Result<(), String> {
    let path = global_config();
    let tmp = path.with_extension("json.telar-tmp");
    let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).map_err(|e| format!("no se pudo escribir ~/.claude.json: {e}"))?;
    if let Ok(m) = std::fs::metadata(&path) {
        let _ = std::fs::set_permissions(&tmp, m.permissions());
    }
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("no se pudo reemplazar ~/.claude.json: {e}")
    })
}

fn identity_of(oauth: &Value) -> Option<Identity> {
    let email = oauth["emailAddress"].as_str().filter(|e| !e.is_empty())?.to_string();
    let org_uuid = oauth["organizationUuid"].as_str().filter(|s| !s.is_empty());
    Some(Identity {
        key: key_of(&email, org_uuid),
        email,
        org: oauth["organizationName"].as_str().filter(|s| !s.is_empty()).map(String::from),
    })
}

/// Cuenta con que está iniciada la sesión de Claude Code ahora.
pub fn current() -> Option<Identity> {
    identity_of(&read_config().ok()?["oauthAccount"])
}

/// Plan de una credencial (`claudeAiOauth.subscriptionType`: max, pro, team…).
fn plan_of(credentials: &str) -> Option<String> {
    let v: Value = serde_json::from_str(credentials).ok()?;
    crate::claude_usage::plan_name(v["claudeAiOauth"]["subscriptionType"].as_str(), None)
}

// ── Índice ──────────────────────────────────────────────────────────────────────

pub fn list() -> Vec<Saved> {
    let Ok(text) = std::fs::read_to_string(index_path()) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
    v["accounts"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    Some(Saved {
                        key: e["key"].as_str()?.to_string(),
                        email: e["email"].as_str()?.to_string(),
                        org: e["org"].as_str().map(String::from),
                        plan: e["plan"].as_str().map(String::from),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn write_index(accounts: &[Saved]) -> Result<(), String> {
    let path = index_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let list: Vec<Value> = accounts.iter().map(|a| json!({ "key": a.key, "email": a.email, "org": a.org, "plan": a.plan })).collect();
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&json!({ "accounts": list })).unwrap_or_default()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// Guarda (o actualiza) la copia de una cuenta: su credencial y su `oauthAccount`.
fn store(id: &Identity, credentials: &str, oauth: &Value) -> Result<Saved, String> {
    let blob = json!({ "credentials": credentials, "oauthAccount": oauth }).to_string();
    kc_set(STORE_SERVICE, &id.key, &blob)?;
    let saved = Saved { key: id.key.clone(), email: id.email.clone(), org: id.org.clone(), plan: plan_of(credentials) };
    let mut all = list();
    match all.iter_mut().find(|a| a.key == saved.key) {
        Some(a) => *a = saved.clone(),
        None => all.push(saved.clone()),
    }
    write_index(&all)?;
    Ok(saved)
}

/// Copia guardada de una cuenta: (credencial, oauthAccount).
fn stored(key: &str) -> Result<(String, Value), String> {
    let blob = kc_get(STORE_SERVICE, key)?.ok_or("la copia de esa cuenta no está en el Llavero; agrégala otra vez")?;
    let v: Value = serde_json::from_str(&blob).map_err(|_| "la copia de esa cuenta está dañada; agrégala otra vez".to_string())?;
    let creds = v["credentials"].as_str().filter(|c| !c.is_empty()).ok_or("la copia de esa cuenta no tiene credencial")?.to_string();
    let oauth = v["oauthAccount"].clone();
    if identity_of(&oauth).is_none() {
        return Err("la copia de esa cuenta no tiene correo".into());
    }
    Ok((creds, oauth))
}

// ── Operaciones ─────────────────────────────────────────────────────────────────

/// Agrega la cuenta con que está iniciada la sesión ahora; si ya estaba, la actualiza.
pub fn save_current() -> Result<Saved, String> {
    let _locks = claude_locks()?;
    let config = read_config()?;
    let oauth = config["oauthAccount"].clone();
    let id = identity_of(&oauth).ok_or("Claude Code no tiene una sesión iniciada: entra con `claude` y vuelve a intentar")?;
    let creds = kc_get(LIVE_SERVICE, &keychain_user())?.ok_or("Claude Code no tiene credencial en el Llavero: inicia sesión y vuelve a intentar")?;
    store(&id, &creds, &oauth)
}

/// Mantiene al día la copia de la cuenta activa (si está guardada): Claude Code rota el
/// refresh token y una copia vieja dejaría de servir. No toma bloqueos: solo lee.
pub fn sync_current() {
    let Ok(config) = read_config() else { return };
    let oauth = config["oauthAccount"].clone();
    let Some(id) = identity_of(&oauth) else { return };
    if !list().iter().any(|a| a.key == id.key) {
        return;
    }
    let Ok(Some(live)) = kc_get(LIVE_SERVICE, &keychain_user()) else { return };
    if stored(&id.key).is_ok_and(|(creds, _)| creds == live) {
        return;
    }
    // La identidad y la credencial se leyeron por separado: solo se guarda si siguen
    // siendo de la misma cuenta (si en el medio se cambió de cuenta, se deja).
    if current().is_some_and(|now| now.key == id.key) {
        let _ = store(&id, &live, &oauth);
    }
}

/// Quita la copia guardada de una cuenta (no cierra su sesión si es la activa).
pub fn remove(key: &str) -> Result<(), String> {
    kc_delete(STORE_SERVICE, key)?;
    let all: Vec<Saved> = list().into_iter().filter(|a| a.key != key).collect();
    write_index(&all)
}

/// Pasa la sesión de Claude Code a la cuenta guardada `key`.
pub fn switch(key: &str) -> Result<Saved, String> {
    let _guard = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let (target_creds, target_oauth) = stored(key)?;
    let target_id = identity_of(&target_oauth).ok_or("la copia de esa cuenta no tiene correo")?;
    let _locks = claude_locks()?;
    let mut config = read_config()?;
    let user = keychain_user();
    let live_creds = kc_get(LIVE_SERVICE, &user)?;
    let live_oauth = config["oauthAccount"].clone();
    let live_id = identity_of(&live_oauth);
    if live_id.as_ref().is_some_and(|l| l.key == target_id.key) {
        return list().into_iter().find(|a| a.key == key).ok_or_else(|| "esa cuenta ya no está guardada".into());
    }
    // 1. La cuenta que se deja: se guarda su credencial viva (la última generación), así al
    //    volver a ella no hace falta iniciar sesión. Si no estaba guardada, queda agregada.
    if let (Some(id), Some(creds)) = (&live_id, &live_creds) {
        store(id, creds, &live_oauth)?;
    }
    // 2. La credencial de la cuenta nueva.
    kc_set(LIVE_SERVICE, &user, &target_creds)?;
    // 3. Su identidad en ~/.claude.json; si falla, se vuelve a la credencial anterior.
    config["oauthAccount"] = target_oauth;
    if let Err(e) = write_config(&config) {
        if let Some(prev) = &live_creds {
            let _ = kc_set(LIVE_SERVICE, &user, prev);
        }
        return Err(e);
    }
    // 4. Comprobación: el Llavero tiene la credencial nueva.
    match kc_get(LIVE_SERVICE, &user) {
        Ok(Some(now)) if now == target_creds => {}
        _ => return Err("la credencial nueva no quedó en el Llavero; revisa con `claude auth status`".into()),
    }
    Ok(list().into_iter().find(|a| a.key == key).unwrap_or(Saved {
        key: key.to_string(),
        email: target_id.email,
        org: target_id.org,
        plan: plan_of(&target_creds),
    }))
}

/// Trae una vez las cuentas que ya tenía cswap (`~/.claude-swap-backup`), para no tener
/// que iniciar sesión de nuevo con cada una. No pisa las que Telar ya tiene ni la activa
/// (su credencial viva es más nueva que la copia de cswap). Devuelve cuántas trajo.
pub fn import_cswap() -> Result<usize, String> {
    let dir = home().join(".claude-swap-backup").join("configs");
    let Ok(entries) = std::fs::read_dir(&dir) else { return Ok(0) };
    let have: Vec<String> = list().into_iter().map(|a| a.key).collect();
    let live = current().map(|c| c.key);
    let mut added = 0;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        // .claude-config-<n>-<correo>.json
        let Some(rest) = name.strip_prefix(".claude-config-").and_then(|r| r.strip_suffix(".json")) else { continue };
        let Some((num, email)) = rest.split_once('-') else { continue };
        let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
        let oauth = if v.get("oauthAccount").is_some() { v["oauthAccount"].clone() } else { v };
        let Some(id) = identity_of(&oauth) else { continue };
        if have.contains(&id.key) || live.as_deref() == Some(id.key.as_str()) {
            continue;
        }
        let Ok(Some(creds)) = kc_get("claude-swap", &format!("account-{num}-{email}")) else { continue };
        if store(&id, &creds, &oauth).is_ok() {
            added += 1;
        }
    }
    Ok(added)
}

/// Las copias guardadas se tocan de a una (renovar un token y cambiar de cuenta a la vez
/// podrían guardar una generación vieja encima de la nueva).
static STORE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Uso de cada cuenta: la activa (con su credencial viva, sin renovarla: es de Claude
/// Code) y, con `others`, las guardadas (si su token venció, se renueva y se guarda el nuevo).
pub fn usage_all(others: bool) -> Vec<(String, Result<crate::claude_usage::Usage, String>)> {
    use crate::claude_usage as cu;
    let mut out = Vec::new();
    let live = current();
    if let Some(id) = &live {
        let result = match kc_get(LIVE_SERVICE, &keychain_user()) {
            Ok(Some(creds)) => match cu::access_token(&creds) {
                Some((_, true)) => Err("el token venció; Claude Code lo renueva al usarlo".into()),
                Some((token, false)) => cu::fetch(&token),
                None => Err("la credencial no tiene token".into()),
            },
            Ok(None) => Err("Claude Code no tiene credencial".into()),
            Err(e) => Err(e),
        };
        out.push((id.key.clone(), result));
    }
    for a in list().into_iter().filter(|_| others) {
        if live.as_ref().is_some_and(|l| l.key == a.key) {
            continue;
        }
        let result = (|| {
            let token = {
                let _guard = STORE.lock().unwrap_or_else(|e| e.into_inner());
                let (creds, oauth) = stored(&a.key)?;
                match cu::access_token(&creds) {
                    Some((token, false)) => token,
                    Some((_, true)) => {
                        let fresh = cu::refresh(&creds)?;
                        let id = identity_of(&oauth).ok_or("la copia no tiene correo")?;
                        store(&id, &fresh, &oauth)?;
                        cu::access_token(&fresh).map(|(t, _)| t).ok_or("la renovación no trajo token")?
                    }
                    None => return Err("la copia no tiene token".to_string()),
                }
            };
            cu::fetch(&token)
        })();
        out.push((a.key.clone(), result));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clave_por_correo_y_organizacion() {
        assert_eq!(key_of("Persona@Ejemplo.com", Some("org-1")), "persona@ejemplo.com|org-1");
        assert_eq!(key_of("a@b.co", None), "a@b.co|");
        let oauth = json!({ "emailAddress": "a@b.co", "organizationUuid": "o", "organizationName": "Equipo" });
        let id = identity_of(&oauth).unwrap();
        assert_eq!((id.key.as_str(), id.org.as_deref()), ("a@b.co|o", Some("Equipo")));
        assert!(identity_of(&json!({ "emailAddress": "" })).is_none());
    }

    #[test]
    fn plan_de_la_credencial() {
        assert_eq!(plan_of(r#"{"claudeAiOauth":{"subscriptionType":"max"}}"#).as_deref(), Some("Max"));
        assert_eq!(plan_of("no es json"), None);
    }

    #[test]
    fn comillas_para_security() {
        assert_eq!(quote("Claude Code-credentials"), "\"Claude Code-credentials\"");
        assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    /// Ida y vuelta en el Llavero real, con un ítem de prueba que se borra al final:
    /// `cargo test llavero -- --ignored`.
    #[test]
    #[ignore]
    fn llavero_ida_y_vuelta() {
        let (svc, acct) = ("Telar · prueba", "prueba@telar");
        let value = r#"{"claudeAiOauth":{"accessToken":"x\"y","refreshToken":"z","subscriptionType":"max"}}"#;
        kc_set(svc, acct, value).unwrap();
        assert_eq!(kc_get(svc, acct).unwrap().as_deref(), Some(value));
        kc_set(svc, acct, "otro").unwrap();
        assert_eq!(kc_get(svc, acct).unwrap().as_deref(), Some("otro"), "-U actualiza");
        kc_delete(svc, acct).unwrap();
        assert_eq!(kc_get(svc, acct).unwrap(), None);
        kc_delete(svc, acct).unwrap();
    }

    /// Diagnóstico de solo lectura (no muestra secretos): `cargo test diagnostico -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn diagnostico_de_cuentas() {
        println!("activa: {:?}", current().map(|c| c.email));
        println!("credencial activa en el Llavero: {}", kc_get(LIVE_SERVICE, &keychain_user()).map(|c| c.is_some()).unwrap_or(false));
        let dir = home().join(".claude-swap-backup").join("configs");
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(rest) = name.strip_prefix(".claude-config-").and_then(|r| r.strip_suffix(".json")) else { continue };
            let Some((num, email)) = rest.split_once('-') else { continue };
            let creds = kc_get("claude-swap", &format!("account-{num}-{email}"));
            println!("cswap {num} {email}: credencial {:?}", creds.map(|c| c.map(|c| plan_of(&c))));
        }
        // Solo la cuenta activa (no renueva nada): uso real.
        if let Ok(Some(creds)) = kc_get(LIVE_SERVICE, &keychain_user()) {
            match crate::claude_usage::access_token(&creds) {
                Some((token, false)) => println!("uso de la activa: {:?}", crate::claude_usage::fetch(&token)),
                other => println!("token de la activa vencido o ausente: {:?}", other.map(|(_, e)| e)),
            }
        }
    }

    #[test]
    fn bloqueo_de_directorio() {
        let dir = std::env::temp_dir().join(format!("telar-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir(&dir);
        let held = lock(&dir, Duration::from_secs(60), Duration::from_millis(300)).unwrap();
        assert!(dir.is_dir());
        assert!(lock(&dir, Duration::from_secs(60), Duration::from_millis(300)).is_err(), "tomado por otro: espera y desiste");
        drop(held);
        assert!(!dir.exists(), "al soltarlo se borra");
        // Uno viejo (de un proceso que murió) se quita.
        std::fs::create_dir(&dir).unwrap();
        let held = lock(&dir, Duration::ZERO, Duration::from_millis(300)).unwrap();
        drop(held);
    }
}
