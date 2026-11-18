//! Sesión guardada: grupos, tabs, carpetas y sesiones de Claude Code.
//! Permite cerrar Telar y volver a abrirlo sin perder nada.
//!
//! Archivo: `~/.config/telar/sesion.json`.

use std::path::PathBuf;

use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub struct SavedTab {
    pub title: Option<String>,
    pub cwd: Option<PathBuf>,
    /// `session_id` de Claude Code si en el tab había una sesión abierta.
    pub claude: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SavedGroup {
    pub name: String,
    pub active: usize,
    pub tabs: Vec<SavedTab>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Saved {
    pub active: usize,
    pub groups: Vec<SavedGroup>,
}

fn path() -> PathBuf {
    // Para pruebas: otra ubicación, sin tocar la sesión real.
    if let Some(p) = std::env::var_os("TELAR_SESSION_FILE") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".config").join("telar").join("sesion.json")
}

impl Saved {
    pub fn to_json(&self) -> Value {
        json!({
            "version": 1,
            "active": self.active,
            "groups": self.groups.iter().map(|g| json!({
                "name": g.name,
                "active": g.active,
                "tabs": g.tabs.iter().map(|t| json!({
                    "title": t.title,
                    "cwd": t.cwd,
                    "claude_session": t.claude,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    }

    fn from_json(v: &Value) -> Option<Self> {
        let groups = v["groups"]
            .as_array()?
            .iter()
            .filter_map(|g| {
                let tabs: Vec<SavedTab> = g["tabs"]
                    .as_array()?
                    .iter()
                    .map(|t| SavedTab {
                        title: t["title"].as_str().map(String::from),
                        cwd: t["cwd"].as_str().map(PathBuf::from),
                        claude: t["claude_session"].as_str().map(String::from),
                    })
                    .collect();
                (!tabs.is_empty()).then(|| SavedGroup {
                    name: g["name"].as_str().unwrap_or("Grupo").to_string(),
                    active: g["active"].as_u64().unwrap_or(0) as usize,
                    tabs,
                })
            })
            .collect();
        Some(Self { active: v["active"].as_u64().unwrap_or(0) as usize, groups })
    }
}

pub fn load() -> Option<Saved> {
    let text = std::fs::read_to_string(path()).ok()?;
    let saved = Saved::from_json(&serde_json::from_str(&text).ok()?)?;
    (!saved.groups.is_empty()).then_some(saved)
}

/// Escritura atómica (archivo temporal + rename): un corte a mitad no deja el archivo roto.
pub fn save(saved: &Saved) {
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(&saved.to_json()).unwrap_or_default();
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}
