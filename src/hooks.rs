//! Instala o quita la integración con Claude Code:
//! - hooks en `~/.claude/settings.json`;
//! - la app `/Applications/Telar.app` (copia de este binario; se actualiza sola);
//! - enlace `~/.local/bin/telar` al binario de la app;
//! - regla `~/.claude/rules/telar.md`, para que Claude sepa usar la CLI.
//!
//! El comando de cada hook no hace nada fuera de Telar (`TELAR_SOCKET` vacío),
//! así que es seguro tenerlo en la configuración global de Claude Code.
//! No toca los hooks que ya existan: solo agrega (o quita) los suyos.

use std::path::PathBuf;

use serde_json::{Value, json};

const COMMAND: &str = r#"[ -z "$TELAR_SOCKET" ] || "$TELAR_BIN" hook"#;
const MARK: &str = "$TELAR_BIN";

/// Eventos y si llevan `matcher` (los de herramientas).
const EVENTS: &[(&str, bool)] = &[
    ("SessionStart", false),
    ("UserPromptSubmit", false),
    ("PreToolUse", true),
    ("PostToolUse", true),
    // Una herramienta que falló: Claude sigue trabajando.
    ("PostToolUseFailure", true),
    ("Notification", false),
    ("Stop", false),
    // El turno terminó por un error (de la API, por el límite de uso…): no llega `Stop`.
    ("StopFailure", false),
    ("SessionEnd", false),
];

const RULE: &str = include_str!("../claude/telar.md");

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn settings_path() -> PathBuf {
    home().join(".claude").join("settings.json")
}

fn link_path() -> PathBuf {
    home().join(".local").join("bin").join("telar")
}

fn rule_path() -> PathBuf {
    home().join(".claude").join("rules").join("telar.md")
}

pub fn install() -> Result<String, String> {
    let mut report = vec![install_hooks()?];

    let link = link_path();
    let exe = crate::update::install_app()?;
    report.push(format!("App en {} (versión {})", crate::update::app_path().display(), crate::update::VERSION));
    match std::fs::symlink_metadata(&link) {
        Ok(m) if !m.file_type().is_symlink() => {
            report.push(format!("No se creó {}: ya existe un archivo que no es un enlace", link.display()));
        }
        _ => {
            let _ = std::fs::remove_file(&link);
            if let Some(dir) = link.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::os::unix::fs::symlink(&exe, &link).map_err(|e| e.to_string())?;
            report.push(format!("Enlace {} → {}", link.display(), exe.display()));
        }
    }

    let rule = rule_path();
    if let Some(dir) = rule.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&rule, RULE).map_err(|e| e.to_string())?;
    report.push(format!("Regla para Claude en {}", rule.display()));
    Ok(report.join("\n"))
}

pub fn uninstall() -> Result<String, String> {
    let mut report = vec![uninstall_hooks()?];
    let link = link_path();
    if std::fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink()) {
        let _ = std::fs::remove_file(&link);
        report.push(format!("Se quitó {}", link.display()));
    }
    if std::fs::remove_file(rule_path()).is_ok() {
        report.push(format!("Se quitó {}", rule_path().display()));
    }
    let app = crate::update::app_path();
    if std::fs::remove_dir_all(&app).is_ok() {
        report.push(format!("Se quitó {}", app.display()));
    }
    Ok(report.join("\n"))
}

fn is_ours(entry: &Value) -> bool {
    entry["hooks"]
        .as_array()
        .is_some_and(|hs| hs.iter().any(|h| h["command"].as_str().is_some_and(|c| c.contains(MARK))))
}

fn load() -> Result<(PathBuf, Value), String> {
    let path = settings_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".into());
    let root: Value = serde_json::from_str(&text).map_err(|e| format!("{} no es JSON válido: {e}", path.display()))?;
    if !root.is_object() {
        return Err(format!("{} no es un objeto JSON", path.display()));
    }
    Ok((path, root))
}

fn save(path: &PathBuf, root: &Value) -> Result<(), String> {
    if path.exists() {
        let backup = path.with_extension("json.telar-bak");
        std::fs::copy(path, &backup).map_err(|e| format!("no se pudo respaldar: {e}"))?;
    }
    let text = serde_json::to_string_pretty(root).map_err(|e| e.to_string())? + "\n";
    std::fs::write(path, text).map_err(|e| e.to_string())
}

pub fn install_hooks() -> Result<String, String> {
    let (path, mut root) = load()?;
    let added = add_hooks(&mut root)?;
    if make_async(&mut root) > 0 {
        save(&path, &root)?;
    }
    if added.is_empty() {
        return Ok(format!("Los hooks de Telar ya estaban en {}", path.display()));
    }
    save(&path, &root)?;
    Ok(format!("Hooks agregados en {}: {}", path.display(), added.join(", ")))
}

/// Si los hooks de Telar ya están instalados, agrega los de eventos que una versión
/// anterior no registraba (al actualizarse, Telar se pone al día solo). Sin instalar, no
/// toca nada: eso lo decide `telar install`.
pub fn upgrade_hooks() -> Result<Vec<&'static str>, String> {
    let (path, mut root) = load()?;
    if !has_ours(&root) {
        return Ok(Vec::new());
    }
    let added = add_hooks(&mut root)?;
    let made_async = make_async(&mut root);
    if !added.is_empty() || made_async > 0 {
        save(&path, &root)?;
    }
    if made_async > 0 {
        log::info!("hooks de Telar ahora asíncronos: {made_async}");
    }
    Ok(added)
}

/// Marca como asíncronos los hooks de Telar que una versión anterior instaló sin esa
/// marca (Claude esperaba a cada uno). Devuelve cuántos cambió.
fn make_async(root: &mut Value) -> usize {
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else { return 0 };
    let mut changed = 0;
    for list in hooks.values_mut().filter_map(Value::as_array_mut) {
        for entry in list.iter_mut().filter(|e| is_ours(e)) {
            for h in entry["hooks"].as_array_mut().into_iter().flatten() {
                let ours = h["command"].as_str().is_some_and(|c| c.contains(MARK));
                if ours && h["async"] != json!(true) {
                    if let Some(o) = h.as_object_mut() {
                        o.insert("async".into(), json!(true));
                        changed += 1;
                    }
                }
            }
        }
    }
    changed
}

/// Algún evento ya tiene el hook de Telar.
fn has_ours(root: &Value) -> bool {
    root["hooks"].as_object().is_some_and(|h| h.values().any(|l| l.as_array().is_some_and(|l| l.iter().any(is_ours))))
}

/// Agrega a `root` (el settings.json) un hook de Telar por evento, sin tocar los demás.
/// Devuelve los eventos agregados (vacío si ya estaban todos).
fn add_hooks(root: &mut Value) -> Result<Vec<&'static str>, String> {
    let hooks = root.as_object_mut().ok_or("settings.json no es un objeto")?.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or("\"hooks\" no es un objeto")?;
    let mut added = Vec::new();
    for (event, matcher) in EVENTS {
        let list = hooks.entry(*event).or_insert_with(|| json!([]));
        let list = list.as_array_mut().ok_or(format!("hooks.{event} no es una lista"))?;
        if list.iter().any(is_ours) {
            continue;
        }
        // Asíncrono: Claude lo lanza y sigue, sin esperar a que Telar reciba el aviso (con el
        // Mac cargado, arrancar el proceso puede tardar segundos).
        let mut entry = json!({ "hooks": [{ "type": "command", "command": COMMAND, "timeout": 5, "async": true }] });
        if *matcher {
            entry.as_object_mut().unwrap().insert("matcher".into(), json!("*"));
        }
        list.push(entry);
        added.push(*event);
    }
    Ok(added)
}

pub fn uninstall_hooks() -> Result<String, String> {
    let (path, mut root) = load()?;
    let removed = remove_hooks(&mut root);
    if removed == 0 {
        return Ok("No había hooks de Telar".into());
    }
    save(&path, &root)?;
    Ok(format!("Se quitaron {removed} hooks de Telar de {}", path.display()))
}

/// Quita de `root` solo los hooks de Telar; los eventos que quedan vacíos se borran.
fn remove_hooks(root: &mut Value) -> usize {
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else { return 0 };
    let mut removed = 0;
    for (_, list) in hooks.iter_mut() {
        if let Some(list) = list.as_array_mut() {
            let before = list.len();
            list.retain(|e| !is_ours(e));
            removed += before - list.len();
        }
    }
    if removed > 0 {
        hooks.retain(|_, list| list.as_array().is_none_or(|l| !l.is_empty()));
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ajeno() -> Value {
        json!({ "hooks": [{ "type": "command", "command": "~/.claude/idioma.sh" }] })
    }

    #[test]
    fn agrega_un_hook_por_evento_y_respeta_los_ajenos() {
        let mut root = json!({ "model": "opus", "hooks": { "Stop": [ajeno()] } });
        let added = add_hooks(&mut root).unwrap();
        assert_eq!(added.len(), EVENTS.len());
        assert_eq!(root["model"], "opus");
        let stop = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0], ajeno(), "el hook ajeno queda primero e intacto");
        assert_eq!(root["hooks"]["PreToolUse"][0]["matcher"], "*");
        assert!(root["hooks"]["Stop"][1].get("matcher").is_none());
    }

    #[test]
    fn instalar_dos_veces_no_duplica() {
        let mut root = json!({});
        add_hooks(&mut root).unwrap();
        let antes = root.clone();
        assert!(add_hooks(&mut root).unwrap().is_empty());
        assert_eq!(root, antes);
    }

    #[test]
    fn quitar_deja_solo_los_ajenos() {
        let mut root = json!({ "hooks": { "Stop": [ajeno()] } });
        add_hooks(&mut root).unwrap();
        assert_eq!(remove_hooks(&mut root), EVENTS.len());
        assert_eq!(root, json!({ "hooks": { "Stop": [ajeno()] } }));
        assert_eq!(remove_hooks(&mut root), 0);
    }

    #[test]
    fn se_pone_al_dia_solo_si_ya_estaba_instalado() {
        let mut root = json!({ "hooks": { "Stop": [ajeno()] } });
        assert!(!has_ours(&root), "sin instalar no se agrega nada");
        add_hooks(&mut root).unwrap();
        assert!(has_ours(&root));
        // Una instalación vieja, sin StopFailure: al ponerse al día se agrega solo ese.
        root["hooks"].as_object_mut().unwrap().remove("StopFailure");
        assert_eq!(add_hooks(&mut root).unwrap(), vec!["StopFailure"]);
    }

    #[test]
    fn los_hooks_son_asincronos() {
        let mut root = json!({ "hooks": { "Stop": [ajeno()] } });
        add_hooks(&mut root).unwrap();
        assert_eq!(root["hooks"]["Stop"][1]["hooks"][0]["async"], true);
        assert_eq!(make_async(&mut root), 0);
        // Una instalación vieja, sin `async`: se marca la suya y el hook ajeno no se toca.
        root["hooks"]["Stop"][1]["hooks"][0].as_object_mut().unwrap().remove("async");
        assert_eq!(make_async(&mut root), 1);
        assert_eq!(root["hooks"]["Stop"][1]["hooks"][0]["async"], true);
        assert_eq!(root["hooks"]["Stop"][0], ajeno());
    }

    #[test]
    fn errores_de_formato() {
        assert!(add_hooks(&mut json!([])).is_err());
        assert!(add_hooks(&mut json!({ "hooks": 1 })).is_err());
        assert!(add_hooks(&mut json!({ "hooks": { "Stop": {} } })).is_err());
    }

    /// El comando no hace nada fuera de Telar y llama al binario del tab.
    #[test]
    fn comando_del_hook() {
        assert!(COMMAND.starts_with(r#"[ -z "$TELAR_SOCKET" ] ||"#));
        assert!(COMMAND.contains(MARK));
    }
}
