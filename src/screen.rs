//! Copia de la pantalla visible de una terminal como texto ANSI (caracteres, colores y estilos).
//!
//! La usa la suspensión: al suspender un tab se guarda su pantalla, se cierra el proceso
//! y la vista previa se reconstruye pasando ese texto por el parser a una `Term` sin PTY.
//! Como es texto, también se guarda en la sesión y sobrevive al cerrar Telar.

use std::fmt::Write;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, StdSyncHandler};

/// Parámetros SGR de un color (`fg` = primer plano).
fn sgr_color(out: &mut String, color: Color, fg: bool) {
    let base = if fg { 30 } else { 40 };
    match color {
        Color::Named(n) => {
            let n = n as usize;
            match n {
                0..=7 => write!(out, ";{}", base + n),
                8..=15 => write!(out, ";{}", base + 60 + n - 8),
                // Foreground, Background y las variantes dim: el color por defecto.
                _ => write!(out, ";{}", base + 9),
            }
        }
        Color::Indexed(i) => write!(out, ";{};5;{i}", base + 8),
        Color::Spec(c) => write!(out, ";{};2;{};{};{}", base + 8, c.r, c.g, c.b),
    }
    .ok();
}

const STYLE: Flags = Flags::BOLD
    .union(Flags::DIM)
    .union(Flags::ITALIC)
    .union(Flags::ALL_UNDERLINES)
    .union(Flags::INVERSE)
    .union(Flags::HIDDEN)
    .union(Flags::STRIKEOUT);

fn sgr(out: &mut String, cell: &Cell) {
    out.push_str("\x1b[0");
    let f = cell.flags;
    for (flag, code) in [
        (Flags::BOLD, 1),
        (Flags::DIM, 2),
        (Flags::ITALIC, 3),
        (Flags::INVERSE, 7),
        (Flags::HIDDEN, 8),
        (Flags::STRIKEOUT, 9),
    ] {
        if f.contains(flag) {
            write!(out, ";{code}").ok();
        }
    }
    if f.intersects(Flags::ALL_UNDERLINES) {
        out.push_str(";4");
    }
    sgr_color(out, cell.fg, true);
    sgr_color(out, cell.bg, false);
    out.push('m');
}

/// Celda que no aporta nada al final de una fila (espacio sin fondo ni estilo).
fn blank(cell: &Cell) -> bool {
    cell.c == ' '
        && cell.bg == Color::Named(NamedColor::Background)
        && !cell.flags.intersects(Flags::INVERSE | Flags::ALL_UNDERLINES | Flags::STRIKEOUT)
}

/// Pantalla visible (sin scrollback) como texto ANSI.
pub fn to_ansi<L: EventListener>(term: &Term<L>) -> String {
    to_ansi_view(term, 0)
}

/// Como `to_ansi`, pero de la vista desplazada `offset` líneas hacia el historial (lo que
/// se ve tras subir con la rueda; 0 es el final).
pub fn to_ansi_view<L: EventListener>(term: &Term<L>, offset: usize) -> String {
    let grid = term.grid();
    let offset = offset.min(grid.history_size()) as i32;
    let (rows, cols) = (grid.screen_lines(), grid.columns());
    let mut out = String::from("\x1b[0m\x1b[H\x1b[2J");
    // Sin filas vacías al final.
    let last_row = (0..rows)
        .rev()
        .find(|&r| (0..cols).any(|c| !blank(&grid[Line(r as i32 - offset)][Column(c)])))
        .unwrap_or(0);
    let mut style: Option<(Color, Color, Flags)> = None;
    for r in 0..=last_row {
        let row = &grid[Line(r as i32 - offset)];
        let end = (0..cols).rev().find(|&c| !blank(&row[Column(c)])).map_or(0, |c| c + 1);
        for c in 0..end {
            let cell = &row[Column(c)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let key = (cell.fg, cell.bg, cell.flags & STYLE);
            if style != Some(key) {
                sgr(&mut out, cell);
                style = Some(key);
            }
            out.push(cell.c);
            if let Some(extra) = cell.zerowidth() {
                out.extend(extra);
            }
        }
        if r < last_row {
            // Fondo por defecto antes de saltar: si no, el resto de la fila se pintaría.
            if style.is_some_and(|(_, bg, _)| bg != Color::Named(NamedColor::Background)) {
                out.push_str("\x1b[0m");
                style = None;
            }
            out.push_str("\r\n");
        }
    }
    // Sin cursor: es una imagen fija.
    out.push_str("\x1b[0m\x1b[?25l");
    out
}

/// Escribe en `term` una pantalla guardada con `to_ansi`.
pub fn load<L: EventListener>(term: &mut Term<L>, ansi: &str) {
    let mut parser = Processor::<StdSyncHandler>::new();
    parser.advance(term, ansi.as_bytes());
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;

    use super::*;
    use crate::terminal::GridSize;

    fn term(cols: usize, rows: usize) -> Term<VoidListener> {
        let size = GridSize { cols, rows, cell_w: 8, cell_h: 16 };
        Term::new(Config::default(), &size, VoidListener)
    }

    /// Lo que se ve en pantalla (carácter, colores y estilo de cada celda) sobrevive a
    /// `to_ansi` + `load`.
    #[test]
    fn ida_y_vuelta() {
        let mut a = term(30, 6);
        load(
            &mut a,
            "\x1b[31mrojo \x1b[1;32mverde\x1b[0m \x1b[44;97m azul \x1b[0m\r\n\
             \x1b[38;5;208mnaranja\x1b[0m \x1b[38;2;10;20;30mrgb\x1b[0m \x1b[3;4mital\x1b[0m\r\n\
             ancho: 日本 ✓\r\n\x1b[7minverso\x1b[0m \x1b[9mtachado\x1b[0m",
        );
        let mut b = term(30, 6);
        load(&mut b, &to_ansi(&a));
        for r in 0..6 {
            for c in 0..30 {
                let (x, y) = (&a.grid()[Line(r)][Column(c)], &b.grid()[Line(r)][Column(c)]);
                assert_eq!((x.c, x.fg, x.bg, x.flags & (STYLE | Flags::WIDE_CHAR)), (y.c, y.fg, y.bg, y.flags & (STYLE | Flags::WIDE_CHAR)), "fila {r}, columna {c}");
            }
        }
        assert!(!b.mode().contains(alacritty_terminal::term::TermMode::SHOW_CURSOR));
    }

    /// La vista desplazada muestra el historial: 10 líneas en una pantalla de 3, subiendo 4.
    #[test]
    fn vista_desplazada() {
        let mut a = term(10, 3);
        let text: Vec<String> = (1..=10).map(|n| format!("linea {n}")).collect();
        load(&mut a, &text.join("\r\n"));
        let plain = |ansi: &str| {
            let mut b = term(10, 3);
            load(&mut b, ansi);
            (0..3).map(|r| (0..10).map(|c| b.grid()[Line(r)][Column(c)].c).collect::<String>().trim_end().to_string()).collect::<Vec<_>>()
        };
        assert_eq!(plain(&to_ansi(&a)), ["linea 8", "linea 9", "linea 10"]);
        assert_eq!(plain(&to_ansi_view(&a, 4)), ["linea 4", "linea 5", "linea 6"]);
        // Más allá del historial, se queda en lo más viejo.
        assert_eq!(plain(&to_ansi_view(&a, 99)), ["linea 1", "linea 2", "linea 3"]);
    }
}
