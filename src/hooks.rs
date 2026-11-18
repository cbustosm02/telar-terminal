//! Instala o quita la integración con Claude Code:
//! - hooks en `~/.claude/settings.json`;
//! - enlace `~/.local/bin/telar` al binario actual;
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
    ("Notification", false),
    ("Stop", false),
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
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
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
    let hooks = root.as_object_mut().unwrap().entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or("\"hooks\" no es un objeto")?;
    let mut added = Vec::new();
    for (event, matcher) in EVENTS {
        let list = hooks.entry(*event).or_insert_with(|| json!([]));
        let list = list.as_array_mut().ok_or(format!("hooks.{event} no es una lista"))?;
        if list.iter().any(is_ours) {
            continue;
        }
        let mut entry = json!({ "hooks": [{ "type": "command", "command": COMMAND, "timeout": 5 }] });
        if *matcher {
            entry.as_object_mut().unwrap().insert("matcher".into(), json!("*"));
        }
        list.push(entry);
        added.push(*event);
    }
    if added.is_empty() {
        return Ok(format!("Los hooks de Telar ya estaban en {}", path.display()));
    }
    save(&path, &root)?;
    Ok(format!("Hooks agregados en {}: {}", path.display(), added.join(", ")))
}

pub fn uninstall_hooks() -> Result<String, String> {
    let (path, mut root) = load()?;
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok("No había hooks de Telar".into());
    };
    let mut removed = 0;
    for (_, list) in hooks.iter_mut() {
        if let Some(list) = list.as_array_mut() {
            let before = list.len();
            list.retain(|e| !is_ours(e));
            removed += before - list.len();
        }
    }
    hooks.retain(|_, list| list.as_array().is_none_or(|l| !l.is_empty()));
    if removed == 0 {
        return Ok("No había hooks de Telar".into());
    }
    save(&path, &root)?;
    Ok(format!("Se quitaron {removed} hooks de Telar de {}", path.display()))
}
