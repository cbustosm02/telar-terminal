//! Sesión guardada: grupos (con su carpeta), tabs, carpetas y sesiones de Claude Code.
//! Permite cerrar Telar y volver a abrirlo sin perder nada.
//!
//! Archivo: `~/.config/telar/sesion.json`.

use std::path::PathBuf;

use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub struct SavedTab {
    pub title: Option<String>,
    /// Nombre puesto por el usuario.
    pub name: Option<String>,
    pub cwd: Option<PathBuf>,
    /// `session_id` de Claude Code si en el tab había una sesión abierta.
    pub claude: Option<String>,
    /// Pantalla (texto ANSI) si el tab estaba suspendido: al reabrir sigue suspendido.
    pub screen: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SavedGroup {
    pub name: String,
    pub dir: Option<PathBuf>,
    pub active: usize,
    pub tabs: Vec<SavedTab>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Saved {
    pub active: usize,
    pub groups: Vec<SavedGroup>,
    /// Modo de `caffeinate` (`off`, `on`, `auto`).
    pub awake: String,
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
            "awake": self.awake,
            "groups": self.groups.iter().map(|g| json!({
                "name": g.name,
                "dir": g.dir,
                "active": g.active,
                "tabs": g.tabs.iter().map(|t| json!({
                    "title": t.title,
                    "name": t.name,
                    "cwd": t.cwd,
                    "claude_session": t.claude,
                    "suspended_screen": t.screen,
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
                        name: t["name"].as_str().map(String::from),
                        cwd: t["cwd"].as_str().map(PathBuf::from),
                        claude: t["claude_session"].as_str().map(String::from),
                        screen: t["suspended_screen"].as_str().map(String::from),
                    })
                    .collect();
                (!tabs.is_empty()).then(|| SavedGroup {
                    name: g["name"].as_str().unwrap_or("Grupo").to_string(),
                    dir: g["dir"].as_str().map(PathBuf::from),
                    active: g["active"].as_u64().unwrap_or(0) as usize,
                    tabs,
                })
            })
            .collect();
        Some(Self {
            active: v["active"].as_u64().unwrap_or(0) as usize,
            groups,
            awake: v["awake"].as_str().unwrap_or("off").to_string(),
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ejemplo() -> Saved {
        Saved {
            active: 1,
            awake: "auto".into(),
            groups: vec![
                SavedGroup {
                    name: "telar".into(),
                    dir: Some("/Users/x/telar".into()),
                    active: 1,
                    tabs: vec![
                        SavedTab { title: Some("✳ Refactor".into()), name: None, cwd: Some("/Users/x/telar".into()), claude: Some("ses-1".into()), screen: None },
                        SavedTab { title: None, name: Some("logs".into()), cwd: Some("/tmp".into()), claude: None, screen: Some("\x1b[31mhola".into()) },
                    ],
                },
                SavedGroup { name: "Grupo 2".into(), dir: None, active: 0, tabs: vec![SavedTab { title: None, name: None, cwd: None, claude: None, screen: None }] },
            ],
        }
    }

    #[test]
    fn ida_y_vuelta_json() {
        let s = ejemplo();
        assert_eq!(Saved::from_json(&s.to_json()), Some(s));
    }

    /// Un archivo de una versión anterior (sin dir, name, pantalla ni awake) se sigue leyendo.
    #[test]
    fn formato_anterior() {
        let v = serde_json::json!({
            "version": 1,
            "active": 0,
            "groups": [{ "name": "viejo", "active": 0, "tabs": [{ "title": "t", "cwd": "/tmp", "claude_session": "s" }] }],
        });
        let s = Saved::from_json(&v).unwrap();
        assert_eq!(s.awake, "off");
        assert_eq!(s.groups[0].dir, None);
        let t = &s.groups[0].tabs[0];
        assert_eq!((t.claude.as_deref(), t.name.as_deref(), t.screen.as_deref()), (Some("s"), None, None));
    }

    #[test]
    fn grupos_sin_tabs_se_descartan() {
        let v = serde_json::json!({ "groups": [{ "name": "vacío", "tabs": [] }, { "name": "ok", "tabs": [{}] }] });
        let s = Saved::from_json(&v).unwrap();
        assert_eq!(s.groups.len(), 1);
        assert_eq!(s.groups[0].name, "ok");
    }

    #[test]
    fn json_invalido() {
        assert_eq!(Saved::from_json(&serde_json::json!({ "groups": 3 })), None);
    }
}
