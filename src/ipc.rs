//! Socket Unix de Telar. Por aquí hablan con la app los hooks de Claude Code
//! (`telar hook`) y la CLI (`telar tabs`, …).
//!
//! Protocolo: una línea JSON de petición y una línea JSON de respuesta por conexión.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};
use winit::event_loop::EventLoopProxy;

use crate::UserEvent;

pub struct Request {
    pub body: Value,
    pub reply: mpsc::Sender<Value>,
}

/// Mientras vive, el socket existe; al soltarlo se borra el archivo.
pub struct Server {
    pub path: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub fn serve(proxy: EventLoopProxy<UserEvent>) -> std::io::Result<Server> {
    let dir = std::env::var_os("TMPDIR").map(PathBuf::from).unwrap_or_else(|| "/tmp".into());
    let path = dir.join(format!("telar-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    std::thread::Builder::new().name("telar-ipc".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let proxy = proxy.clone();
            std::thread::spawn(move || handle(stream, proxy));
        }
    })?;
    Ok(Server { path })
}

fn handle(stream: UnixStream, proxy: EventLoopProxy<UserEvent>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }
    let body = match serde_json::from_str::<Value>(&line) {
        Ok(v) => v,
        Err(e) => return reply(&stream, json!({ "ok": false, "error": e.to_string() })),
    };
    let (tx, rx) = mpsc::channel();
    if proxy.send_event(UserEvent::Ipc(Request { body, reply: tx })).is_err() {
        return;
    }
    let response = rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap_or_else(|_| json!({ "ok": false, "error": "la app no respondió" }));
    reply(&stream, response);
}

fn reply(mut stream: &UnixStream, value: Value) {
    let _ = writeln!(stream, "{value}");
}

/// Cliente: envía una petición al Telar indicado por `TELAR_SOCKET`.
/// Con `wait = false` no espera respuesta (lo usan los hooks para no frenar a Claude).
pub fn request(body: &Value, wait: bool) -> Result<Value, String> {
    let path = std::env::var_os("TELAR_SOCKET").ok_or("TELAR_SOCKET no está definido: esto solo funciona dentro de Telar")?;
    request_to(Path::new(&path), body, wait)
}

pub fn request_to(path: &Path, body: &Value, wait: bool) -> Result<Value, String> {
    let mut stream = UnixStream::connect(path).map_err(|e| format!("no se pudo conectar a {}: {e}", path.display()))?;
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    writeln!(stream, "{body}").map_err(|e| e.to_string())?;
    if !wait {
        return Ok(Value::Null);
    }
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).map_err(|e| e.to_string())?;
    serde_json::from_str(&line).map_err(|e| e.to_string())
}

/// `telar hook`: Claude Code lo ejecuta en cada evento, con el JSON del evento por stdin.
/// Nunca falla ni bloquea: si algo sale mal, simplemente no avisa.
pub fn hook_main() {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let Some(tab) = std::env::var("TELAR_TAB_ID").ok().and_then(|t| t.parse::<u64>().ok()) else { return };
    let payload = serde_json::from_str::<Value>(&input).unwrap_or(Value::Null);
    let _ = request(&json!({ "cmd": "hook", "tab": tab, "payload": payload }), false);
}
