//! Uso de Claude en la barra lateral: el límite de la sesión (5 h) y el semanal, con su
//! reinicio, y la cuenta (correo y plan).
//!
//! - El uso lo consulta Telar con el token OAuth de cada cuenta en
//!   `GET https://api.anthropic.com/api/oauth/usage` (el mismo que usan cswap y Claude
//!   Usage Tracker): `five_hour` y `seven_day`, con `utilization` (0 a 100) y `resets_at`.
//!   Se guarda por cuenta en `~/.config/telar/uso-claude.json` para verlo al reabrir.
//! - La cuenta sale de `claude auth status --json`, al abrir Telar y cada hora.
//! - El token va a curl por la entrada estándar (`-K -`): no aparece en la lista de procesos.

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

/// `{"session":{"percent":42,"resets_at":<epoch ms>},"week":{…}}` (el formato del caché).
pub fn parse(v: &Value) -> Usage {
    Usage { session: limit_from(&v["session"]), week: limit_from(&v["week"]) }
}

pub fn to_json(u: &Usage) -> Value {
    json!({ "session": limit_json(&u.session), "week": limit_json(&u.week) })
}

// ── Consulta a la API ───────────────────────────────────────────────────────────

/// Fecha ISO 8601 (`2026-11-19T21:00:00.123456+00:00` o con `Z`) a un instante.
pub fn parse_iso(s: &str) -> Option<SystemTime> {
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    // Zona: lo que sigue a los segundos y a las fracciones.
    let rest = s.get(19..)?.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match rest {
        "" | "Z" | "z" => 0,
        tz => {
            let sign = if tz.starts_with('-') { -1 } else { 1 };
            let hh = tz.get(1..3)?.parse::<i64>().ok()?;
            let mm = tz.get(tz.len() - 2..)?.parse::<i64>().ok()?;
            sign * (hh * 3600 + mm * 60)
        }
    };
    // Días desde 1970 (algoritmo de Howard Hinnant).
    let (yy, mm) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * mm + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + se - offset;
    u64::try_from(secs).ok().map(|s| UNIX_EPOCH + Duration::from_secs(s))
}

/// Respuesta de `/api/oauth/usage`.
pub fn parse_api(v: &Value) -> Usage {
    let window = |w: &Value| -> Option<Limit> {
        let pct = w["utilization"].as_f64().or_else(|| w["utilization"].as_str()?.parse().ok())? as f32;
        Some(Limit { percent: pct.clamp(0.0, 100.0), resets_at: w["resets_at"].as_str().and_then(parse_iso) })
    };
    Usage { session: window(&v["five_hour"]), week: window(&v["seven_day"]) }
}

/// Comillas para un valor del archivo de configuración de curl.
fn curl_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// curl con la configuración (URL, headers con el token, cuerpo) por la entrada estándar.
/// Devuelve el código HTTP y el cuerpo.
fn curl_private(config: &str) -> Result<(u16, Vec<u8>), String> {
    use std::io::Write as _;
    let mut child = std::process::Command::new("/usr/bin/curl")
        .args(["-sS", "--max-time", "12", "-K", "-", "-w", "\n%{http_code}"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(config.as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("sin conexión: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let text = out.stdout;
    let cut = text.iter().rposition(|b| *b == b'\n').unwrap_or(0);
    let code = String::from_utf8_lossy(&text[cut..]).trim().parse::<u16>().unwrap_or(0);
    Ok((code, text[..cut].to_vec()))
}

/// Uso de la cuenta dueña de `access_token`.
pub fn fetch(access_token: &str) -> Result<Usage, String> {
    let config = format!(
        "url = {}\nheader = {}\nheader = {}\nheader = {}\n",
        curl_quote("https://api.anthropic.com/api/oauth/usage"),
        curl_quote(&format!("Authorization: Bearer {access_token}")),
        curl_quote("anthropic-beta: oauth-2025-04-20"),
        curl_quote("User-Agent: telar"),
    );
    let (code, body) = curl_private(&config)?;
    match code {
        200 => serde_json::from_slice::<Value>(&body).map(|v| parse_api(&v)).map_err(|e| format!("respuesta inesperada: {e}")),
        401 => Err("el token venció".into()),
        429 => Err("demasiadas consultas; se reintenta luego".into()),
        c => Err(format!("la API respondió {c}")),
    }
}

/// Error de renovación: el refresh token ya no sirve (hay que iniciar sesión de nuevo).
pub const DEAD_LOGIN: &str = "la sesión de esta cuenta venció: inicia sesión con ella y agrégala otra vez";

/// Renueva el token OAuth de una credencial guardada (de una cuenta que no es la activa:
/// la activa la renueva Claude Code). Devuelve la credencial con los tokens nuevos.
pub fn refresh(credentials: &str) -> Result<String, String> {
    let mut v: Value = serde_json::from_str(credentials).map_err(|_| "credencial dañada".to_string())?;
    let refresh_token = v["claudeAiOauth"]["refreshToken"].as_str().ok_or(DEAD_LOGIN)?.to_string();
    let body = json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
    });
    let config = format!(
        "url = {}\nrequest = \"POST\"\nheader = {}\nheader = {}\ndata = {}\n",
        curl_quote("https://platform.claude.com/v1/oauth/token"),
        curl_quote("Content-Type: application/json"),
        curl_quote("User-Agent: telar"),
        curl_quote(&body.to_string()),
    );
    let (code, resp) = curl_private(&config)?;
    let r: Value = serde_json::from_slice(&resp).unwrap_or(Value::Null);
    if code != 200 {
        return Err(match r["error"].as_str() {
            Some("invalid_grant") => DEAD_LOGIN.into(),
            _ => format!("no se pudo renovar el token ({code})"),
        });
    }
    let oauth = &mut v["claudeAiOauth"];
    oauth["accessToken"] = r["access_token"].clone();
    let expires_in = r["expires_in"].as_u64().unwrap_or(3600);
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    oauth["expiresAt"] = json!(now_ms + expires_in * 1000);
    if let Some(rt) = r["refresh_token"].as_str() {
        oauth["refreshToken"] = json!(rt);
    }
    if let Some(scope) = r["scope"].as_str() {
        oauth["scopes"] = json!(scope.split_whitespace().collect::<Vec<_>>());
    }
    if oauth["accessToken"].as_str().is_none() {
        return Err("la renovación no trajo token".into());
    }
    Ok(v.to_string())
}

/// Token de acceso de una credencial y si ya venció (o vence en menos de 1 min).
pub fn access_token(credentials: &str) -> Option<(String, bool)> {
    let v: Value = serde_json::from_str(credentials).ok()?;
    let token = v["claudeAiOauth"]["accessToken"].as_str()?.to_string();
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let expired = v["claudeAiOauth"]["expiresAt"].as_u64().is_some_and(|e| e <= now_ms + 60_000);
    Some((token, expired))
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

/// Último uso conocido de cada cuenta (clave de `cuentas`), para verlo al reabrir.
pub fn load() -> std::collections::HashMap<String, Usage> {
    let v = cache_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or(Value::Null);
    v["cuentas"].as_object().map(|m| m.iter().map(|(k, u)| (k.clone(), parse(u))).collect()).unwrap_or_default()
}

pub fn save(all: &std::collections::HashMap<String, Usage>) {
    let Some(path) = cache_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let map: serde_json::Map<String, Value> = all.iter().map(|(k, u)| (k.clone(), to_json(u))).collect();
    let _ = std::fs::write(path, json!({ "cuentas": map }).to_string());
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
    fn fechas_iso() {
        let t = |s: &str| parse_iso(s).map(|t| t.duration_since(UNIX_EPOCH).unwrap().as_secs());
        assert_eq!(t("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(t("2026-11-19T21:00:00.123456+00:00"), Some(1_795_122_000));
        assert_eq!(t("2026-11-19T16:00:00-05:00"), Some(1_795_122_000), "con zona horaria");
        assert_eq!(t("2024-02-29T12:30:15Z"), Some(1_709_209_815), "año bisiesto");
        assert_eq!(t("no es fecha"), None);
    }

    #[test]
    fn respuesta_de_la_api() {
        let v = json!({
            "five_hour": { "utilization": 42.0, "resets_at": "2026-11-19T21:00:00+00:00" },
            "seven_day": { "utilization": 18, "resets_at": null },
            "seven_day_opus": null,
        });
        let u = parse_api(&v);
        assert_eq!(u.session.unwrap().percent, 42.0);
        assert!(u.session.unwrap().resets_at.is_some());
        assert_eq!(u.week.unwrap(), Limit { percent: 18.0, resets_at: None });
        assert_eq!(parse_api(&json!({ "five_hour": null })), Usage::default());
    }

    #[test]
    fn token_de_la_credencial() {
        let c = r#"{"claudeAiOauth":{"accessToken":"t","expiresAt":1}}"#;
        assert_eq!(access_token(c), Some(("t".into(), true)), "venció hace mucho");
        let c = r#"{"claudeAiOauth":{"accessToken":"t","expiresAt":99999999999999}}"#;
        assert_eq!(access_token(c), Some(("t".into(), false)));
        assert_eq!(curl_quote(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    #[test]
    fn nombre_del_plan() {
        assert_eq!(plan_name(Some("max"), Some("claude.ai")).as_deref(), Some("Max"));
        assert_eq!(plan_name(Some("team_premium"), None).as_deref(), Some("Team Premium"));
        assert_eq!(plan_name(None, Some("api-key")).as_deref(), Some("API"));
        assert_eq!(plan_name(Some(""), Some("claude.ai")), None);
    }
}
