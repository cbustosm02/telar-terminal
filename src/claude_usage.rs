//! Uso de Claude en la barra lateral: el límite de la sesión (5 h) y el semanal, con su
//! reinicio, y la cuenta (correo y plan).
//!
//! - El uso lo envía el mod `prompt-info` de Claude Code desde cualquier tab de Telar con
//!   `telar usage '<json>'` cada vez que cambia (Claude Code lo sabe por las respuestas de
//!   la API; Telar no tiene cómo leerlo). Es de la cuenta, no del tab: el último que llega
//!   vale para todos. Se guarda en `~/.config/telar/uso-claude.json` para verlo al reabrir.
//! - La cuenta sale de `claude auth status --json`, al abrir Telar y cada hora.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// Un límite: porcentaje usado (0 a 100) y cuándo se reinicia.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limit {
    pub percent: f32,
    pub resets_at: Option<SystemTime>,
}

impl Limit {
    /// Porcentaje y tiempo hasta el reinicio, ahora. Si el reinicio ya pasó, el uso
    /// volvió a cero (hasta que llegue un dato nuevo).
    pub fn now(&self, now: SystemTime) -> (f32, Option<Duration>) {
        match self.resets_at.map(|r| r.duration_since(now)) {
            Some(Ok(left)) => (self.percent, Some(left)),
            Some(Err(_)) => (0.0, None),
            None => (self.percent, None),
        }
    }
}

/// Duración de cada ventana: la sesión se reinicia cada 5 h y la semana cada 7 días.
pub const SESSION_WINDOW: Duration = Duration::from_secs(5 * 60 * 60);
pub const WEEK_WINDOW: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Ritmo: la fracción de la ventana que ya pasó (0 a 1). Es donde «debería» ir el uso
/// para que alcance hasta el reinicio: a la mitad de la sesión, el 50 %.
pub fn pace(left: Duration, window: Duration) -> f32 {
    (1.0 - left.as_secs_f32() / window.as_secs_f32()).clamp(0.0, 1.0)
}

/// Qué tan bien va el consumo contra el ritmo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    /// Por debajo del ritmo: alcanza hasta el reinicio.
    Fine,
    /// Por encima del ritmo.
    Ahead,
    /// Muy por encima (20 puntos o más) o casi en el límite (90 %).
    Far,
}

/// `percent` usado (0 a 100) contra el ritmo (0 a 1). Sin ritmo (no se sabe el
/// reinicio), solo cuenta cuán cerca está del límite.
pub fn pace_level(percent: f32, pace: Option<f32>) -> Pace {
    let used = percent / 100.0;
    match pace {
        _ if used >= 0.9 => Pace::Far,
        Some(p) if used >= p + 0.2 => Pace::Far,
        Some(p) if used > p => Pace::Ahead,
        None if used >= 0.7 => Pace::Ahead,
        _ => Pace::Fine,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    pub session: Option<Limit>,
    pub week: Option<Limit>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Account {
    pub email: Option<String>,
    pub plan: Option<String>,
}

fn limit_from(v: &Value) -> Option<Limit> {
    let percent = v["percent"].as_f64()? as f32;
    let resets_at = v["resets_at"].as_u64().map(|ms| UNIX_EPOCH + Duration::from_millis(ms));
    Some(Limit { percent: percent.clamp(0.0, 100.0), resets_at })
}

fn limit_json(l: &Option<Limit>) -> Value {
    match l {
        Some(l) => json!({
            "percent": l.percent,
            "resets_at": l.resets_at.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64),
        }),
        None => Value::Null,
    }
}

/// `{"session":{"percent":42,"resets_at":<epoch ms>},"week":{…}}` (lo que manda el mod).
pub fn parse(v: &Value) -> Usage {
    Usage { session: limit_from(&v["session"]), week: limit_from(&v["week"]) }
}

pub fn to_json(u: &Usage) -> Value {
    json!({ "session": limit_json(&u.session), "week": limit_json(&u.week) })
}

/// «2 d 5 h», «2 h 14 min», «45 min», «menos de 1 min».
pub fn until(left: Duration) -> String {
    let minutes = left.as_secs().div_ceil(60);
    match minutes {
        0 => "menos de 1 min".into(),
        m if m < 60 => format!("{m} min"),
        m if m < 60 * 24 => match m % 60 {
            0 => format!("{} h", m / 60),
            r => format!("{} h {r} min", m / 60),
        },
        m => match (m / 60) % 24 {
            0 => format!("{} d", m / (60 * 24)),
            h => format!("{} d {h} h", m / (60 * 24)),
        },
    }
}

/// `max` → «Max», `team` → «Team»; una API key no tiene plan: «API».
pub fn plan_name(subscription: Option<&str>, auth_method: Option<&str>) -> Option<String> {
    match subscription.filter(|s| !s.is_empty()) {
        Some(sub) => Some(
            sub.split(['_', '-', ' '])
                .filter(|w| !w.is_empty())
                .map(|w| {
                    let mut c = w.chars();
                    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(" "),
        ),
        None if matches!(auth_method, Some("api-key" | "console")) => Some("API".into()),
        None => None,
    }
}

fn cache_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config").join("telar").join("uso-claude.json"))
}

pub fn load() -> Usage {
    cache_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .map(|v| parse(&v))
        .unwrap_or_default()
}

pub fn save(u: &Usage) {
    let Some(path) = cache_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, to_json(u).to_string());
}

/// Ejecutable de Claude Code: Telar abierto desde el Finder no tiene `~/.local/bin` en el PATH.
fn claude_bin() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    [home.join(".local/bin/claude"), PathBuf::from("/opt/homebrew/bin/claude"), PathBuf::from("/usr/local/bin/claude")]
        .into_iter()
        .find(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("claude"))
}

/// Correo y plan de la cuenta con que inició sesión Claude Code (bloquea: llamarlo en un hilo).
pub fn account() -> Option<Account> {
    let out = std::process::Command::new(claude_bin()).args(["auth", "status", "--json"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    if v["loggedIn"] == json!(false) {
        return Some(Account::default());
    }
    Some(Account {
        email: v["email"].as_str().filter(|s| !s.is_empty()).map(String::from),
        plan: plan_name(v["subscriptionType"].as_str(), v["authMethod"].as_str()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uso_ida_y_vuelta() {
        let v = json!({ "session": { "percent": 42.5, "resets_at": 1_763_572_800_000u64 }, "week": { "percent": 180, "resets_at": null } });
        let u = parse(&v);
        assert_eq!(u.session.unwrap().percent, 42.5);
        assert_eq!(u.week.unwrap().percent, 100.0, "se acota a 100");
        assert_eq!(u.week.unwrap().resets_at, None);
        assert_eq!(parse(&to_json(&u)), u);
        assert_eq!(parse(&json!({ "session": null })), Usage::default());
    }

    #[test]
    fn reinicio_pasado_vuelve_a_cero() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let l = Limit { percent: 64.0, resets_at: Some(now + Duration::from_secs(3600)) };
        assert_eq!(l.now(now), (64.0, Some(Duration::from_secs(3600))));
        let past = Limit { resets_at: Some(now - Duration::from_secs(1)), ..l };
        assert_eq!(past.now(now), (0.0, None));
    }

    #[test]
    fn tiempo_hasta_el_reinicio() {
        let min = Duration::from_secs(60);
        assert_eq!(until(Duration::from_secs(20)), "1 min");
        assert_eq!(until(Duration::ZERO), "menos de 1 min");
        assert_eq!(until(min * 45), "45 min");
        assert_eq!(until(min * 120), "2 h");
        assert_eq!(until(min * 134), "2 h 14 min");
        assert_eq!(until(min * 60 * 53), "2 d 5 h");
        assert_eq!(until(min * 60 * 48), "2 d");
    }

    #[test]
    fn ritmo_del_consumo() {
        let h = Duration::from_secs(3600);
        assert_eq!(pace(h * 2, SESSION_WINDOW), 0.6, "faltan 2 de 5 h: pasó el 60 %");
        assert_eq!(pace(SESSION_WINDOW, SESSION_WINDOW), 0.0);
        assert_eq!(pace(h * 10, SESSION_WINDOW), 0.0, "un reinicio más lejano que la ventana no da negativo");
        assert_eq!(pace_level(40.0, Some(0.6)), Pace::Fine);
        assert_eq!(pace_level(70.0, Some(0.6)), Pace::Ahead);
        assert_eq!(pace_level(85.0, Some(0.6)), Pace::Far, "20 puntos por encima");
        assert_eq!(pace_level(92.0, Some(0.99)), Pace::Far, "casi en el límite");
        assert_eq!(pace_level(50.0, None), Pace::Fine);
        assert_eq!(pace_level(75.0, None), Pace::Ahead);
    }

    #[test]
    fn nombre_del_plan() {
        assert_eq!(plan_name(Some("max"), Some("claude.ai")).as_deref(), Some("Max"));
        assert_eq!(plan_name(Some("team_premium"), None).as_deref(), Some("Team Premium"));
        assert_eq!(plan_name(None, Some("api-key")).as_deref(), Some("API"));
        assert_eq!(plan_name(Some(""), Some("claude.ai")), None);
    }
}
