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
    /// Con `lazy`, la última pantalla de un tab abierto (se guarda al cerrar Telar).
    pub screen: Option<String>,
    /// El tab estaba abierto (no lo suspendiste tú): al reabrir Telar arranca recién al
    /// mostrarlo, con su pantalla de vista previa mientras tanto.
    pub lazy: bool,
    /// Comando a volver a ejecutar al reanudarlo (una TUI, un servidor…).
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SavedGroup {
    pub name: String,
    pub dir: Option<PathBuf>,
    /// Límite de memoria en MB (0: sin límite).
    pub ram_limit_mb: u64,
    /// Excluido de la suspensión por inactividad.
    pub idle_exempt: bool,
    pub active: usize,
    pub tabs: Vec<SavedTab>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Saved {
    pub active: usize,
    pub groups: Vec<SavedGroup>,
    /// Modo de `caffeinate` (`off`, `on`, `auto`).
    pub awake: String,
    /// Dónde estaba la ventana, para abrirla igual.
    pub window: Option<WindowFrame>,
}

/// Posición y tamaño de la ventana en puntos (las coordenadas globales de macOS, iguales en
/// todas las pantallas aunque tengan distinta escala), y si estaba maximizada o en
/// pantalla completa.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WindowFrame {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub maximized: bool,
    pub fullscreen: bool,
}

impl WindowFrame {
    fn to_json(self) -> Value {
        json!({ "x": self.x, "y": self.y, "w": self.w, "h": self.h, "maximized": self.maximized, "fullscreen": self.fullscreen })
    }

    fn from_json(v: &Value) -> Option<Self> {
        let n = |k: &str| v[k].as_f64().filter(|x| x.is_finite());
        let frame = Self {
            x: n("x")?,
            y: n("y")?,
            w: n("w")?,
            h: n("h")?,
            maximized: v["maximized"].as_bool().unwrap_or(false),
            fullscreen: v["fullscreen"].as_bool().unwrap_or(false),
        };
        (frame.w >= 100.0 && frame.h >= 100.0).then_some(frame)
    }

    /// Si se ve en alguna de estas pantallas (rectángulos en puntos): al menos un trozo de
    /// la barra de título, para poder agarrarla. Si no (se desconectó el monitor), la
    /// ventana abre donde macOS la ponga.
    pub fn visible_on(&self, screens: &[(f64, f64, f64, f64)]) -> bool {
        let (left, right, top) = (self.x, self.x + self.w, self.y);
        screens.iter().any(|&(sx, sy, sw, sh)| right - 80.0 > sx && left + 80.0 < sx + sw && top >= sy - 1.0 && top + 30.0 <= sy + sh)
    }
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
            "window": self.window.map(WindowFrame::to_json),
            "groups": self.groups.iter().map(|g| json!({
                "name": g.name,
                "dir": g.dir,
                "ram_limit_mb": g.ram_limit_mb,
                "idle_exempt": g.idle_exempt,
                "active": g.active,
                "tabs": g.tabs.iter().map(|t| json!({
                    "title": t.title,
                    "name": t.name,
                    "cwd": t.cwd,
                    "claude_session": t.claude,
                    "suspended_screen": t.screen,
                    "suspended_command": t.command,
                    "lazy": t.lazy,
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
                        command: t["suspended_command"].as_str().map(String::from),
                        lazy: t["lazy"].as_bool().unwrap_or(false),
                    })
                    .collect();
                (!tabs.is_empty()).then(|| SavedGroup {
                    name: g["name"].as_str().unwrap_or("Grupo").to_string(),
                    dir: g["dir"].as_str().map(PathBuf::from),
                    ram_limit_mb: g["ram_limit_mb"].as_u64().unwrap_or(0),
                    idle_exempt: g["idle_exempt"].as_bool().unwrap_or(false),
                    active: g["active"].as_u64().unwrap_or(0) as usize,
                    tabs,
                })
            })
            .collect();
        Some(Self {
            active: v["active"].as_u64().unwrap_or(0) as usize,
            groups,
            awake: v["awake"].as_str().unwrap_or("off").to_string(),
            window: WindowFrame::from_json(&v["window"]),
        })
    }
}

/// Solo la ventana (se necesita antes de crearla, antes de restaurar los tabs).
pub fn load_window() -> Option<WindowFrame> {
    let text = std::fs::read_to_string(path()).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    WindowFrame::from_json(&v["window"])
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
            window: Some(WindowFrame { x: -1440.0, y: 25.0, w: 1200.0, h: 780.0, maximized: false, fullscreen: true }),
            groups: vec![
                SavedGroup {
                    name: "telar".into(),
                    dir: Some("/Users/x/telar".into()),
                    ram_limit_mb: 2048,
                    idle_exempt: true,
                    active: 1,
                    tabs: vec![
                        SavedTab { title: Some("✳ Refactor".into()), name: None, cwd: Some("/Users/x/telar".into()), claude: Some("ses-1".into()), screen: Some("\x1b[1mhola".into()), command: None, lazy: true },
                        SavedTab { title: None, name: Some("logs".into()), cwd: Some("/tmp".into()), claude: None, screen: Some("\x1b[31mhola".into()), command: Some("htop -d 10".into()), lazy: false },
                    ],
                },
                SavedGroup { name: "Grupo 2".into(), dir: None, ram_limit_mb: 0, idle_exempt: false, active: 0, tabs: vec![SavedTab { title: None, name: None, cwd: None, claude: None, screen: None, command: None, lazy: false }] },
            ],
        }
    }

    #[test]
    fn ventana_visible_o_no() {
        let main = (0.0, 0.0, 1512.0, 982.0);
        let left = (-1920.0, -100.0, 1920.0, 1080.0);
        let f = |x: f64, y: f64| WindowFrame { x, y, w: 1200.0, h: 780.0, ..Default::default() };
        assert!(f(100.0, 40.0).visible_on(&[main]));
        // En el monitor de la izquierda: visible si está conectado, si no, no.
        assert!(f(-1500.0, 0.0).visible_on(&[main, left]));
        assert!(!f(-1500.0, 0.0).visible_on(&[main]));
        // La barra de título por encima o por debajo de todas las pantallas: no.
        assert!(!f(100.0, 2000.0).visible_on(&[main]));
        // Tamaños absurdos se descartan al leer.
        assert_eq!(WindowFrame::from_json(&json!({"x":0,"y":0,"w":10,"h":10})), None);
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
