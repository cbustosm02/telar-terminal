//! Configuración del usuario: `~/.config/telar/config.json`, agrupada por secciones
//! (las mismas del modal de configuración, ⌘,). Se guarda sola al cambiar algo.
//!
//! ```json
//! { "appearance": { "theme": "Telar", "font_family": "JetBrains Mono", "font_size": 13 } }
//! ```

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::fontlist;

pub const FONT_SIZE_MIN: f32 = 8.0;
pub const FONT_SIZE_MAX: f32 = 36.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Tema de colores (ver `theme::themes`).
    pub theme: String,
    /// Familia de la terminal.
    pub font_family: String,
    /// Tamaño en puntos (⌘0 vuelve a este).
    pub font_size: f32,
    /// Ícono de la app (`appicon::CHOICES` o «propio»).
    pub app_icon: String,
}

impl Default for Config {
    fn default() -> Self {
        // Lo mismo que cmux/Ghostty por defecto.
        Self {
            theme: crate::theme::DEFAULT.into(),
            font_family: fontlist::DEFAULT_FAMILY.into(),
            font_size: 13.0,
            app_icon: crate::appicon::DEFAULT.into(),
        }
    }
}

fn path() -> PathBuf {
    if let Some(p) = std::env::var_os("TELAR_CONFIG") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".config").join("telar").join("config.json")
}

impl Config {
    pub fn to_json(&self) -> Value {
        json!({
            "appearance": {
                "theme": self.theme,
                "font_family": self.font_family,
                "font_size": self.font_size,
                "app_icon": self.app_icon,
            },
        })
    }

    /// Lo que falte o no sea válido queda con su valor por defecto.
    pub fn from_json(v: &Value) -> Self {
        let d = Self::default();
        let a = &v["appearance"];
        Self {
            theme: a["theme"].as_str().filter(|s| !s.trim().is_empty()).map_or(d.theme, String::from),
            font_family: a["font_family"].as_str().filter(|s| !s.trim().is_empty()).map_or(d.font_family, String::from),
            font_size: a["font_size"].as_f64().map_or(d.font_size, |s| (s as f32).clamp(FONT_SIZE_MIN, FONT_SIZE_MAX)),
            app_icon: a["app_icon"].as_str().filter(|s| !s.trim().is_empty()).map_or(d.app_icon, String::from),
        }
    }

    pub fn load() -> Self {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .map_or_else(Self::default, |v| Self::from_json(&v))
    }

    /// Escritura atómica (temporal + rename).
    pub fn save(&self) {
        let path = path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(&self.to_json()).unwrap_or_default() + "\n";
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn por_defecto_como_cmux() {
        let c = Config::default();
        assert_eq!((c.font_family.as_str(), c.font_size), ("JetBrains Mono", 13.0));
    }

    #[test]
    fn ida_y_vuelta() {
        let c = Config { theme: "Nord".into(), font_family: "Menlo".into(), font_size: 15.0, app_icon: "tejido".into() };
        assert_eq!(Config::from_json(&c.to_json()), c);
    }

    #[test]
    fn valores_invalidos_o_faltantes() {
        assert_eq!(Config::from_json(&json!({})), Config::default());
        let c = Config::from_json(&json!({ "appearance": { "font_family": " ", "font_size": 400 } }));
        assert_eq!(c.font_family, "JetBrains Mono");
        assert_eq!(c.font_size, FONT_SIZE_MAX);
    }
}
