//! Hooks de Claude Code que llegan **por la propia terminal** (un Claude en un servidor,
//! por SSH). El hook remoto (de claude-mods) escribe en su tty:
//!
//!   ESC ] 7768 ; telar-hook ; <base64 del JSON del hook> BEL   (o ST: ESC \)
//!
//! La secuencia viaja por la conexión SSH (y por tmux, con `allow-passthrough on`) hasta
//! el PTY del tab. `ScanPty` envuelve el PTY de alacritty: lee lo mismo que antes y, de
//! paso, reconoce esa secuencia y la manda como `UserEvent::InbandHook(tab, json)`. El
//! emulador la descarta sola (es un OSC que no conoce). Buscar es barato: solo se mira
//! dónde hay un ESC.

use std::fs::File;
use std::io::{self, Read};
use std::sync::Arc;

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite, Pty};
use polling::{Event, PollMode, Poller};
use serde_json::Value;
use winit::event_loop::EventLoopProxy;

use crate::UserEvent;

const PREFIX: &[u8] = b"\x1b]7768;telar-hook;";
/// Más que esto no es un hook: se descarta (y no se sigue juntando).
const MAX_PAYLOAD: usize = 8 * 1024;

/// Reconoce la secuencia en un flujo que llega por partes.
#[derive(Default)]
pub struct Scanner {
    /// Cuántos bytes del prefijo coinciden hasta ahora.
    matched: usize,
    /// Con el prefijo completo: lo que va del payload.
    payload: Option<Vec<u8>>,
    /// Vino un ESC dentro del payload (¿empieza el ST?).
    esc: bool,
}

impl Scanner {
    /// Revisa `bytes` y devuelve los payloads (base64) completos.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if let Some(p) = self.payload.as_mut() {
                let b = bytes[i];
                i += 1;
                match b {
                    0x07 => out.extend(self.finish()),
                    b'\\' if self.esc => out.extend(self.finish()),
                    0x1b => self.esc = true,
                    _ if p.len() >= MAX_PAYLOAD => *self = Self::default(),
                    _ => {
                        self.esc = false;
                        p.push(b);
                    }
                }
                continue;
            }
            if self.matched == 0 {
                // Lo normal: saltar hasta el próximo ESC.
                match bytes[i..].iter().position(|&b| b == 0x1b) {
                    Some(at) => i += at,
                    None => break,
                }
            }
            if bytes[i] == PREFIX[self.matched] {
                self.matched += 1;
                if self.matched == PREFIX.len() {
                    self.matched = 0;
                    self.payload = Some(Vec::new());
                }
            } else {
                // No era: si este byte es otro ESC, puede empezar de nuevo aquí.
                self.matched = usize::from(bytes[i] == 0x1b);
            }
            i += 1;
        }
        out
    }

    fn finish(&mut self) -> Option<Vec<u8>> {
        let p = self.payload.take();
        self.esc = false;
        p
    }
}

/// El JSON de un payload (base64). `None` si no es un hook válido.
pub fn decode(payload: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(payload).ok()?;
    let bytes = base64(text)?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v["hook_event_name"].is_string().then_some(v)
}

fn base64(s: &str) -> Option<Vec<u8>> {
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
            n |= val(*c)? << (18 - 6 * i);
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..chunk.len().saturating_sub(1)]);
    }
    Some(out)
}

/// Lector del PTY que reconoce los hooks remotos mientras lee.
pub struct ScanReader {
    file: File,
    scanner: Scanner,
    tab: u64,
    proxy: EventLoopProxy<UserEvent>,
}

impl Read for ScanReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.file.read(buf)?;
        for payload in self.scanner.feed(&buf[..n]) {
            if let Some(v) = decode(&payload) {
                let _ = self.proxy.send_event(UserEvent::InbandHook(self.tab, v));
            }
        }
        Ok(n)
    }
}

/// El PTY de alacritty con su lectura envuelta (todo lo demás, igual).
pub struct ScanPty {
    inner: Pty,
    reader: ScanReader,
}

impl ScanPty {
    pub fn new(inner: Pty, tab: u64, proxy: EventLoopProxy<UserEvent>) -> io::Result<Self> {
        // Otro descriptor del mismo PTY (misma descripción de archivo: también sin bloqueo).
        let file = inner.file().try_clone()?;
        Ok(Self { inner, reader: ScanReader { file, scanner: Scanner::default(), tab, proxy } })
    }
}

impl EventedReadWrite for ScanPty {
    type Reader = ScanReader;
    type Writer = File;

    unsafe fn register(&mut self, poll: &Arc<Poller>, interest: Event, mode: PollMode) -> io::Result<()> {
        unsafe { self.inner.register(poll, interest, mode) }
    }

    fn reregister(&mut self, poll: &Arc<Poller>, interest: Event, mode: PollMode) -> io::Result<()> {
        self.inner.reregister(poll, interest, mode)
    }

    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        self.inner.deregister(poll)
    }

    fn reader(&mut self) -> &mut ScanReader {
        &mut self.reader
    }

    fn writer(&mut self) -> &mut File {
        self.inner.writer()
    }
}

impl EventedPty for ScanPty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.inner.next_child_event()
    }
}

impl OnResize for ScanPty {
    fn on_resize(&mut self, size: WindowSize) {
        self.inner.on_resize(size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(json: &str, st: bool) -> Vec<u8> {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let b = json.as_bytes();
        let mut b64 = String::new();
        for c in b.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                b64.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
            }
        }
        let mut out = PREFIX.to_vec();
        out.extend(b64.bytes());
        out.extend_from_slice(if st { b"\x1b\\" } else { b"\x07" });
        out
    }

    #[test]
    fn reconoce_el_hook_entre_otra_salida() {
        let mut s = Scanner::default();
        let mut stream = b"hola \x1b[31mrojo\x1b[0m ".to_vec();
        stream.extend(seq(r#"{"hook_event_name":"Stop"}"#, false));
        stream.extend(b" \x1b]0;titulo\x07 fin");
        stream.extend(seq(r#"{"hook_event_name":"PreToolUse"}"#, true));
        let got: Vec<Value> = s.feed(&stream).iter().filter_map(|p| decode(p)).collect();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["hook_event_name"], "Stop");
        assert_eq!(got[1]["hook_event_name"], "PreToolUse");
    }

    #[test]
    fn llega_partido_en_varias_lecturas() {
        let full = seq(r#"{"hook_event_name":"Notification","message":"¿Puedo?"}"#, false);
        let mut s = Scanner::default();
        let mut got = Vec::new();
        for chunk in full.chunks(3) {
            got.extend(s.feed(chunk));
        }
        assert_eq!(got.len(), 1);
        assert_eq!(decode(&got[0]).unwrap()["message"], "¿Puedo?");
    }

    #[test]
    fn lo_que_no_es_hook_se_ignora() {
        let mut s = Scanner::default();
        // Otro OSC, un prefijo parecido, base64 que no es JSON de hook, y uno enorme.
        assert!(s.feed(b"\x1b]7768;otra-cosa;abc\x07\x1b]776").is_empty());
        let not_hook = seq(r#"{"x":1}"#, false);
        assert!(s.feed(&not_hook).iter().filter_map(|p| decode(p)).next().is_none());
        let mut huge = PREFIX.to_vec();
        huge.extend(std::iter::repeat_n(b'A', MAX_PAYLOAD + 10));
        huge.push(0x07);
        assert!(s.feed(&huge).is_empty());
        // Y después sigue funcionando.
        assert_eq!(s.feed(&seq(r#"{"hook_event_name":"Stop"}"#, false)).len(), 1);
    }
}
