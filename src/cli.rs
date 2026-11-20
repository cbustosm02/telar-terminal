//! CLI de Telar: comandos que hablan con la instancia de Telar donde corren
//! (la de `TELAR_SOCKET`). Pensados para que Claude Code, desde un tab, use la terminal.
//!
//! `<tab>` puede ser: el id, `self` (el tab actual), `grupo/n` (n desde 1) o parte del título.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::ipc;

pub const USAGE: &str = "Uso: telar [comando]

Sin comando abre la terminal. Comandos (dentro de Telar):
  tabs                               grupos y tabs con su estado, en JSON
  status <tab>                       estado de un tab, en JSON
  read <tab> [-n LÍNEAS]             texto de la pantalla y el scrollback (por defecto 50 líneas)
  send <tab> <texto…> [--enter]      escribe en el tab (--enter: y pulsa Enter)
  new [--group G] [--cwd DIR] [--focus] [-- comando…]
                                     abre un tab (en el grupo del tab actual si no se indica)
                                     y ejecuta el comando; imprime el id del tab nuevo
  group [--group G] [--dir DIR] [--name NOMBRE]
                                     muestra o cambia la carpeta y el nombre de un grupo
                                     (por defecto el del tab actual); sus tabs nuevos abren
                                     en esa carpeta. --dir \"\" la quita
  suspend <tab> | --group G [--force]
                                     suspende: cierra la shell y Claude y deja la última pantalla
                                     a la vista (no suspende tabs que trabajan, salvo --force)
  resume <tab> | --group G           reanuda (Claude retoma la misma sesión)
  clear <tab>                        borra la pantalla y el scrollback del tab (como ⌘K)
  awake [on|off|auto]                mantener el Mac despierto (caffeinate -dims): siempre,
                                     nunca, o solo mientras algún Claude trabaja
  browser open|activity|close --name N [--pid P] [--headless] [--app A] [--label L] [-- comando…]
                                     avisa que el agente de este tab abrió, usó o cerró un
                                     navegador; el tab muestra un ícono que lo trae al frente
                                     (corre el comando, si lo hay, y activa la app del pid)
  update [--check]                   busca la última versión y la instala (--check: solo busca)
  version                            versión instalada
  wait <tab> [--state S] [--timeout SEG]
                                     espera a que Claude en ese tab deje de trabajar
                                     (o a que llegue al estado S: idle, done, needs-you, shell)
  wait <tab> --match 'TEXTO|OTRO' [--timeout SEG]
                                     espera a que el tab muestre una línea nueva con alguno de
                                     esos textos (p. ej. lo que imprime un servidor al compilar)
                                     e imprime esa línea

Instalación:
  install      hooks de Claude Code, enlace ~/.local/bin/telar y regla para Claude
  uninstall    quita todo lo anterior

<tab>: id, self, grupo/n (n desde 1) o parte del título.";

fn tab_spec(s: &str) -> Value {
    if s == "self" {
        if let Some(id) = std::env::var("TELAR_TAB_ID").ok().and_then(|v| v.parse::<u64>().ok()) {
            return json!(id);
        }
    }
    json!(s)
}

fn call(body: Value) -> Result<Value, String> {
    let v = ipc::request(&body, true)?;
    if v["ok"] == json!(false) {
        return Err(v["error"].as_str().unwrap_or("error desconocido").to_string());
    }
    Ok(v)
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

/// Separa flags (`--x valor`, `-n valor`, `--enter`) de los argumentos posicionales.
/// Todo lo que va después de `--` es posicional.
struct Args {
    positional: Vec<String>,
    flags: Vec<(String, Option<String>)>,
}

impl Args {
    fn parse(args: &[String], with_value: &[&str]) -> Self {
        let mut positional = Vec::new();
        let mut flags = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == "--" {
                positional.extend(it.by_ref().cloned());
                break;
            }
            if a.starts_with('-') && a.len() > 1 {
                let value = if with_value.contains(&a.as_str()) { it.next().cloned() } else { None };
                flags.push((a.clone(), value));
            } else {
                positional.push(a.clone());
            }
        }
        Self { positional, flags }
    }

    fn has(&self, names: &[&str]) -> bool {
        self.flags.iter().any(|(f, _)| names.contains(&f.as_str()))
    }

    fn value(&self, names: &[&str]) -> Option<&str> {
        self.flags.iter().find(|(f, _)| names.contains(&f.as_str())).and_then(|(_, v)| v.as_deref())
    }
}

pub fn run(cmd: &str, raw: &[String]) -> Result<String, String> {
    match cmd {
        "tabs" => Ok(pretty(&call(json!({ "cmd": "tabs" }))?)),
        "status" => {
            let tab = raw.first().ok_or("falta <tab>")?;
            Ok(pretty(&call(json!({ "cmd": "status", "tab": tab_spec(tab) }))?["tab"]))
        }
        "read" => {
            let a = Args::parse(raw, &["-n", "--lines"]);
            let tab = a.positional.first().ok_or("falta <tab>")?;
            let lines = a.value(&["-n", "--lines"]).map_or(Ok(50), |n| n.parse::<u64>().map_err(|_| "-n debe ser un número"))?;
            let v = call(json!({ "cmd": "read", "tab": tab_spec(tab), "lines": lines }))?;
            Ok(v["text"].as_str().unwrap_or_default().to_string())
        }
        "send" => {
            let a = Args::parse(raw, &[]);
            let (tab, words) = a.positional.split_first().ok_or("falta <tab>")?;
            let text = words.join(" ");
            let tab = tab_spec(tab);
            if !text.is_empty() {
                call(json!({ "cmd": "send", "tab": tab, "text": text }))?;
            }
            if a.has(&["--enter"]) {
                // Enter aparte: si llega pegado al texto, un TUI como Claude Code lo toma como parte de un pegado.
                if !text.is_empty() {
                    std::thread::sleep(Duration::from_millis(150));
                }
                call(json!({ "cmd": "send", "tab": tab, "text": "\r", "raw": true }))?;
            }
            Ok(String::new())
        }
        "new" => {
            let a = Args::parse(raw, &["--group", "-g", "--cwd"]);
            // Absoluta: la app resolvería una ruta relativa desde su propia carpeta.
            let cwd = match a.value(&["--cwd"]) {
                Some(d) => std::fs::canonicalize(d).map_err(|e| format!("{d}: {e}"))?,
                None => std::env::current_dir().map_err(|e| e.to_string())?,
            };
            let near = std::env::var("TELAR_TAB_ID").ok().and_then(|v| v.parse::<u64>().ok());
            let command = (!a.positional.is_empty()).then(|| a.positional.join(" "));
            let v = call(json!({
                "cmd": "new",
                "group": a.value(&["--group", "-g"]),
                "cwd": cwd,
                "near": near,
                "focus": a.has(&["--focus"]),
                "command": command,
            }))?;
            Ok(v["id"].to_string())
        }
        "group" => {
            let a = Args::parse(raw, &["--group", "-g", "--dir", "--name"]);
            // Rutas relativas, respecto a la carpeta de quien llama (la app no la conoce).
            let dir = match a.value(&["--dir"]) {
                Some("") => Some(String::new()),
                Some(d) => Some(std::fs::canonicalize(d).map_err(|e| format!("{d}: {e}"))?.display().to_string()),
                None => None,
            };
            let near = std::env::var("TELAR_TAB_ID").ok().and_then(|v| v.parse::<u64>().ok());
            let mut v = call(json!({
                "cmd": "group",
                "group": a.value(&["--group", "-g"]),
                "near": near,
                "dir": dir,
                "name": a.value(&["--name"]),
            }))?;
            v.as_object_mut().map(|o| o.remove("ok"));
            Ok(pretty(&v))
        }
        "suspend" | "resume" => {
            let a = Args::parse(raw, &["--group", "-g"]);
            let group = a.value(&["--group", "-g"]);
            let tab = match (group, a.positional.first()) {
                (Some(_), _) => Value::Null,
                (None, Some(t)) => tab_spec(t),
                (None, None) => return Err("falta <tab> o --group G".into()),
            };
            let v = call(json!({ "cmd": cmd, "tab": tab, "group": group, "force": a.has(&["--force"]) }))?;
            let n = v["changed"].as_array().map_or(0, Vec::len);
            Ok(match (cmd, n) {
                (_, 0) => "Nada que cambiar".to_string(),
                ("suspend", _) => format!("Suspendidos: {}", v["changed"]),
                _ => format!("Reanudados: {}", v["changed"]),
            })
        }
        "update" => {
            use crate::update::{self, Status};
            let status = update::latest()?;
            let Status::Available(release) = status else {
                return Ok(format!("Telar {} está al día", update::VERSION));
            };
            if raw.iter().any(|a| a == "--check") {
                return Ok(format!("Hay una versión nueva: {} (instalada: {})", release.version, update::VERSION));
            }
            let last = std::cell::Cell::new(-1i32);
            update::install_sync(&release, &|s| {
                let pct = (s.progress().unwrap_or(0.0) * 100.0) as i32;
                if pct / 10 != last.get() / 10 {
                    last.set(pct);
                    eprintln!("{pct:>3} %  {}", if matches!(s, Status::Applying(_)) { "aplicando" } else { "descargando" });
                }
            })?;
            Ok(format!("Telar {} instalado. Reinicia Telar para usarlo.", release.version))
        }
        "clear" => {
            let tab = raw.first().ok_or("falta <tab>")?;
            call(json!({ "cmd": "clear", "tab": tab_spec(tab) }))?;
            Ok(String::new())
        }
        "awake" => {
            let v = call(json!({ "cmd": "awake", "mode": raw.first() }))?;
            let mode = v["mode"].as_str().unwrap_or_default();
            let what = match mode {
                "on" => "siempre",
                "auto" => "mientras algún Claude trabaja",
                _ => "no",
            };
            let now = if v["active"] == json!(true) { "activo ahora" } else { "inactivo ahora" };
            Ok(format!("Despierto: {what} ({mode}); caffeinate {now}"))
        }
        "browser" => {
            // Lo usan las herramientas que abren un navegador para el agente (ver `browsers`).
            let a = Args::parse(raw, &["--name", "--pid", "--tab", "--app", "--label"]);
            let event = a.positional.first().ok_or("falta el evento: open, activity o close")?;
            let tab = tab_spec(a.value(&["--tab"]).unwrap_or("self"));
            let mut body = json!({ "cmd": "browser", "event": event, "tab": tab, "name": a.value(&["--name"]) });
            if let Some(pid) = a.value(&["--pid"]) {
                body["pid"] = json!(pid.parse::<i64>().map_err(|_| "--pid debe ser un número")?);
            }
            body["headless"] = json!(a.has(&["--headless"]));
            body["app"] = json!(a.value(&["--app"]));
            body["label"] = json!(a.value(&["--label"]));
            // Después de `--`: el comando para mostrarlo.
            if a.positional.len() > 1 {
                body["show"] = json!(a.positional[1..]);
            }
            call(body)?;
            Ok(String::new())
        }
        "wait" => {
            let a = Args::parse(raw, &["--state", "--timeout", "--match"]);
            let tab = tab_spec(a.positional.first().ok_or("falta <tab>")?);
            let target = a.value(&["--state"]).map(String::from);
            let timeout = a.value(&["--timeout"]).map_or(Ok(1800), |t| t.parse::<u64>().map_err(|_| "--timeout debe ser un número"))?;
            if let Some(pattern) = a.value(&["--match"]) {
                let options: Vec<&str> = pattern.split('|').map(str::trim).filter(|p| !p.is_empty()).collect();
                if options.is_empty() {
                    return Err("--match necesita un texto".into());
                }
                return wait_match(tab, &options, Duration::from_secs(timeout));
            }
            wait(tab, target, Duration::from_secs(timeout))
        }
        _ => Err(format!("Comando desconocido: {cmd}\n\n{USAGE}")),
    }
}

/// Sin `--state`: espera a que el tab deje de trabajar. Si en este momento no está
/// trabajando, espera primero a que cambie de estado (p. ej. justo después de un `send`).
fn wait(tab: Value, target: Option<String>, timeout: Duration) -> Result<String, String> {
    let start = Instant::now();
    let first = call(json!({ "cmd": "status", "tab": tab }))?;
    let initial_changes = first["tab"]["changes"].as_u64().unwrap_or(0);
    loop {
        let v = call(json!({ "cmd": "status", "tab": tab }))?;
        let state = v["tab"]["state"].as_str().unwrap_or_default().to_string();
        let changes = v["tab"]["changes"].as_u64().unwrap_or(0);
        let done = match &target {
            Some(t) => &state == t,
            None => state != "working" && (changes != initial_changes || first["tab"]["state"] == json!("working")),
        };
        if done {
            return Ok(state);
        }
        if start.elapsed() >= timeout {
            return Err(format!("tiempo agotado; estado actual: {state}"));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Líneas que se leen de cada vez para ver qué apareció.
const MATCH_WINDOW: u64 = 400;

/// Espera a que el tab muestre una línea nueva que contenga alguno de `options` y la
/// devuelve. Lo que ya estaba en pantalla al empezar no cuenta.
fn wait_match(tab: Value, options: &[&str], timeout: Duration) -> Result<String, String> {
    let start = Instant::now();
    let read = || -> Result<Vec<String>, String> {
        let v = call(json!({ "cmd": "read", "tab": tab, "lines": MATCH_WINDOW }))?;
        Ok(v["text"].as_str().unwrap_or_default().lines().map(String::from).collect())
    };
    let mut seen = read()?;
    loop {
        std::thread::sleep(Duration::from_millis(400));
        let now = read()?;
        if let Some(line) = new_lines(&seen, &now).iter().find(|l| options.iter().any(|o| l.contains(o))) {
            return Ok(line.trim().to_string());
        }
        seen = now;
        if start.elapsed() >= timeout {
            return Err(format!("tiempo agotado sin ver «{}»", options.join("» ni «")));
        }
    }
}

/// Lo que apareció en `now` respecto de `before` (las dos, las últimas líneas del tab). Lo
/// nuevo se agrega al final y lo viejo sale por arriba: se busca el desplazamiento con que
/// `before` sigue estando al comienzo de `now`. La última línea de `before` puede haber
/// crecido (se seguía escribiendo), así que se compara sin ella y se cuenta como nueva. Si
/// no hay coincidencia (se borró la pantalla), todo `now` es nuevo.
fn new_lines<'a>(before: &[String], now: &'a [String]) -> &'a [String] {
    let old = &before[..before.len().saturating_sub(1)];
    for drop in 0..=old.len() {
        let kept = &old[drop..];
        if now.len() >= kept.len() && now[..kept.len()] == *kept {
            let added = &now[kept.len()..];
            // La última línea solo es nueva si cambió.
            return match (added.first(), before.last()) {
                (Some(first), Some(last)) if first == last => &added[1..],
                _ => added,
            };
        }
    }
    now
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn lineas_nuevas() {
        let antes = lines(&["a", "b", "Compiled successfully", "> "]);
        // Lo viejo no cuenta: la línea que ya decía «Compiled successfully» queda fuera.
        let ahora = lines(&["a", "b", "Compiled successfully", "> cambio", "Compiled successfully"]);
        // La última línea creció («> » pasó a «> cambio»): cuenta como nueva.
        assert_eq!(new_lines(&antes, &ahora), lines(&["> cambio", "Compiled successfully"]));
        // Salieron líneas por arriba (la ventana de lectura es fija).
        let ahora = lines(&["Compiled successfully", "> ", "Failed to compile"]);
        assert_eq!(new_lines(&antes, &ahora), lines(&["Failed to compile"]));
        // Sin cambios: nada nuevo (tampoco la última línea, aunque diga lo que se espera).
        assert!(new_lines(&antes, &antes).is_empty());
        let fin = lines(&["a", "Compiled successfully"]);
        assert!(new_lines(&fin, &fin).is_empty());
        // Pantalla borrada: todo es nuevo.
        let ahora = lines(&["otra cosa"]);
        assert_eq!(new_lines(&antes, &ahora), lines(&["otra cosa"]));
    }

    fn args(v: &[&str], with_value: &[&str]) -> Args {
        Args::parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>(), with_value)
    }

    #[test]
    fn flags_y_posicionales() {
        let a = args(&["tab1", "-n", "20", "--enter", "hola"], &["-n"]);
        assert_eq!(a.positional, ["tab1", "hola"]);
        assert_eq!(a.value(&["-n", "--lines"]), Some("20"));
        assert!(a.has(&["--enter"]));
        assert!(!a.has(&["--focus"]));
    }

    #[test]
    fn todo_despues_de_doble_guion_es_posicional() {
        let a = args(&["--group", "api", "--", "claude", "--resume", "x"], &["--group"]);
        assert_eq!(a.value(&["--group", "-g"]), Some("api"));
        assert_eq!(a.positional, ["claude", "--resume", "x"]);
    }

    #[test]
    fn flag_con_valor_al_final_sin_valor() {
        let a = args(&["--dir"], &["--dir"]);
        assert!(a.has(&["--dir"]));
        assert_eq!(a.value(&["--dir"]), None);
    }

    #[test]
    fn tab_spec_numeros_y_texto() {
        assert_eq!(tab_spec("telar/2"), json!("telar/2"));
        assert_eq!(tab_spec("12"), json!("12"));
    }

    #[test]
    fn comando_desconocido() {
        assert!(run("nada", &[]).unwrap_err().starts_with("Comando desconocido"));
    }
}
