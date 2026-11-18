//! Colores del tema: paleta ANSI de la terminal y colores de la interfaz.

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

// Interfaz (barra lateral y barra de tabs).
pub const CHROME_BG: Rgba = hex(0x101216);
pub const BORDER: Rgba = hex(0x22252c);
pub const ITEM_ACTIVE: Rgba = hex(0x232731);
pub const ITEM_HOVER: Rgba = hex(0x1a1d23);
pub const ACCENT: Rgba = hex(0x6fa8f2);
pub const TEXT: Rgba = hex(0xd6dae3);
pub const TEXT_DIM: Rgba = hex(0x7d8390);
pub const ATTENTION: Rgba = hex(0xe6b450);
pub const SELECTION: Rgba = hex(0x2f4a6e);

// Terminal.
pub const TERM_BG: u32 = 0x15171c;
pub const TERM_FG: u32 = 0xd6dae3;
pub const CURSOR: u32 = 0xe6b450;

const ANSI: [u32; 16] = [
    0x1d2027, 0xe5677b, 0x9ccf7e, 0xe9c46a, 0x6fa8f2, 0xc792ea, 0x5cc8d4, 0xc8ccd4, // normales
    0x5c6370, 0xff8095, 0xb5e392, 0xf5d78a, 0x8cbcff, 0xd8a8f5, 0x7fdbe5, 0xf0f2f5, // brillantes
];

/// Tabla completa de 269 colores con el mismo índice que usa alacritty_terminal:
/// 0..16 ANSI, 16..232 cubo 6x6x6, 232..256 grises, 256 fg, 257 bg, 258 cursor,
/// 259..267 atenuados, 267 fg brillante, 268 fg atenuado.
pub fn palette() -> [Rgba; COUNT] {
    let mut p = [[0.0; 4]; COUNT];
    for (i, c) in ANSI.iter().enumerate() {
        p[i] = hex(*c);
    }
    let steps = [0u32, 95, 135, 175, 215, 255];
    for r in 0..6 {
        for g in 0..6 {
            for b in 0..6 {
                let idx = 16 + r * 36 + g * 6 + b;
                p[idx] = hex((steps[r] << 16) | (steps[g] << 8) | steps[b]);
            }
        }
    }
    for i in 0..24u32 {
        let v = 8 + i * 10;
        p[232 + i as usize] = hex((v << 16) | (v << 8) | v);
    }
    p[256] = hex(TERM_FG);
    p[257] = hex(TERM_BG);
    p[258] = hex(CURSOR);
    for i in 0..8 {
        p[259 + i] = dim(p[i]);
    }
    p[267] = hex(0xffffff);
    p[268] = dim(hex(TERM_FG));
    p
}

fn dim(c: Rgba) -> Rgba {
    [c[0] * 0.66, c[1] * 0.66, c[2] * 0.66, 1.0]
}
