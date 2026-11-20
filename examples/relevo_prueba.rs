//! Relevo de prueba de Telar remoto, para probar `telar bridge` sin el homelab. Habla el
//! mismo protocolo que el relevo real (ver README, «Telar remoto»), sin autenticación:
//! solo escucha en 127.0.0.1.
//!
//!   cargo run --example relevo_prueba
//!   → bridge:   ws://127.0.0.1:8787/bridge
//!   → página:   http://127.0.0.1:8788 (lista de tabs, terminal con xterm.js, teclas)

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tungstenite::handshake::server::{Request, Response};
use tungstenite::{Message, WebSocket};

#[derive(Default)]
struct Relay {
    bridge: Option<Sender<String>>,
    clients: HashMap<String, Sender<String>>,
    /// Navegador → (dispositivo, tab que mira).
    viewers: HashMap<String, (String, Option<u64>)>,
    next: u64,
    /// Inicio de sesión de dispositivos: device_code → (user_code, autorizado).
    devices: HashMap<String, (String, bool)>,
    tokens: Vec<String>,
}

impl Relay {
    fn send_viewers(&self) {
        let list: Vec<Value> = self.viewers.iter().map(|(c, (d, t))| json!({"client": c, "device": d, "tab": t, "since": 0})).collect();
        if let Some(b) = &self.bridge {
            let _ = b.send(json!({"type":"viewers","list":list}).to_string());
        }
    }
}

type Shared = Arc<Mutex<Relay>>;

const PAGE: &str = include_str!("relevo_prueba.html");

fn main() {
    let relay: Shared = Arc::default();
    let r2 = relay.clone();
    std::thread::spawn(move || {
        let http = TcpListener::bind("127.0.0.1:8788").expect("puerto 8788");
        for s in http.incoming().flatten() {
            serve_http(s, &r2);
        }
    });
    let ws = TcpListener::bind("127.0.0.1:8787").expect("puerto 8787");
    eprintln!("relevo de prueba: bridge en ws://127.0.0.1:8787/bridge, página en http://127.0.0.1:8788");
    for stream in ws.incoming().flatten() {
        let relay = relay.clone();
        std::thread::spawn(move || handle(stream, relay));
    }
}

fn serve_http(mut s: TcpStream, relay: &Shared) {
    let mut buf = vec![0u8; 16384];
    let n = s.read(&mut buf).unwrap_or(0);
    let req = String::from_utf8_lossy(&buf[..n]).to_string();
    let line = req.lines().next().unwrap_or_default().to_string();
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
    let body = req.split("\r\n\r\n").nth(1).unwrap_or_default();
    let body: Value = serde_json::from_str(body).unwrap_or_default();
    let reply = |s: &mut TcpStream, ctype: &str, text: String| {
        let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
    };
    let mut r = relay.lock().unwrap();
    if path == "/api/devices/start" {
        r.next += 1;
        let device_code = format!("dev-{}-{}", r.next, std::process::id());
        let user_code = format!("AB{:02}-CD{:02}", r.next % 100, r.next % 100);
        r.devices.insert(device_code.clone(), (user_code.clone(), false));
        eprintln!("start: {} ({})", body["name"], user_code);
        let url = format!("http://127.0.0.1:8788/vincular?c={user_code}");
        reply(&mut s, "application/json", json!({"device_code": device_code, "user_code": user_code, "verification_url": url, "interval": 1, "expires_in": 600}).to_string());
    } else if path == "/api/devices/poll" {
        let code = body["device_code"].as_str().unwrap_or_default().to_string();
        let out = match r.devices.get(&code) {
            Some((_, true)) => {
                r.devices.remove(&code);
                let token = format!("tlr_prueba_{}", r.tokens.len() + 1);
                r.tokens.push(token.clone());
                json!({"status":"ok","token":token,"device_id":"mac-1","user":{"email":"cesar@ejemplo.dev","name":"César"},"bridge_url":"ws://127.0.0.1:8787/bridge","server":"http://127.0.0.1:8788"})
            }
            Some(_) => json!({"status":"pending"}),
            None => json!({"status":"expired"}),
        };
        reply(&mut s, "application/json", out.to_string());
    } else if path.starts_with("/vincular") {
        let code = path.split("c=").nth(1).unwrap_or_default().to_string();
        let page = format!("<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width'><body style='font:16px -apple-system;background:#1e1f29;color:#eee;padding:40px'><h2>¿Conectar este Mac a tu cuenta?</h2><p>Código: <b style='font-size:28px'>{code}</b></p><form method=post action='/aprobar?c={code}'><button style='font-size:18px;padding:10px 20px'>Autorizar</button></form>");
        reply(&mut s, "text/html; charset=utf-8", page);
    } else if path.starts_with("/aprobar") {
        let code = path.split("c=").nth(1).unwrap_or_default().to_string();
        for (uc, ok) in r.devices.values_mut() {
            if *uc == code {
                *ok = true;
            }
        }
        eprintln!("aprobado: {code}");
        reply(&mut s, "text/html; charset=utf-8", "<!doctype html><meta charset=utf-8><body style='font:16px -apple-system;background:#1e1f29;color:#eee;padding:40px'><h2>Listo: tu Mac quedó conectado.</h2>".into());
    } else if path == "/api/devices/revoke" {
        eprintln!("revoke");
        reply(&mut s, "application/json", "{}".into());
    } else {
        reply(&mut s, "text/html; charset=utf-8", PAGE.to_string());
    }
}

fn handle(stream: TcpStream, relay: Shared) {
    let mut path = String::new();
    let mut auth = String::new();
    let mut agent = String::new();
    let cb = |req: &Request, resp: Response| {
        path = req.uri().path().to_string();
        auth = req.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        agent = req.headers().get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("Navegador").to_string();
        Ok(resp)
    };
    let Ok(mut ws) = tungstenite::accept_hdr(stream, cb) else { return };
    let _ = ws.get_ref().set_read_timeout(Some(Duration::from_millis(20)));
    let (tx, rx) = channel::<String>();
    if path == "/bridge" {
        let token = auth.trim_start_matches("Bearer ").to_string();
        // Con TELAR_PRUEBA_ABIERTO=1 se acepta cualquier token (para la prueba sin login).
        if !relay.lock().unwrap().tokens.contains(&token) && std::env::var_os("TELAR_PRUEBA_ABIERTO").is_none() {
            eprintln!("bridge rechazado: token desconocido");
            let _ = ws.send(Message::text(json!({"type":"error","fatal":true,"code":"revoked","message":"Sesión cerrada"}).to_string()));
            let _ = ws.close(Some(tungstenite::protocol::CloseFrame { code: 4003.into(), reason: "revoked".into() }));
            let _ = ws.flush();
            return;
        }
        eprintln!("bridge conectado");
        relay.lock().unwrap().bridge = Some(tx);
        relay.lock().unwrap().send_viewers();
        pump(&mut ws, rx, |text| {
            let v: Value = serde_json::from_str(&text).unwrap_or_default();
            let r = relay.lock().unwrap();
            if v["type"] == "bridge-hello" {
                eprintln!("bridge-hello: {}", v);
                for c in r.clients.values() {
                    let _ = c.send(json!({"type":"bridges","list":[{"id":"mac","name":v["name"],"online":true}]}).to_string());
                }
            } else if v["broadcast"] == json!(true) {
                for c in r.clients.values() {
                    let _ = c.send(v["msg"].to_string());
                }
            } else if let Some(c) = v["client"].as_str().and_then(|c| r.clients.get(c)) {
                let _ = c.send(v["msg"].to_string());
            }
        });
        eprintln!("bridge desconectado");
        let mut r = relay.lock().unwrap();
        r.bridge = None;
        for c in r.clients.values() {
            let _ = c.send(json!({"type":"bridges","list":[{"id":"mac","online":false}]}).to_string());
        }
    } else {
        let id = {
            let mut r = relay.lock().unwrap();
            r.next += 1;
            let id = format!("c{}", r.next);
            r.clients.insert(id.clone(), tx);
            let device = if agent.contains("iPhone") { "iPhone · Safari" } else if agent.contains("Mac") { "Mac · navegador" } else { "Navegador" };
            r.viewers.insert(id.clone(), (device.to_string(), None));
            r.send_viewers();
            id
        };
        eprintln!("navegador {id} conectado");
        pump(&mut ws, rx, |text| {
            let msg: Value = serde_json::from_str(&text).unwrap_or_default();
            let mut r = relay.lock().unwrap();
            if matches!(msg["type"].as_str(), Some("attach" | "detach")) {
                let tab = if msg["type"] == "attach" { msg["tab"].as_u64() } else { None };
                if let Some(v) = r.viewers.get_mut(&id) {
                    v.1 = tab;
                }
                r.send_viewers();
            }
            if let Some(b) = &r.bridge {
                let _ = b.send(json!({"client": id, "msg": msg}).to_string());
            }
        });
        let mut r = relay.lock().unwrap();
        r.clients.remove(&id);
        r.viewers.remove(&id);
        r.send_viewers();
        if let Some(b) = &r.bridge {
            let _ = b.send(json!({"client": id, "closed": true}).to_string());
        }
        eprintln!("navegador {id} desconectado");
    }
}

/// Lee del WebSocket y escribe lo que llegue por `rx`, hasta que se cierre.
fn pump(ws: &mut WebSocket<TcpStream>, rx: Receiver<String>, mut on_text: impl FnMut(String)) {
    loop {
        match ws.read() {
            Ok(Message::Text(t)) => on_text(t.to_string()),
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => return,
        }
        while let Ok(line) = rx.try_recv() {
            if ws.send(Message::text(line)).is_err() {
                return;
            }
        }
    }
}
