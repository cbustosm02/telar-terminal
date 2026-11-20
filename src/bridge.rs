//! `telar bridge`: el puente de Telar remoto. Corre aparte de la app (la app lo lanza y
//! lo cierra), abre una conexión WebSocket **saliente** al relevo de Telar web y la
//! conecta con el Telar local:
//!
//! - del Telar local recibe el estado y las pantallas (`remote-subscribe`, ver `remote`);
//! - del relevo, los pedidos de los navegadores (`{"client","msg"}`), que reparte entre
//!   el socket de Telar (attach, input, answer…) y las respuestas a ese navegador.
//!
//! El Mac nunca acepta conexiones: solo sale. La sesión se obtiene iniciando sesión en
//! Telar web desde el navegador (flujo de dispositivo: `login_start` y `login_poll`) y se
//! guarda en el Llavero, servicio «Telar remoto». Cada usuario vincula su propio Mac.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::ipc;

pub const PROTO: u64 = 1;
/// Servidor de Telar web por defecto (se cambia con `telar remote login --server URL`).
pub const DEFAULT_SERVER: &str = "https://telar.cesarbustos.dev";
const KEYCHAIN_SERVICE: &str = "Telar remoto";
const KEYCHAIN_ACCOUNT: &str = "sesion";
const PING_EVERY: Duration = Duration::from_secs(20);
/// Tope de lo que se acepta de un navegador.
const MAX_INPUT: usize = 16 * 1024;
const MAX_SCROLLBACK: u64 = 5000;

// ── Sesión ──────────────────────────────────────────────────────────────────────

/// Este Mac vinculado a una cuenta de Telar web.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    /// `https://…` del servidor (para cerrar sesión y abrir la web).
    pub server: String,
    /// `wss://…/bridge`.
    pub bridge_url: String,
    pub token: String,
    /// Service token de Cloudflare Access de este dispositivo (si el servidor está detrás
    /// de Access): va en /bridge y al cerrar sesión.
    pub access: Option<(String, String)>,
    pub device_id: Option<String>,
    pub email: Option<String>,
    pub name: Option<String>,
}

fn local(url: &str) -> bool {
    ["://127.0.0.1", "://localhost"].iter().any(|h| url.contains(h))
}

impl Session {
    /// De la respuesta `ok` de `/api/devices/poll` (o de lo guardado en el Llavero).
    pub fn from_json(v: &Value, server: &str) -> Result<Self, String> {
        let s = |k: &str| v[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
        let server = s("server").unwrap_or_else(|| server.trim_end_matches('/').to_string());
        let bridge_url = s("bridge_url").ok_or("el servidor no mandó bridge_url")?;
        // Cifrado siempre, salvo a este mismo Mac (pruebas con el relevo local).
        if !(bridge_url.starts_with("wss://") || (bridge_url.starts_with("ws://") && local(&bridge_url))) {
            return Err("bridge_url debe ser wss://".into());
        }
        let user = |k: &str| v["user"][k].as_str().or_else(|| v[k].as_str()).map(String::from);
        let access = match (v["access"]["client_id"].as_str(), v["access"]["client_secret"].as_str()) {
            (Some(id), Some(secret)) if !id.is_empty() && !secret.is_empty() => Some((id.to_string(), secret.to_string())),
            _ => None,
        };
        Ok(Self {
            server,
            bridge_url,
            token: s("token").ok_or("el servidor no mandó token")?,
            access,
            device_id: s("device_id"),
            email: user("email"),
            name: user("name"),
        })
    }

    fn to_json(&self) -> String {
        let access = self.access.as_ref().map(|(id, secret)| json!({ "client_id": id, "client_secret": secret }));
        json!({ "server": self.server, "bridge_url": self.bridge_url, "token": self.token, "access": access, "device_id": self.device_id, "email": self.email, "name": self.name, "user": { "email": self.email, "name": self.name } }).to_string()
    }

    /// Del Llavero; para pruebas, de `TELAR_REMOTE_CODE` (el mismo JSON).
    pub fn load() -> Option<Self> {
        if let Some(code) = std::env::var("TELAR_REMOTE_CODE").ok().filter(|c| !c.is_empty()) {
            let v: Value = serde_json::from_str(&code).ok()?;
            return Self::from_json(&v, "").ok();
        }
        let text = crate::cuentas::kc_get(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).ok()??;
        let v: Value = serde_json::from_str(&text).ok()?;
        Self::from_json(&v, "").ok()
    }

    pub fn save(&self) -> Result<(), String> {
        crate::cuentas::kc_set(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, &self.to_json())
    }

    /// Borra la sesión del Llavero (este Mac deja de estar vinculado).
    pub fn forget() -> Result<(), String> {
        crate::cuentas::kc_delete(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
    }

    /// Le avisa al servidor que este dispositivo cerró sesión (si responde; si no, la web
    /// lo puede revocar a mano).
    pub fn revoke(&self) {
        let _ = post(&format!("{}/api/devices/revoke", self.server), &json!({}), Some(self));
    }

    /// Quién está conectado, para mostrar: el correo o, si no, el servidor.
    pub fn who(&self) -> String {
        self.email.clone().or_else(|| self.name.clone()).unwrap_or_else(|| host(&self.server))
    }
}

/// Solo el host de una URL.
pub fn host(url: &str) -> String {
    url.split("://").nth(1).unwrap_or(url).split('/').next().unwrap_or_default().to_string()
}

/// POST JSON con curl (como el actualizador: sin dependencias de HTTP). Con `auth`, las
/// credenciales del dispositivo van en cabeceras que curl lee de la fd 3, nunca en los
/// argumentos (que cualquiera ve con `ps`). Devuelve el código HTTP y el JSON.
fn post(url: &str, body: &Value, auth: Option<&Session>) -> Result<(u16, Value), String> {
    let mut cmd = Command::new("/usr/bin/curl");
    cmd.args(["-sS", "--max-time", "15", "-X", "POST", "-H", "content-type: application/json", "-H"])
        .arg(format!("user-agent: telar/{}", crate::update::VERSION))
        .args(["--data-binary", "@-", "-w", "\n%{http_code}"]);
    let secrets = auth.map(|a| {
        let mut c = format!("header = \"authorization: Bearer {}\"\n", a.token);
        if let Some((id, secret)) = &a.access {
            c.push_str(&format!("header = \"cf-access-client-id: {id}\"\nheader = \"cf-access-client-secret: {secret}\"\n"));
        }
        c
    });
    if secrets.is_some() {
        cmd.args(["-K", "/dev/fd/3"]);
    }
    cmd.arg(url).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = spawn_with_fd3(cmd, secrets)?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(body.to_string().as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", &text));
    let code: u16 = code.trim().parse().unwrap_or(0);
    if code == 0 {
        return Err(format!("no se pudo conectar con el servidor: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok((code, serde_json::from_str(body).unwrap_or(Value::Null)))
}

/// Mensaje de error de una respuesta que no es 2xx.
fn http_error(code: u16, v: &Value) -> String {
    v["error"].as_str().or(v["message"].as_str()).map(String::from).unwrap_or_else(|| format!("el servidor respondió {code}"))
}

/// Lanza `cmd` con `extra` disponible en la fd 3 (para pasarle secretos a curl sin
/// ponerlos en los argumentos, que cualquiera ve con `ps`).
fn spawn_with_fd3(mut cmd: Command, extra: Option<String>) -> Result<std::process::Child, String> {
    use std::os::unix::process::CommandExt;
    let Some(extra) = extra else { return cmd.spawn().map_err(|e| e.to_string()) };
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err("no se pudo crear la tubería".into());
    }
    let (read, write) = (fds[0], fds[1]);
    unsafe {
        cmd.pre_exec(move || {
            if libc::dup2(read, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn();
    unsafe { libc::close(read) };
    // El config de curl es chico: entra entero en la tubería sin bloquear.
    let _ = unsafe { libc::write(write, extra.as_ptr() as *const _, extra.len()) };
    unsafe { libc::close(write) };
    child.map_err(|e| e.to_string())
}

/// Paso 1 del inicio de sesión: el servidor da un código y la página para autorizarlo.
pub struct LoginStart {
    pub device_code: String,
    /// Código corto (XXXX-XXXX) que la página muestra igual: para comprobar que se
    /// autoriza este Mac.
    pub user_code: Option<String>,
    pub verification_url: String,
    pub interval: Duration,
    pub expires: Instant,
}

pub fn login_start(server: &str) -> Result<LoginStart, String> {
    let server = server.trim_end_matches('/');
    if !(server.starts_with("https://") || (server.starts_with("http://") && local(server))) {
        return Err("el servidor debe ser https://".into());
    }
    let (code, v) = post(&format!("{server}/api/devices/start"), &json!({ "name": mac_name(), "version": crate::update::VERSION }), None)?;
    if code == 429 {
        return Err("demasiados intentos: espera unos minutos".into());
    }
    if !(200..300).contains(&code) {
        return Err(http_error(code, &v));
    }
    let device_code = v["device_code"].as_str().ok_or("el servidor no mandó device_code")?.to_string();
    let url = v["verification_url"].as_str().ok_or("el servidor no mandó verification_url")?.to_string();
    if !(url.starts_with("https://") || (url.starts_with("http://") && local(&url))) {
        return Err("la página de autorización no es https://".into());
    }
    Ok(LoginStart {
        device_code,
        user_code: v["user_code"].as_str().map(String::from),
        verification_url: url,
        interval: Duration::from_secs(v["interval"].as_u64().unwrap_or(2).clamp(1, 30)),
        expires: Instant::now() + Duration::from_secs(v["expires_in"].as_u64().unwrap_or(600).clamp(30, 3600)),
    })
}

pub enum Poll {
    Pending,
    /// Se preguntó muy seguido: esperar 2 s más entre preguntas.
    SlowDown,
    Done(Session),
    Failed(String),
}

/// Paso 2: ¿ya lo autorizó el usuario en el navegador?
pub fn login_poll(server: &str, device_code: &str) -> Poll {
    let server = server.trim_end_matches('/');
    match post(&format!("{server}/api/devices/poll"), &json!({ "device_code": device_code }), None) {
        Ok((code, v)) if !(200..300).contains(&code) && code != 429 => Poll::Failed(http_error(code, &v)),
        Ok((_, v)) => match v["status"].as_str() {
            Some("pending") => Poll::Pending,
            Some("slow_down") => Poll::SlowDown,
            Some("ok") => match Session::from_json(&v, server) {
                Ok(s) => Poll::Done(s),
                Err(e) => Poll::Failed(e),
            },
            Some("denied") => Poll::Failed("rechazaste la conexión en el navegador".into()),
            Some("expired") => Poll::Failed("el código venció: vuelve a iniciar sesión".into()),
            _ => Poll::Failed("respuesta inesperada del servidor".into()),
        },
        Err(e) => Poll::Failed(e),
    }
}

/// Respuesta a una acción: `done` o el `error` de Telar (con su `code`, p. ej. `busy`).
fn done_or_error(action: &str, msg: &Value, r: &Value) -> Vec<Value> {
    if r["ok"] == json!(false) {
        vec![json!({ "type": "error", "action": action, "code": r["code"], "message": r["error"], "tab": msg["tab"], "group": msg["group"] })]
    } else {
        vec![json!({ "type": "done", "action": action, "tab": msg["tab"], "group": msg["group"] })]
    }
}

/// Imágenes que se aceptan desde la web (las que Claude lee, más HEIC, que se convierte).
const MAX_IMAGE: usize = 15 * 1024 * 1024;

fn image_ext(mime: &str, name: &str) -> Option<&'static str> {
    let by_name = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match (mime, by_name.as_str()) {
        ("image/png", _) | (_, "png") => Some("png"),
        ("image/jpeg" | "image/jpg", _) | (_, "jpg" | "jpeg") => Some("jpg"),
        ("image/gif", _) | (_, "gif") => Some("gif"),
        ("image/webp", _) | (_, "webp") => Some("webp"),
        ("image/heic" | "image/heif", _) | (_, "heic" | "heif") => Some("heic"),
        _ => None,
    }
}

/// Carpeta donde quedan las imágenes que llegan de la web (se borran al día siguiente).
fn paste_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("Library/Caches/Telar/pegado")
}

/// Guarda la imagen de `{"name","mime","data":"<base64>"}` y devuelve su ruta.
fn save_image(msg: &Value) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    let data = msg["data"].as_str().ok_or("falta la imagen")?;
    // Puede venir como data URL.
    let data = data.split_once(";base64,").map_or(data, |(_, d)| d);
    let mime = msg["mime"].as_str().unwrap_or_default();
    let ext = image_ext(mime, msg["name"].as_str().unwrap_or_default()).ok_or("solo se pueden pegar imágenes PNG, JPEG, GIF, WebP o HEIC")?;
    if data.len() / 4 * 3 > MAX_IMAGE {
        return Err(format!("la imagen pasa de {} MB", MAX_IMAGE / 1024 / 1024));
    }
    let bytes = base64_decode(data)?;
    let dir = paste_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    // Limpieza: lo de más de un día.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > Duration::from_secs(24 * 3600));
            if old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let stem = format!("telar-web-{}-{:09}", t.as_secs(), t.subsec_nanos());
    let path = dir.join(format!("{stem}.{ext}"));
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    if ext != "heic" {
        return Ok(path);
    }
    // Claude no lee HEIC (las fotos del iPhone): se pasa a JPEG con `sips`.
    let jpg = dir.join(format!("{stem}.jpg"));
    let ok = Command::new("/usr/bin/sips").args(["-s", "format", "jpeg"]).arg(&path).arg("--out").arg(&jpg).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    let _ = std::fs::remove_file(&path);
    if !ok {
        return Err("no se pudo convertir la foto HEIC".into());
    }
    let _ = std::fs::set_permissions(&jpg, std::fs::Permissions::from_mode(0o600));
    Ok(jpg)
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let clean: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace() && *b != b'=').collect();
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    for chunk in clean.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= val(*c).ok_or("la imagen no viene en base64")? << (18 - 6 * i);
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..chunk.len().saturating_sub(1)]);
    }
    Ok(out)
}

/// Nombre del Mac (el de Compartir), para la lista de Macs del relevo.
fn mac_name() -> String {
    std::process::Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Mac".into())
}

// ── Puente ──────────────────────────────────────────────────────────────────────

enum Event {
    /// Línea del Telar local (estado, pantalla, keepalive).
    Telar(String),
    /// Se cerró Telar: el puente termina.
    TelarGone,
}

/// Quién mira qué: cliente → (tab, columnas, filas).
#[derive(Default)]
struct Clients {
    watching: HashMap<String, (u64, usize, usize)>,
}

impl Clients {
    /// Tamaño para un tab: el más chico de quienes lo miran (como tmux).
    fn size_for(&self, tab: u64) -> Option<(usize, usize)> {
        let sizes = self.watching.values().filter(|(t, ..)| *t == tab);
        sizes.fold(None, |acc, &(_, c, r)| Some(acc.map_or((c, r), |(ac, ar): (usize, usize)| (ac.min(c), ar.min(r)))))
    }

    fn watchers(&self, tab: u64) -> Vec<String> {
        self.watching.iter().filter(|(_, (t, ..))| *t == tab).map(|(c, _)| c.clone()).collect()
    }
}

struct Bridge {
    socket: PathBuf,
    clients: Clients,
    last_state: Option<Value>,
}

impl Bridge {
    fn telar(&self, body: Value) -> Value {
        ipc::request_to(&self.socket, &body, true).unwrap_or_else(|e| json!({ "ok": false, "error": e }))
    }

    fn status(&self, status: &str, detail: &str) {
        let _ = ipc::request_to(&self.socket, &json!({ "cmd": "remote-status", "status": status, "detail": detail }), false);
    }

    fn viewers(&self, list: &Value) {
        let _ = ipc::request_to(&self.socket, &json!({ "cmd": "remote-viewers", "list": list }), false);
    }

    /// Ajusta el tamaño de un tab a quienes lo miran; sin nadie, lo suelta.
    fn sync_tab(&self, tab: u64) -> Value {
        match self.clients.size_for(tab) {
            Some((cols, rows)) => self.telar(json!({ "cmd": "remote-attach", "tab": tab, "cols": cols, "rows": rows })),
            None => self.telar(json!({ "cmd": "remote-release", "tab": tab })),
        }
    }

    fn release_all(&mut self) {
        let tabs: Vec<u64> = self.clients.watching.values().map(|(t, ..)| *t).collect();
        self.clients.watching.clear();
        for tab in tabs {
            self.sync_tab(tab);
        }
    }

    /// Un pedido de un navegador. Devuelve los mensajes para ese navegador.
    fn on_client(&mut self, client: &str, msg: &Value) -> Vec<Value> {
        let tab = msg["tab"].as_u64();
        let err = |m: String| vec![json!({ "type": "error", "message": m, "tab": tab })];
        match msg["type"].as_str().unwrap_or_default() {
            "hello" => self.last_state.clone().into_iter().collect(),
            "attach" | "resize" => {
                let (Some(tab), Some(cols), Some(rows)) = (tab, msg["cols"].as_u64(), msg["rows"].as_u64()) else {
                    return err("faltan tab, cols o rows".into());
                };
                let before = self.clients.watching.insert(client.to_string(), (tab, cols as usize, rows as usize));
                if let Some((old, ..)) = before.filter(|(old, ..)| *old != tab) {
                    self.sync_tab(old);
                }
                let r = self.sync_tab(tab);
                if r["ok"] == json!(false) {
                    self.clients.watching.remove(client);
                    return err(r["error"].as_str().unwrap_or("no se pudo abrir el tab").into());
                }
                Vec::new()
            }
            "detach" => {
                if let Some((t, ..)) = self.clients.watching.remove(client) {
                    self.sync_tab(t);
                }
                Vec::new()
            }
            "input" => {
                let (Some(tab), Some(data)) = (tab, msg["data"].as_str()) else { return err("faltan tab o data".into()) };
                if data.len() > MAX_INPUT {
                    return err(format!("el texto pasa de {} KB", MAX_INPUT / 1024));
                }
                let r = self.telar(json!({ "cmd": "send", "tab": tab, "text": data, "raw": true }));
                if r["ok"] == json!(false) { err(r["error"].as_str().unwrap_or("no se pudo escribir").into()) } else { Vec::new() }
            }
            "answer" => {
                let Some(tab) = tab else { return err("falta tab".into()) };
                let r = self.telar(json!({ "cmd": "remote-answer", "tab": tab, "question_id": msg["question_id"], "digit": msg["digit"] }));
                if r["ok"] == json!(false) {
                    vec![json!({ "type": "error", "code": r["code"], "message": r["error"], "tab": tab })]
                } else {
                    Vec::new()
                }
            }
            // Suspender o reanudar un tab o un grupo. Con Claude trabajando o esperando
            // respuesta, suspender pide `force` (la web lo confirma antes; error `busy`).
            "suspend" | "resume" => {
                let kind = msg["type"].as_str().unwrap_or_default();
                let r = self.telar(json!({ "cmd": kind, "tab": msg["tab"], "group": msg["group"], "force": msg["force"].as_bool().unwrap_or(false) }));
                done_or_error(kind, msg, &r)
            }
            // Cerrar un tab o eliminar un grupo (la web pide confirmación antes).
            "close-tab" => {
                let r = self.telar(json!({ "cmd": "remote-close", "tab": msg["tab"] }));
                done_or_error("close-tab", msg, &r)
            }
            "delete-group" => {
                let r = self.telar(json!({ "cmd": "remote-delete-group", "group": msg["group"] }));
                done_or_error("delete-group", msg, &r)
            }
            // Imagen pegada en la web: se guarda aquí y Telar pega su ruta (Claude la adjunta).
            "paste-image" => {
                let Some(tab) = tab else { return err("falta tab".into()) };
                let path = match save_image(msg) {
                    Ok(p) => p,
                    Err(e) => return err(e),
                };
                let r = self.telar(json!({ "cmd": "remote-paste-file", "tab": tab, "path": path }));
                done_or_error("paste-image", msg, &r)
            }
            // Tab nuevo en un grupo (o grupo nuevo con su primer tab): shell o agente.
            "new-tab" | "new-group" => {
                let r = self.telar(json!({
                    "cmd": "remote-new",
                    "group": msg["group"],
                    "name": msg["name"],
                    "new_group": msg["type"] == "new-group",
                    "kind": msg["kind"],
                }));
                if r["ok"] == json!(false) {
                    return vec![json!({ "type": "error", "message": r["error"] })];
                }
                vec![json!({ "type": "tab-created", "tab": r["id"], "group": r["group"] })]
            }
            "scrollback" => {
                let Some(tab) = tab else { return err("falta tab".into()) };
                let lines = msg["lines"].as_u64().unwrap_or(500).clamp(1, MAX_SCROLLBACK);
                let r = self.telar(json!({ "cmd": "read", "tab": tab, "lines": lines }));
                if r["ok"] == json!(false) {
                    return err(r["error"].as_str().unwrap_or("no se pudo leer").into());
                }
                vec![json!({ "type": "scrollback", "tab": tab, "text": r["text"] })]
            }
            other => err(format!("tipo desconocido: {other}")),
        }
    }

    /// Un navegador se fue: suelta lo que miraba.
    fn on_closed(&mut self, client: &str) {
        if let Some((t, ..)) = self.clients.watching.remove(client) {
            self.sync_tab(t);
        }
    }

    /// Una línea del Telar local: a quién va. `None` = a nadie.
    fn route(&mut self, line: &str) -> Vec<Value> {
        let Ok(msg) = serde_json::from_str::<Value>(line) else { return Vec::new() };
        match msg["type"].as_str() {
            Some("state") => {
                self.last_state = Some(msg.clone());
                vec![json!({ "broadcast": true, "msg": msg })]
            }
            Some("screen") => {
                let Some(tab) = msg["tab"].as_u64() else { return Vec::new() };
                self.clients.watchers(tab).into_iter().map(|c| json!({ "client": c, "msg": msg })).collect()
            }
            _ => Vec::new(),
        }
    }
}

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

/// Por qué no se pudo conectar.
enum ConnectError {
    /// La sesión ya no vale (revocada desde la web): no se reintenta.
    Revoked,
    Other(String),
}

fn connect(session: &Session) -> Result<Ws, ConnectError> {
    let other = |e: String| ConnectError::Other(e);
    let mut req = session.bridge_url.as_str().into_client_request().map_err(|e| other(format!("URL inválida: {e}")))?;
    let h = req.headers_mut();
    let header = |v: &str| v.parse().map_err(|_| other("credencial con caracteres no válidos".to_string()));
    h.insert("authorization", header(&format!("Bearer {}", session.token))?);
    if let Some((id, secret)) = &session.access {
        h.insert("cf-access-client-id", header(id)?);
        h.insert("cf-access-client-secret", header(secret)?);
    }
    h.insert("user-agent", header(&format!("telar-bridge/{}", crate::update::VERSION))?);
    let (ws, resp) = tungstenite::connect(req).map_err(|e| match e {
        tungstenite::Error::Http(r) => {
            let status = r.status().as_u16();
            let body = r.body().as_deref().map(String::from_utf8_lossy).unwrap_or_default().to_string();
            let v: Value = serde_json::from_str(&body).unwrap_or_default();
            // Revocada solo si la app lo dice. Un 401/403 de Cloudflare (p. ej. un service
            // token recién creado que todavía no se propagó) se reintenta.
            if v["code"] == "revoked" {
                return ConnectError::Revoked;
            }
            let why = v["message"].as_str().or(v["error"].as_str()).map(String::from).unwrap_or_else(|| {
                let text = body.split_whitespace().collect::<Vec<_>>().join(" ");
                if text.contains("cloudflare") || text.contains("Access") { "Cloudflare Access no lo dejó pasar".into() } else { text.chars().take(120).collect() }
            });
            log::warn!("telar remoto: /bridge respondió {status}: {why}");
            other(format!("el servidor respondió {status}{}", if why.is_empty() { String::new() } else { format!(": {why}") }))
        }
        e => other(e.to_string()),
    })?;
    log::info!("telar remoto: conectado ({})", resp.status());
    // Lecturas con tiempo límite: el mismo hilo atiende al relevo y a Telar.
    let tcp = match ws.get_ref() {
        MaybeTlsStream::Plain(s) => s,
        MaybeTlsStream::NativeTls(s) => s.get_ref(),
        _ => return Err(other("tipo de conexión inesperado".into())),
    };
    // Sin bloqueo: el hilo duerme en `poll` hasta que llega algo (ver `serve`).
    tcp.set_nonblocking(true).map_err(|e| other(e.to_string()))?;
    let _ = tcp.set_nodelay(true);
    Ok(ws)
}

fn would_block(e: &tungstenite::Error) -> bool {
    matches!(e, tungstenite::Error::Io(io) if matches!(io.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
}

/// Encola y envía; si el socket está lleno queda en el búfer de tungstenite y sale con
/// el próximo `flush` (cuando `poll` dice que se puede escribir).
fn send(ws: &mut Ws, value: &Value) -> Result<(), String> {
    match ws.send(Message::text(value.to_string())) {
        Ok(()) => Ok(()),
        Err(e) if would_block(&e) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn raw_fd(ws: &Ws) -> i32 {
    use std::os::fd::AsRawFd;
    match ws.get_ref() {
        MaybeTlsStream::NativeTls(s) => s.get_ref().as_raw_fd(),
        MaybeTlsStream::Plain(s) => s.as_raw_fd(),
        _ => -1,
    }
}

/// Tubería para despertar al hilo del puente cuando Telar manda algo.
struct Wake {
    read: i32,
    write: i32,
}

impl Wake {
    fn new() -> Result<Self, String> {
        let mut fds = [0i32; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err("no se pudo crear la tubería".into());
        }
        for fd in fds {
            unsafe { libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK) };
        }
        Ok(Self { read: fds[0], write: fds[1] })
    }

    fn ring(fd: i32) {
        let _ = unsafe { libc::write(fd, [1u8].as_ptr() as *const _, 1) };
    }

    fn drain(&self) {
        let mut buf = [0u8; 64];
        while unsafe { libc::read(self.read, buf.as_mut_ptr() as *mut _, buf.len()) } > 0 {}
    }
}

/// Suscripción al Telar local: sus líneas llegan como eventos (y despiertan al puente).
fn subscribe(socket: &Path, tx: mpsc::Sender<Event>, wake: i32) -> Result<(), String> {
    let mut stream = UnixStream::connect(socket).map_err(|e| format!("no se pudo conectar a Telar: {e}"))?;
    writeln!(stream, "{}", json!({ "cmd": "remote-subscribe" })).map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            if tx.send(Event::Telar(line)).is_err() {
                return;
            }
            Wake::ring(wake);
        }
        let _ = tx.send(Event::TelarGone);
        Wake::ring(wake);
    });
    Ok(())
}

/// Espera antes de reconectar: 1 s, 2 s, 4 s… hasta 30 s, con algo de azar.
fn backoff(attempt: u32) -> Duration {
    let base = (1u64 << attempt.min(5)).min(30);
    let jitter = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_millis()) % 1000) as u64;
    Duration::from_millis(base * 1000 / 2 + jitter * base / 2)
}

/// `telar bridge`. Termina cuando se cierra Telar.
pub fn run() -> Result<String, String> {
    let socket = std::env::var_os("TELAR_SOCKET").map(PathBuf::from).ok_or("TELAR_SOCKET no está definido")?;
    let session = Session::load().ok_or("no hay sesión de Telar web: inicia sesión desde Telar")?;
    let (tx, rx) = mpsc::channel();
    let wake = Wake::new()?;
    subscribe(&socket, tx, wake.write)?;
    let mut bridge = Bridge { socket, clients: Clients::default(), last_state: None };
    let name = mac_name();
    let mut attempt = 0u32;
    loop {
        bridge.status("connecting", "");
        let mut ws = match connect(&session) {
            Ok(ws) => ws,
            Err(ConnectError::Revoked) => {
                bridge.status("revoked", "la sesión se cerró desde Telar web");
                return Err("sesión revocada".into());
            }
            Err(ConnectError::Other(e)) => {
                log::warn!("telar remoto: {e}");
                // Recién vinculado, Cloudflare tarda unos segundos en activar el service token
                // del Mac (responde 403): no es un error todavía.
                if e.contains("403") && attempt < 8 {
                    bridge.status("connecting", "Cloudflare está activando el acceso…");
                } else {
                    bridge.status("error", &e);
                }
                let wait = backoff(attempt);
                attempt += 1;
                // Mientras espera, igual vacía lo que manda Telar (y nota si se cerró).
                let until = Instant::now() + wait;
                while let Some(left) = until.checked_duration_since(Instant::now()) {
                    match rx.recv_timeout(left) {
                        Ok(Event::Telar(line)) => {
                            bridge.route(&line);
                        }
                        Ok(Event::TelarGone) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(String::new()),
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                    }
                }
                continue;
            }
        };
        let hello = json!({ "type": "bridge-hello", "proto": PROTO, "name": name, "version": crate::update::VERSION });
        if send(&mut ws, &hello).is_err() {
            continue;
        }
        if let Some(state) = bridge.last_state.clone() {
            let _ = send(&mut ws, &json!({ "broadcast": true, "msg": state }));
        }
        attempt = 0;
        bridge.status("connected", "");
        let reason = serve(&mut ws, &mut bridge, &rx, &wake);
        // Lo que miraban los navegadores vuelve a su tamaño en el Mac, y ya nadie mira.
        bridge.release_all();
        bridge.viewers(&json!([]));
        match reason {
            Exit::TelarGone => {
                let _ = ws.close(None);
                return Ok(String::new());
            }
            Exit::Lost(e) => {
                log::warn!("telar remoto: se cortó la conexión: {e}");
                bridge.status("error", &format!("se cortó: {e}"));
                std::thread::sleep(backoff(attempt));
                attempt += 1;
            }
            Exit::Rejected(e) => {
                bridge.status("revoked", &e);
                return Err(e);
            }
        }
    }
}

enum Exit {
    TelarGone,
    Lost(String),
    /// El relevo rechazó al bridge (versión, credenciales): no tiene sentido reintentar.
    Rejected(String),
}

fn serve(ws: &mut Ws, bridge: &mut Bridge, rx: &mpsc::Receiver<Event>, wake: &Wake) -> Exit {
    let mut last_ping = Instant::now();
    let fd = raw_fd(ws);
    loop {
        // Del relevo: todo lo que haya (TLS puede tener más de un mensaje ya descifrado).
        loop {
            match ws.read() {
            Ok(Message::Text(text)) => {
                let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                if v["type"] == "error" && v.get("client").is_none() {
                    let msg = v["message"].as_str().unwrap_or("el relevo rechazó la conexión").to_string();
                    return if v["fatal"] == json!(true) { Exit::Rejected(msg) } else { Exit::Lost(msg) };
                }
                // Ping de la aplicación (además de los del WebSocket).
                if v["type"] == "ping" {
                    if let Err(e) = send(ws, &json!({ "type": "pong" })) {
                        return Exit::Lost(e);
                    }
                    continue;
                }
                // Quién mira este Mac desde el navegador: para el indicador de Telar.
                if v["type"] == "viewers" {
                    bridge.viewers(&v["list"]);
                    continue;
                }
                let Some(client) = v["client"].as_str().map(String::from) else { continue };
                if v["closed"] == json!(true) {
                    bridge.on_closed(&client);
                    continue;
                }
                for reply in bridge.on_client(&client, &v["msg"]) {
                    if let Err(e) = send(ws, &json!({ "client": client, "msg": reply })) {
                        return Exit::Lost(e);
                    }
                }
            }
            // 4000–4003: el relevo no quiere que se reconecte (sesión revocada, versión…).
            Ok(Message::Close(frame)) => {
                let code = frame.as_ref().map(|f| u16::from(f.code));
                let reason = frame.map(|f| f.reason.to_string()).filter(|r| !r.is_empty());
                return match code {
                    Some(4000..=4003) => Exit::Rejected(reason.unwrap_or_else(|| "el servidor cerró la sesión".into())),
                    _ => Exit::Lost(reason.unwrap_or_else(|| "el relevo cerró la conexión".into())),
                };
            }
            Ok(_) => {}
            Err(e) if would_block(&e) => break,
            Err(e) => return Exit::Lost(e.to_string()),
            }
        }
        // Del Telar local.
        wake.drain();
        loop {
            match rx.try_recv() {
                Ok(Event::Telar(line)) => {
                    for out in bridge.route(&line) {
                        if let Err(e) = send(ws, &out) {
                            return Exit::Lost(e);
                        }
                    }
                }
                Ok(Event::TelarGone) | Err(mpsc::TryRecvError::Disconnected) => return Exit::TelarGone,
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        if last_ping.elapsed() >= PING_EVERY {
            last_ping = Instant::now();
            if let Err(e) = ws.send(Message::Ping(Vec::new().into())) {
                if !would_block(&e) {
                    return Exit::Lost(e.to_string());
                }
            }
        }
        // Lo que quedó en el búfer de salida.
        let pending = match ws.flush() {
            Ok(()) => false,
            Err(e) if would_block(&e) => true,
            Err(e) => return Exit::Lost(e.to_string()),
        };
        // A dormir hasta que llegue algo, se pueda escribir lo pendiente o toque el ping.
        let timeout = PING_EVERY.saturating_sub(last_ping.elapsed()).as_millis().max(1) as i32;
        let mut fds = [
            libc::pollfd { fd, events: libc::POLLIN | if pending { libc::POLLOUT } else { 0 }, revents: 0 },
            libc::pollfd { fd: wake.read, events: libc::POLLIN, revents: 0 },
        ];
        unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sesion_del_servidor() {
        let ok = json!({"status":"ok","token":"tlr_x","device_id":"d1","user":{"email":"a@b.co","name":"A"},
            "bridge_url":"wss://telar.ejemplo.dev/bridge","access":{"client_id":"id.access","client_secret":"sec"}});
        let s = Session::from_json(&ok, "https://telar.ejemplo.dev/").unwrap();
        assert_eq!((s.server.as_str(), s.token.as_str(), s.who().as_str()), ("https://telar.ejemplo.dev", "tlr_x", "a@b.co"));
        assert_eq!(s.access, Some(("id.access".into(), "sec".into())));
        // Lo guardado en el Llavero vuelve igual.
        let back: Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(Session::from_json(&back, "").unwrap(), s);
        // Sin cifrar solo a este Mac; sin token, nada.
        assert!(Session::from_json(&json!({"token":"t","bridge_url":"ws://telar.ejemplo.dev/bridge"}), "").is_err());
        assert!(Session::from_json(&json!({"token":"t","bridge_url":"ws://127.0.0.1:8787/bridge"}), "").is_ok());
        assert!(Session::from_json(&json!({"bridge_url":"wss://x/bridge"}), "").is_err());
        // Sin Access (desarrollo): solo el Bearer.
        assert_eq!(Session::from_json(&json!({"token":"t","bridge_url":"wss://x/bridge"}), "").unwrap().access, None);
    }

    #[test]
    fn imagen_de_la_web() {
        assert_eq!(base64_decode("aG9sYQ==").unwrap(), b"hola");
        assert_eq!(base64_decode("aG9sYSE").unwrap(), b"hola!");
        assert_eq!(image_ext("image/png", "x"), Some("png"));
        assert_eq!(image_ext("", "Foto.HEIC"), Some("heic"));
        assert_eq!(image_ext("application/pdf", "a.pdf"), None);
        // Un PNG de 1×1 llega, se guarda y queda solo para el usuario.
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
        let path = save_image(&json!({"name":"a.png","mime":"image/png","data": format!("data:image/png;base64,{png}")})).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_file(&path);
        assert!(save_image(&json!({"name":"a.pdf","mime":"application/pdf","data":"aG9sYQ=="})).is_err());
    }

    #[test]
    fn tamano_del_mas_chico() {
        let mut c = Clients::default();
        c.watching.insert("a".into(), (3, 50, 30));
        c.watching.insert("b".into(), (3, 45, 40));
        c.watching.insert("c".into(), (4, 10, 10));
        assert_eq!(c.size_for(3), Some((45, 30)));
        assert_eq!(c.size_for(5), None);
        let mut w = c.watchers(3);
        w.sort();
        assert_eq!(w, ["a", "b"]);
    }

    #[test]
    fn espera_creciente() {
        assert!(backoff(0) <= Duration::from_secs(1));
        assert!(backoff(10) >= Duration::from_secs(15) && backoff(10) <= Duration::from_secs(30));
    }
}
