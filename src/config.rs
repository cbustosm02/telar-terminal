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
    /// Límite de memoria de Telar completo en MB (0: sin límite). Ver `limits`.
    pub ram_limit_mb: u64,
    /// Al pasar un límite, suspender tabs inactivos (si no, solo avisa).
    pub auto_suspend: bool,
    /// Suspensión por inactividad (ver `idle`).
    pub idle: crate::idle::Rules,
    /// Carpeta de inicio: donde abren los tabs y grupos nuevos (si el grupo no tiene la suya).
    pub start_dir: Option<PathBuf>,
    /// Barra lateral: el bloque de Claude (uso de la sesión y la semana, cuenta).
    pub show_claude: bool,
    /// Barra lateral: los medidores de consumo (CPU, RAM, GPU).
    pub show_meters: bool,
    /// Al abrir Telar, reanudar solo el tab visible: los demás quedan con su última
    /// pantalla y arrancan al mostrarlos (no se lanzan todos los Claude a la vez).
    pub lazy_restore: bool,
    /// Líneas de historial de cada tab (ver `SCROLLBACK_STEPS`).
    pub scrollback: usize,
    /// Telar remoto: la app lanza `telar bridge` (si hay sesión de Telar web en el Llavero).
    pub remote: bool,
    /// Servidor de Telar web (`None`: el de siempre, `bridge::DEFAULT_SERVER`).
    pub remote_server: Option<String>,
    /// Desde la web solo se mira: Telar rechaza escribir, responder, crear, cerrar,
    /// suspender y pegar.
    pub remote_view_only: bool,
}

/// Valores del historial por tab que se eligen con − / +.
pub const SCROLLBACK_STEPS: &[usize] = &[1_000, 2_000, 5_000, 10_000, 20_000, 50_000, 100_000];

/// Siguiente (o anterior) valor del historial.
pub fn step_scrollback(current: usize, delta: i32) -> usize {
    let i = SCROLLBACK_STEPS.iter().position(|&v| v >= current).unwrap_or(SCROLLBACK_STEPS.len() - 1) as i32;
    SCROLLBACK_STEPS[(i + delta).clamp(0, SCROLLBACK_STEPS.len() as i32 - 1) as usize]
}

impl Default for Config {
    fn default() -> Self {
        // Lo mismo que Ghostty por defecto.
        Self {
            theme: crate::theme::DEFAULT.into(),
            font_family: fontlist::DEFAULT_FAMILY.into(),
            font_size: 13.0,
            app_icon: crate::appicon::DEFAULT.into(),
            ram_limit_mb: 0,
            auto_suspend: true,
            idle: crate::idle::Rules::default(),
            start_dir: None,
            show_claude: true,
            show_meters: true,
            lazy_restore: true,
            scrollback: 10_000,
            remote: false,
            remote_server: None,
            remote_view_only: false,
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
            "general": {
                "start_dir": self.start_dir,
                "show_claude": self.show_claude,
                "show_meters": self.show_meters,
                "lazy_restore": self.lazy_restore,
                "scrollback_lines": self.scrollback,
            },
            "remote": {
                "enabled": self.remote,
                "server": self.remote_server,
                "view_only": self.remote_view_only,
            },
            "appearance": {
                "theme": self.theme,
                "font_family": self.font_family,
                "font_size": self.font_size,
                "app_icon": self.app_icon,
            },
            "performance": {
                "ram_limit_mb": self.ram_limit_mb,
                "auto_suspend": self.auto_suspend,
            },
            "inactivity": {
                "tab_minutes": self.idle.tab_min,
                "group_minutes": self.idle.group_min,
                "include_waiting": self.idle.include_waiting,
                "include_running": self.idle.include_running,
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
            ram_limit_mb: v["performance"]["ram_limit_mb"].as_u64().unwrap_or(d.ram_limit_mb),
            auto_suspend: v["performance"]["auto_suspend"].as_bool().unwrap_or(d.auto_suspend),
            idle: {
                let i = &v["inactivity"];
                crate::idle::Rules {
                    tab_min: i["tab_minutes"].as_u64().unwrap_or(d.idle.tab_min),
                    group_min: i["group_minutes"].as_u64().unwrap_or(d.idle.group_min),
                    include_waiting: i["include_waiting"].as_bool().unwrap_or(d.idle.include_waiting),
                    include_running: i["include_running"].as_bool().unwrap_or(d.idle.include_running),
                }
            },
            start_dir: v["general"]["start_dir"].as_str().map(PathBuf::from).filter(|p| p.is_absolute()),
            show_claude: v["general"]["show_claude"].as_bool().unwrap_or(d.show_claude),
            show_meters: v["general"]["show_meters"].as_bool().unwrap_or(d.show_meters),
            lazy_restore: v["general"]["lazy_restore"].as_bool().unwrap_or(d.lazy_restore),
            scrollback: v["general"]["scrollback_lines"].as_u64().map_or(d.scrollback, |n| (n as usize).clamp(SCROLLBACK_STEPS[0], 100_000)),
            remote: v["remote"]["enabled"].as_bool().unwrap_or(d.remote),
            remote_server: v["remote"]["server"].as_str().filter(|s| !s.is_empty()).map(String::from),
            remote_view_only: v["remote"]["view_only"].as_bool().unwrap_or(d.remote_view_only),
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
    fn por_defecto_jetbrains_mono() {
        let c = Config::default();
        assert_eq!((c.font_family.as_str(), c.font_size), ("JetBrains Mono", 13.0));
    }

    #[test]
    fn ida_y_vuelta() {
        let c = Config {
            theme: "Nord".into(),
            font_family: "Menlo".into(),
            font_size: 15.0,
            app_icon: "tejido".into(),
            ram_limit_mb: 4096,
            auto_suspend: false,
            idle: crate::idle::Rules { tab_min: 30, group_min: 120, include_waiting: true, include_running: false },
            start_dir: Some(PathBuf::from("/tmp/proyectos")),
            show_claude: false,
            show_meters: false,
            lazy_restore: false,
            scrollback: 50_000,
            remote: true,
            remote_server: Some("https://telar.ejemplo.dev".into()),
            remote_view_only: true,
        };
        assert_eq!(Config::from_json(&c.to_json()), c);
    }

    #[test]
    fn valores_invalidos_o_faltantes() {
        assert_eq!(Config::from_json(&json!({})), Config::default());
        let c = Config::from_json(&json!({ "appearance": { "font_family": " ", "font_size": 400 } }));
        assert_eq!(c.font_family, "JetBrains Mono");
        assert_eq!(c.font_size, FONT_SIZE_MAX);
        let c = Config::from_json(&json!({ "general": { "start_dir": "relativa/no" } }));
        assert_eq!(c.start_dir, None, "solo rutas absolutas");
        let c = Config::from_json(&json!({ "general": { "scrollback_lines": 5 } }));
        assert_eq!(c.scrollback, 1_000);
    }

    #[test]
    fn pasos_del_historial() {
        assert_eq!(step_scrollback(10_000, 1), 20_000);
        assert_eq!(step_scrollback(10_000, -1), 5_000);
        assert_eq!(step_scrollback(1_000, -1), 1_000);
        assert_eq!(step_scrollback(100_000, 1), 100_000);
        // Un valor escrito a mano entre dos pasos sigue al más cercano hacia arriba.
        assert_eq!(step_scrollback(7_000, 0), 10_000);
    }
}
