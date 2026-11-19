//! Temas: paleta de la terminal (16 colores ANSI, fondo, texto, cursor, selección) y
//! colores de la interfaz, derivados de esa paleta para que todo combine.
//!
//! El tema activo se lee con `t()` desde cualquier parte del dibujo; `set` lo cambia.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use alacritty_terminal::term::color::COUNT;

pub type Rgba = [f32; 4];

pub const fn hex(c: u32) -> Rgba {
    [
        ((c >> 16) & 0xff) as f32 / 255.0,
        ((c >> 8) & 0xff) as f32 / 255.0,
        (c & 0xff) as f32 / 255.0,
        1.0,
    ]
}

/// Mezcla `a` hacia `b` (k = 0: a; k = 1: b).
pub fn mix(a: Rgba, b: Rgba, k: f32) -> Rgba {
    [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k, a[2] + (b[2] - a[2]) * k, 1.0]
}

pub fn alpha(c: Rgba, a: f32) -> Rgba {
    [c[0], c[1], c[2], a]
}

/// Paleta base de un tema (lo que define cada uno).
struct Scheme {
    name: &'static str,
    dark: bool,
    bg: u32,
    fg: u32,
    cursor: u32,
    selection: u32,
    accent: u32,
    ansi: [u32; 16],
}

const SCHEMES: &[Scheme] = &[
    Scheme {
        name: "Telar",
        dark: true,
        bg: 0x15171c,
        fg: 0xd6dae3,
        cursor: 0xe6b450,
        selection: 0x2f4a6e,
        accent: 0x6fa8f2,
        ansi: [
            0x1d2027, 0xe5677b, 0x9ccf7e, 0xe9c46a, 0x6fa8f2, 0xc792ea, 0x5cc8d4, 0xc8ccd4, //
            0x5c6370, 0xff8095, 0xb5e392, 0xf5d78a, 0x8cbcff, 0xd8a8f5, 0x7fdbe5, 0xf0f2f5,
        ],
    },
    Scheme {
        name: "Tokyo Night",
        dark: true,
        bg: 0x1a1b26,
        fg: 0xc0caf5,
        cursor: 0xc0caf5,
        selection: 0x283457,
        accent: 0x7aa2f7,
        ansi: [
            0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, //
            0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
        ],
    },
    Scheme {
        name: "Catppuccin Mocha",
        dark: true,
        bg: 0x1e1e2e,
        fg: 0xcdd6f4,
        cursor: 0xf5e0dc,
        selection: 0x45475a,
        accent: 0x89b4fa,
        ansi: [
            0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, //
            0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
        ],
    },
    Scheme {
        name: "Dracula",
        dark: true,
        bg: 0x282a36,
        fg: 0xf8f8f2,
        cursor: 0xf8f8f2,
        selection: 0x44475a,
        accent: 0xbd93f9,
        ansi: [
            0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2, //
            0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
        ],
    },
    Scheme {
        name: "Nord",
        dark: true,
        bg: 0x2e3440,
        fg: 0xd8dee9,
        cursor: 0xd8dee9,
        selection: 0x434c5e,
        accent: 0x88c0d0,
        ansi: [
            0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0, //
            0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
        ],
    },
    Scheme {
        name: "Gruvbox Dark",
        dark: true,
        bg: 0x282828,
        fg: 0xebdbb2,
        cursor: 0xebdbb2,
        selection: 0x504945,
        accent: 0x83a598,
        ansi: [
            0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984, //
            0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
        ],
    },
    Scheme {
        name: "One Dark",
        dark: true,
        bg: 0x282c34,
        fg: 0xabb2bf,
        cursor: 0x528bff,
        selection: 0x3e4451,
        accent: 0x61afef,
        ansi: [
            0x282c34, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, //
            0x5c6370, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff,
        ],
    },
    Scheme {
        name: "Rosé Pine",
        dark: true,
        bg: 0x191724,
        fg: 0xe0def4,
        cursor: 0x524f67,
        selection: 0x403d52,
        accent: 0xc4a7e7,
        ansi: [
            0x26233a, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4, //
            0x6e6a86, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
        ],
    },
    Scheme {
        name: "Catppuccin Latte",
        dark: false,
        bg: 0xeff1f5,
        fg: 0x4c4f69,
        cursor: 0xdc8a78,
        selection: 0xacb0be,
        accent: 0x1e66f5,
        ansi: [
            0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be, //
            0x6c6f85, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xbcc0cc,
        ],
    },
    Scheme {
        name: "GitHub Light",
        dark: false,
        bg: 0xffffff,
        fg: 0x1f2328,
        cursor: 0x0969da,
        selection: 0xb6e3ff,
        accent: 0x0969da,
        ansi: [
            0x24292f, 0xcf222e, 0x116329, 0x4d2d00, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781, //
            0x57606a, 0xa40e26, 0x1a7f37, 0x633c01, 0x218bff, 0xa475f9, 0x3192aa, 0x8c959f,
        ],
    },
];

/// Tema listo para dibujar.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub dark: bool,
    // Terminal.
    pub bg: Rgba,
    pub fg: Rgba,
    pub cursor: Rgba,
    pub selection: Rgba,
    pub ansi: [Rgba; 16],
    // Interfaz.
    /// Fondo de la barra lateral y la de tabs.
    pub chrome: Rgba,
    /// Fondo de menús y modales.
    pub surface: Rgba,
    pub border: Rgba,
    pub hover: Rgba,
    pub active: Rgba,
    pub accent: Rgba,
    /// Texto sobre el color de acento.
    pub on_accent: Rgba,
    pub text: Rgba,
    pub text_dim: Rgba,
    /// Indicador apagado, separadores suaves.
    pub muted: Rgba,
    pub attention: Rgba,
    pub done: Rgba,
    pub danger: Rgba,
}

impl Theme {
    fn from(s: &Scheme) -> Self {
        let (bg, fg) = (hex(s.bg), hex(s.fg));
        let black = [0.0, 0.0, 0.0, 1.0];
        let chrome = if s.dark { mix(bg, black, 0.22) } else { mix(bg, fg, 0.04) };
        let ansi = s.ansi.map(hex);
        let accent = hex(s.accent);
        Self {
            name: s.name,
            dark: s.dark,
            bg,
            fg,
            cursor: hex(s.cursor),
            selection: hex(s.selection),
            ansi,
            chrome,
            surface: if s.dark { mix(bg, fg, 0.03) } else { bg },
            border: mix(chrome, fg, if s.dark { 0.12 } else { 0.14 }),
            hover: mix(chrome, fg, 0.05),
            active: mix(chrome, fg, if s.dark { 0.10 } else { 0.09 }),
            accent,
            on_accent: if s.dark { mix(bg, black, 0.3) } else { [1.0, 1.0, 1.0, 1.0] },
            text: fg,
            text_dim: mix(fg, chrome, 0.42),
            muted: mix(fg, chrome, 0.7),
            attention: ansi[3],
            done: ansi[2],
            danger: ansi[1],
        }
    }

    /// Tabla completa de 269 colores con el mismo índice que usa alacritty_terminal:
    /// 0..16 ANSI, 16..232 cubo 6x6x6, 232..256 grises, 256 fg, 257 bg, 258 cursor,
    /// 259..267 atenuados, 267 fg brillante, 268 fg atenuado.
    pub fn palette(&self) -> [Rgba; COUNT] {
        let mut p = [[0.0; 4]; COUNT];
        p[..16].copy_from_slice(&self.ansi);
        let steps = [0u32, 95, 135, 175, 215, 255];
        for r in 0..6 {
            for g in 0..6 {
                for b in 0..6 {
                    p[16 + r * 36 + g * 6 + b] = hex((steps[r] << 16) | (steps[g] << 8) | steps[b]);
                }
            }
        }
        for i in 0..24u32 {
            let v = 8 + i * 10;
            p[232 + i as usize] = hex((v << 16) | (v << 8) | v);
        }
        p[256] = self.fg;
        p[257] = self.bg;
        p[258] = self.cursor;
        for i in 0..8 {
            p[259 + i] = mix(self.ansi[i], self.bg, 0.34);
        }
        p[267] = if self.dark { [1.0; 4] } else { self.fg };
        p[268] = mix(self.fg, self.bg, 0.34);
        p
    }
}

fn all() -> &'static [Theme] {
    static THEMES: OnceLock<Vec<Theme>> = OnceLock::new();
    THEMES.get_or_init(|| SCHEMES.iter().map(Theme::from).collect())
}

static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// Tema activo.
pub fn t() -> &'static Theme {
    &all()[CURRENT.load(Ordering::Relaxed)]
}

pub fn themes() -> &'static [Theme] {
    all()
}

/// Activa el tema `name` (sin distinguir mayúsculas). Devuelve `false` si no existe.
pub fn set(name: &str) -> bool {
    match all().iter().position(|th| th.name.eq_ignore_ascii_case(name)) {
        Some(i) => {
            CURRENT.store(i, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

pub const DEFAULT: &str = "Telar";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temas_disponibles() {
        let names: Vec<_> = themes().iter().map(|t| t.name).collect();
        assert_eq!(names[0], DEFAULT);
        assert!(names.contains(&"Tokyo Night") && names.contains(&"GitHub Light"));
        assert!(themes().iter().any(|t| !t.dark), "hay temas claros");
    }

    #[test]
    fn paleta_con_indices_de_alacritty() {
        let th = &themes()[1];
        let p = th.palette();
        assert_eq!(p[1], th.ansi[1]);
        assert_eq!(p[256], th.fg);
        assert_eq!(p[257], th.bg);
        assert_eq!(p[16], hex(0x000000));
        assert_eq!(p[231], hex(0xffffff));
    }

    /// El texto de la interfaz se distingue del fondo en todos los temas.
    #[test]
    fn contraste_minimo() {
        let lum = |c: Rgba| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        for th in themes() {
            assert!((lum(th.text) - lum(th.chrome)).abs() > 0.4, "{}", th.name);
            assert!((lum(th.text_dim) - lum(th.chrome)).abs() > 0.15, "{}", th.name);
        }
    }

    #[test]
    fn mezcla() {
        assert_eq!(mix(hex(0x000000), hex(0xffffff), 0.5), [0.5, 0.5, 0.5, 1.0]);
        assert!(!set("No existe"));
    }
}
