//! Telar remoto, lado de la app: lo que `telar bridge` (ver `bridge.rs`) le lleva al
//! celular. El bridge se suscribe por el socket (`remote-subscribe`) y Telar le empuja
//! líneas JSON:
//!
//! - `{"type":"state","groups":[…]}`: grupos y tabs con su estado y la pregunta pendiente
//!   (lo de `telar tabs` más la bandeja). Solo cuando cambia.
//! - `{"type":"screen","tab","cols","rows","ansi","cursor":[col,row],"seq"}`: la pantalla
//!   visible completa de un tab que alguien mira, como mucho cada `SCREEN_EVERY`.
//!
//! Sin suscriptores no se calcula nada. Mientras alguien mira un tab, ese tab toma su
//! tamaño (`Terminal::set_remote_size`); al soltarlo vuelve al de la ventana.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::inbox;
use crate::terminal::{AgentState, Terminal};
use crate::workspace::Workspace;

/// Pantallas de un tab: como mucho 10 por segundo.
pub const SCREEN_EVERY: Duration = Duration::from_millis(100);
/// El estado se recalcula al menos así de seguido (si no cambió, no se manda).
pub const STATE_EVERY: Duration = Duration::from_secs(1);
/// Y como mucho así de seguido, aunque cambie más rápido.
pub const STATE_MIN: Duration = Duration::from_millis(250);

/// Tamaños razonables para un cliente remoto.
pub const COLS: std::ops::RangeInclusive<usize> = 20..=400;
pub const ROWS: std::ops::RangeInclusive<usize> = 5..=200;

/// Este Mac vinculado a Telar web: lo que se muestra (sin secretos).
#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub who: String,
    pub server: String,
}

/// Inicio de sesión en curso: esperando que lo autorices en el navegador.
pub struct LoginUi {
    /// Número de este intento: los avisos de uno anterior (cancelado) se ignoran.
    pub generation: u64,
    pub user_code: Option<String>,
    pub url: String,
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Debug)]
pub enum LoginEvent {
    Started { generation: u64, user_code: Option<String>, url: String },
    Done(u64, SessionInfo),
    Failed(u64, String),
    /// Se cerró la sesión; con el error si no se pudo avisar al servidor.
    LoggedOut(Option<String>),
}

/// Un navegador que mira este Mac (según el relevo).
#[derive(Clone, Debug)]
pub struct Viewer {
    pub device: String,
    pub tab: Option<u64>,
}

/// Los dispositivos que miran, sin repetir: varias pestañas (o conexiones) del mismo
/// navegador son un solo dispositivo. Se agrupa por `browser` (id estable del navegador,
/// si el relevo lo manda) o, si no, por el nombre del dispositivo. Si alguna de sus
/// pestañas mira un tab, el dispositivo cuenta como mirando ese tab.
pub fn viewers_from(list: &Value) -> Vec<Viewer> {
    let mut out: Vec<(String, Viewer)> = Vec::new();
    for v in list.as_array().into_iter().flatten() {
        let device: String = v["device"].as_str().unwrap_or("Navegador").chars().filter(|c| !c.is_control()).take(60).collect();
        let key = v["browser"].as_str().map_or_else(|| device.clone(), |b| b.chars().take(80).collect());
        let tab = v["tab"].as_u64();
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some((_, seen)) => seen.tab = seen.tab.or(tab),
            None => out.push((key, Viewer { device, tab })),
        }
    }
    out.into_iter().map(|(_, v)| v).collect()
}

/// Un tab que alguien mira desde el celular.
pub struct Watch {
    pub seq: u64,
    pub dirty: bool,
    pub sent: Option<Instant>,
}

#[derive(Default)]
pub struct Hub {
    subscribers: Vec<mpsc::Sender<String>>,
    last_state: String,
    state_sent: Option<Instant>,
    /// Algo cambió (un hook, un tab nuevo…): el estado se manda en cuanto se pueda.
    pub state_dirty: bool,
    pub watches: HashMap<u64, Watch>,
    /// Hay alguien con Telar web abierto: el estado se revisa cada `STATE_EVERY` (títulos,
    /// preguntas que aparecen en pantalla…). Sin nadie, solo cuando algo avisa (un hook).
    pub poll: bool,
}

impl Hub {
    pub fn active(&self) -> bool {
        !self.subscribers.is_empty()
    }

    pub fn subscribe(&mut self, tx: mpsc::Sender<String>) {
        self.subscribers.push(tx);
        // El nuevo necesita el estado aunque no haya cambiado.
        self.last_state.clear();
        self.state_dirty = true;
    }

    /// Quita a los suscriptores que se fueron. Devuelve si queda alguno.
    pub fn prune(&mut self) -> bool {
        self.broadcast(r#"{"type":"keepalive"}"#)
    }

    /// Manda una línea a todos; quita a los que se fueron. Devuelve si queda alguno.
    pub fn broadcast(&mut self, line: &str) -> bool {
        self.subscribers.retain(|tx| tx.send(line.to_string()).is_ok());
        self.active()
    }

    /// Hay que mandar el estado: cambió algo, o pasó `STATE_EVERY` (preguntas que aparecen
    /// en pantalla después del hook, títulos…), y no se mandó hace menos de `STATE_MIN`.
    pub fn state_due(&self, now: Instant) -> bool {
        let since = self.state_sent.map(|t| now.duration_since(t));
        match since {
            None => true,
            Some(d) if d < STATE_MIN => false,
            Some(d) => self.state_dirty || (self.poll && d >= STATE_EVERY),
        }
    }

    /// Manda el estado si es distinto del último. Devuelve si quedan suscriptores.
    pub fn send_state(&mut self, state: Value, now: Instant) -> bool {
        self.state_sent = Some(now);
        self.state_dirty = false;
        let line = state.to_string();
        if line == self.last_state {
            return self.active();
        }
        self.last_state = line.clone();
        self.broadcast(&line)
    }

    /// Cuándo hay que volver a mirar (pantallas pendientes, el próximo estado).
    pub fn next_wake(&self, now: Instant) -> Option<Instant> {
        if !self.active() {
            return None;
        }
        let state = match (self.state_dirty, self.poll) {
            (true, _) => Some(self.state_sent.map_or(now, |t| t + STATE_MIN)),
            (false, true) => Some(self.state_sent.map_or(now, |t| t + STATE_EVERY)),
            // Nadie mira y nada cambió: no hay por qué despertar.
            (false, false) => None,
        };
        let screens = self.watches.values().filter(|w| w.dirty).map(|w| w.sent.map_or(now, |t| t + SCREEN_EVERY)).min();
        match (state, screens) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

/// Identificador estable de una pregunta: si cambia el texto o las opciones, cambia.
pub fn question_id(tab: u64, question: Option<&str>, options: &[(char, String)]) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    tab.hash(&mut h);
    question.hash(&mut h);
    options.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn epoch_secs(ago: Duration) -> u64 {
    std::time::SystemTime::now().checked_sub(ago).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

/// Grupos y tabs para el celular.
/// `view_only`: el Mac está en «Solo ver desde la web» (la web oculta los controles).
/// `accounts`: las cuentas de Claude guardadas y su uso (ver `State::accounts_json`).
pub fn state(ws: &Workspace, terms: &HashMap<u64, Terminal>, view_only: bool, accounts: Value) -> Value {
    let entries: HashMap<u64, inbox::Entry> = inbox::entries(ws, terms).into_iter().map(|e| (e.tab, e)).collect();
    let groups: Vec<Value> = ws
        .groups
        .iter()
        .map(|g| {
            let tabs: Vec<Value> = g
                .tabs
                .iter()
                .enumerate()
                .filter_map(|(i, id)| {
                    let t = terms.get(id)?;
                    let entry = entries.get(id);
                    let needs = t.agent.state == AgentState::NeedsYou;
                    let options = entry.filter(|_| needs).map(|e| e.options.clone()).unwrap_or_default();
                    let question = entry.filter(|_| needs).and_then(|e| e.detail.clone());
                    let mut tab = json!({
                        "id": id,
                        "title": t.label().map(String::from).unwrap_or_else(|| format!("Tab {}", i + 1)),
                        "name": t.name,
                        "state": t.agent.state.name(),
                        "suspended": t.is_suspended(),
                        "claude_shells": t.claude_shells,
                        "browser": !t.browsers.is_empty(),
                        "message": t.agent.message,
                        // El último paso (trabajando) o la pregunta (te necesita).
                        "detail": entry.and_then(|e| e.detail.clone()),
                        "cwd": t.agent.cwd,
                        "since": epoch_secs(t.agent.since.elapsed()),
                        "remote": t.remote_size.is_some(),
                    });
                    if needs {
                        tab["question"] = json!(question);
                        tab["question_id"] = json!(question_id(*id, question.as_deref(), &options));
                        tab["options"] = json!(options.iter().map(|(d, l)| json!({ "digit": d.to_string(), "label": l })).collect::<Vec<_>>());
                    }
                    Some(tab)
                })
                .collect();
            json!({ "name": g.name, "dir": g.dir, "tabs": tabs })
        })
        .collect();
    json!({ "type": "state", "view_only": view_only, "accounts": accounts, "groups": groups })
}

/// Pantalla visible de un tab, con el cursor donde está.
pub fn screen(id: u64, t: &Terminal, seq: u64) -> Value {
    let term = t.term.lock();
    let mut ansi = crate::screen::to_ansi(&*term);
    // `to_ansi` deja el cursor oculto (es para una imagen fija): aquí va donde está.
    if let Some(stripped) = ansi.strip_suffix("\x1b[?25l") {
        ansi.truncate(stripped.len());
    }
    let cursor = term.grid().cursor.point;
    let (col, row) = (cursor.column.0, cursor.line.0.max(0) as usize);
    ansi.push_str(&format!("\x1b[{};{}H", row + 1, col + 1));
    if term.mode().contains(alacritty_terminal::term::TermMode::SHOW_CURSOR) {
        ansi.push_str("\x1b[?25h");
    }
    let size = t.size();
    json!({ "type": "screen", "tab": id, "cols": size.cols, "rows": size.rows, "ansi": ansi, "cursor": [col, row], "seq": seq })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_navegador_con_varias_pestanas_es_uno() {
        let list = json!([
            {"client":"a","device":"Mac · Chrome","tab":null},
            {"client":"b","device":"Mac · Chrome","tab":3},
            {"client":"c","device":"Mac · Chrome","tab":null},
            {"client":"d","device":"iPhone · Safari","tab":null},
        ]);
        let v = viewers_from(&list);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].device.as_str(), v[0].tab), ("Mac · Chrome", Some(3)));
        // Con id de navegador, dos Mac con Chrome son dos dispositivos.
        let list = json!([{"device":"Mac · Chrome","browser":"x"},{"device":"Mac · Chrome","browser":"y"}]);
        assert_eq!(viewers_from(&list).len(), 2);
    }

    #[test]
    fn la_pregunta_cambia_su_id() {
        let opts = vec![('1', "Yes".to_string()), ('2', "No".to_string())];
        let a = question_id(3, Some("¿Ejecutar ls?"), &opts);
        assert_eq!(a, question_id(3, Some("¿Ejecutar ls?"), &opts));
        assert_ne!(a, question_id(3, Some("¿Ejecutar rm?"), &opts));
        assert_ne!(a, question_id(4, Some("¿Ejecutar ls?"), &opts));
        assert_ne!(a, question_id(3, Some("¿Ejecutar ls?"), &opts[..1]));
    }

    #[test]
    fn ritmo_del_estado() {
        let mut hub = Hub::default();
        let (tx, rx) = mpsc::channel();
        hub.subscribe(tx);
        hub.poll = true;
        let t0 = Instant::now();
        assert!(hub.state_due(t0));
        assert!(hub.send_state(json!({"a":1}), t0));
        assert_eq!(rx.try_recv().unwrap(), r#"{"a":1}"#);
        // Igual: no se repite.
        hub.send_state(json!({"a":1}), t0 + STATE_EVERY);
        assert!(rx.try_recv().is_err());
        // Cambió algo, pero hace muy poco que se mandó.
        hub.state_dirty = true;
        assert!(!hub.state_due(t0 + STATE_EVERY + Duration::from_millis(10)));
        assert!(hub.state_due(t0 + STATE_EVERY + STATE_MIN));
        // Sin nadie con la web abierta, no se revisa por tiempo: solo si algo cambió.
        hub.poll = false;
        hub.state_dirty = false;
        assert!(!hub.state_due(t0 + STATE_EVERY * 2));
        assert_eq!(hub.next_wake(t0 + STATE_EVERY * 2), None);
        hub.state_dirty = true;
        assert!(hub.state_due(t0 + STATE_EVERY * 2));
        // Se fue el suscriptor: ya no hay nadie.
        drop(rx);
        assert!(!hub.send_state(json!({"a":2}), t0 + STATE_EVERY * 3));
    }
}
