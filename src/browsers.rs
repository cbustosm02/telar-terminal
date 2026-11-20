//! Navegadores que abrió un agente desde un tab (p. ej. el Chrome de pruebas de
//! pirani-cli). Telar no los dibuja: solo sabe de quién es cada uno, si el agente lo está
//! usando, y lo trae al frente con un clic en el ícono del tab.
//!
//! Protocolo (una línea JSON por el socket `TELAR_SOCKET`, como los hooks; el tab sale de
//! `TELAR_TAB_ID`). Quien lo envía no espera respuesta y, sin Telar, no hace nada:
//!
//! ```json
//! {"cmd":"browser","event":"open","tab":3,"name":"claude-123","pid":4567,
//!  "headless":false,"app":"pirani-cli","label":"claude 5365",
//!  "show":["/usr/local/bin/node","/ruta/pirani-cli.js","frontend","browser-inspector","browser","show","claude-123"]}
//! {"cmd":"browser","event":"activity","tab":3,"name":"claude-123"}
//! {"cmd":"browser","event":"close","tab":3,"name":"claude-123"}
//! ```
//!
//! - `open` registra (o actualiza, si se reabrió con otro pid) el navegador `name`.
//! - `activity`: el agente lo usó; el ícono late unos segundos.
//! - `close`: se cerró. Si no llega (se cerró solo, se cortó), Telar lo nota igual: revisa
//!   cada pocos segundos que su pid siga vivo.
//! - `show` (opcional): comando para mostrarlo (p. ej. reabrir con ventana uno que corría
//!   sin ella, o restaurar uno minimizado). Telar lo corre al hacer clic y después activa
//!   la app del pid.

use std::time::{Duration, Instant};

use serde_json::Value;

/// Cuánto late el ícono después de una acción del agente.
pub const ACTIVE_FOR: Duration = Duration::from_secs(3);
/// Sin acciones durante este tiempo, el ícono queda apagado (gris).
pub const IDLE_AFTER: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub struct Browser {
    pub name: String,
    pub pid: i32,
    pub headless: bool,
    /// Quién lo abrió (p. ej. «pirani-cli»).
    pub app: Option<String>,
    /// Agente según quien lo abrió (p. ej. «claude 5365»).
    pub label: Option<String>,
    pub show: Vec<String>,
    pub last_activity: Instant,
}

impl Browser {
    pub fn active(&self) -> bool {
        self.last_activity.elapsed() < ACTIVE_FOR
    }

    pub fn idle(&self) -> bool {
        self.last_activity.elapsed() >= IDLE_AFTER
    }
}

pub enum Event {
    Open(Browser),
    Activity(String),
    Close(String),
}

/// Lee un pedido `browser`. Error si le falta algo.
pub fn parse(body: &Value) -> Result<Event, String> {
    let name = body["name"].as_str().filter(|n| !n.trim().is_empty()).ok_or("falta name")?.to_string();
    match body["event"].as_str() {
        Some("open") => {
            let pid = body["pid"].as_i64().filter(|p| *p > 0 && *p <= i32::MAX as i64).ok_or("falta pid")? as i32;
            let text = |k: &str| body[k].as_str().filter(|s| !s.is_empty()).map(str::to_string);
            let show = body["show"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
            Ok(Event::Open(Browser {
                name,
                pid,
                headless: body["headless"].as_bool().unwrap_or(false),
                app: text("app"),
                label: text("label"),
                show,
                last_activity: Instant::now(),
            }))
        }
        Some("activity") => Ok(Event::Activity(name)),
        Some("close") => Ok(Event::Close(name)),
        other => Err(format!("event desconocido: {other:?} (usa open, activity o close)")),
    }
}

/// Aplica un evento a los navegadores de un tab. Devuelve si cambió algo visible.
pub fn apply(list: &mut Vec<Browser>, event: Event) -> bool {
    match event {
        Event::Open(b) => {
            list.retain(|x| x.name != b.name);
            list.push(b);
            true
        }
        Event::Activity(name) => match list.iter_mut().find(|b| b.name == name) {
            Some(b) => {
                b.last_activity = Instant::now();
                true
            }
            None => false,
        },
        Event::Close(name) => {
            let before = list.len();
            list.retain(|b| b.name != name);
            list.len() != before
        }
    }
}

/// Si el proceso sigue vivo (`kill(pid, 0)`; EPERM también cuenta como vivo).
pub fn alive(pid: i32) -> bool {
    pid > 0 && (unsafe { libc::kill(pid, 0) } == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

/// El que conviene mostrar: el último que usó el agente.
pub fn main(list: &[Browser]) -> Option<&Browser> {
    list.iter().max_by_key(|b| b.last_activity)
}

/// «hace 5 s», «hace 3 min»…
pub fn ago(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("hace {s} s"),
        60..=3599 => format!("hace {} min", s / 60),
        _ => format!("hace {} h", s / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn abrir_usar_y_cerrar() {
        let mut list = Vec::new();
        let open = json!({"cmd":"browser","event":"open","tab":1,"name":"a","pid":42,"headless":true,"show":["node","x.js","show","a"]});
        assert!(apply(&mut list, parse(&open).unwrap()));
        assert_eq!(list.len(), 1);
        assert!(list[0].headless && list[0].show.len() == 4);
        // Reabierto con otro pid: se reemplaza, no se duplica.
        let reopen = json!({"event":"open","name":"a","pid":43});
        apply(&mut list, parse(&reopen).unwrap());
        assert_eq!((list.len(), list[0].pid, list[0].headless), (1, 43, false));
        assert!(apply(&mut list, parse(&json!({"event":"activity","name":"a"})).unwrap()));
        assert!(!apply(&mut list, parse(&json!({"event":"activity","name":"otro"})).unwrap()));
        assert!(apply(&mut list, parse(&json!({"event":"close","name":"a"})).unwrap()));
        assert!(list.is_empty());
    }

    #[test]
    fn pedidos_incompletos() {
        assert!(parse(&json!({"event":"open","name":"a"})).is_err());
        assert!(parse(&json!({"event":"open","pid":3})).is_err());
        assert!(parse(&json!({"event":"x","name":"a"})).is_err());
    }

    #[test]
    fn proceso_vivo() {
        assert!(alive(std::process::id() as i32));
        assert!(!alive(0));
    }
}
