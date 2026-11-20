//! Cliente de prueba de Telar remoto: hace de navegador contra el relevo de prueba.
//!   cargo run --example remote_probe -- <tab> "<texto a escribir>"
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::Message;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tab: u64 = args.first().and_then(|t| t.parse().ok()).unwrap_or(1);
    let text = args.get(1).cloned().unwrap_or_else(|| "echo hola-desde-el-celular\r".into());
    let (mut ws, _) = tungstenite::connect("ws://127.0.0.1:8787/client").expect("relevo");
    if let tungstenite::stream::MaybeTlsStream::Plain(s) = ws.get_ref() {
        s.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    }
    let send = |ws: &mut tungstenite::WebSocket<_>, v: Value| ws.send(Message::text(v.to_string())).unwrap();
    send(&mut ws, json!({"type":"hello"}));
    let start = Instant::now();
    let mut attached = false;
    let mut typed = false;
    let mut screens = 0;
    let mut last_screen = Value::Null;
    while start.elapsed() < Duration::from_secs(8) {
        if let Ok(Message::Text(t)) = ws.read() {
            let m: Value = serde_json::from_str(&t).unwrap();
            match m["type"].as_str() {
                Some("state") => {
                    if !attached {
                        for g in m["groups"].as_array().unwrap() {
                            for t in g["tabs"].as_array().unwrap() {
                                println!("state: {} › {} [{}] remote={}", g["name"], t["title"], t["state"], t["remote"]);
                            }
                        }
                        send(&mut ws, json!({"type":"attach","tab":tab,"cols":48,"rows":16}));
                        attached = true;
                    }
                }
                Some("screen") => {
                    screens += 1;
                    if !typed {
                        println!("primera pantalla: {}×{} seq {}", m["cols"], m["rows"], m["seq"]);
                        send(&mut ws, json!({"type":"input","tab":tab,"data":text}));
                        typed = true;
                    }
                    last_screen = m;
                }
                _ => println!("otro: {t}"),
            }
        }
    }
    send(&mut ws, json!({"type":"detach","tab":tab}));
    std::thread::sleep(Duration::from_millis(300));
    println!("pantallas recibidas: {screens}");
    let ansi = last_screen["ansi"].as_str().unwrap_or_default();
    // Sin escapes, para leerla.
    let plain: String = strip(ansi);
    println!("--- última pantalla ({}×{}) ---\n{plain}", last_screen["cols"], last_screen["rows"]);
}

fn strip(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if c != '\r' {
            out.push(c);
        }
    }
    out
}
