//! Enlaces en la pantalla de la terminal: los que el programa marca como hipervínculo
//! (OSC 8) y las URL escritas como texto (`https://…`, `http://localhost:4200/`,
//! `file://…`). Se buscan en lo visible cuando cambia la pantalla (ver `visible_links`), uniendo las filas que la
//! terminal partió por ancho (WRAPLINE), así una URL larga partida en dos se abre entera.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;

/// Un enlace en la vista: su URL y los tramos que ocupa (fila de pantalla, columna
/// inicial, columna final inclusive).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    pub spans: Vec<(usize, usize, usize)>,
}

impl Link {
    pub fn contains(&self, col: usize, row: usize) -> bool {
        self.spans.iter().any(|&(r, c0, c1)| r == row && (c0..=c1).contains(&col))
    }
}

const SCHEMES: &[&str] = &["https://", "http://", "file://"];

/// Esquemas que se abren al hacer clic (un OSC 8 podría traer cualquier otro).
pub fn openable(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    SCHEMES.iter().any(|s| lower.starts_with(s)) || lower.starts_with("mailto:")
}

fn is_url_char(c: char) -> bool {
    c.is_ascii_graphic() && !matches!(c, '<' | '>' | '"' | '\'' | '`' | '{' | '}' | '|' | '\\' | '^')
}

/// URL escritas como texto en `chars`: rangos `[inicio, fin)`.
pub fn find_urls(chars: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let scheme = SCHEMES.iter().find(|s| {
            let n = s.len();
            i + n <= chars.len() && chars[i..i + n].iter().zip(s.chars()).all(|(a, b)| a.to_ascii_lowercase() == b)
        });
        let Some(scheme) = scheme else {
            i += 1;
            continue;
        };
        // «xhttps://» no es una URL; «(https://» o «=https://» sí.
        if i > 0 && chars[i - 1].is_ascii_alphanumeric() {
            i += 1;
            continue;
        }
        let mut end = i + scheme.len();
        while end < chars.len() && is_url_char(chars[end]) {
            end += 1;
        }
        // Puntuación de la frase al final («…en https://x.co.») y paréntesis o corchetes
        // de cierre que no abrió la URL («(ver https://x.co)», Markdown).
        loop {
            let Some(&last) = chars[i..end].last() else { break };
            let count = |c: char| chars[i..end].iter().filter(|&&x| x == c).count();
            let unbalanced = match last {
                ')' => count('(') < count(')'),
                ']' => count('[') < count(']'),
                _ => false,
            };
            if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '*' | '_') || unbalanced {
                end -= 1;
            } else {
                break;
            }
        }
        if end > i + scheme.len() {
            out.push((i, end));
        }
        i = end.max(i + 1);
    }
    out
}

/// Enlaces de la pantalla visible de `term`.
pub fn scan<T>(term: &Term<T>) -> Vec<Link> {
    let grid = term.grid();
    let offset = grid.display_offset() as i32;
    let (rows, cols) = (term.screen_lines(), term.columns());
    let mut links = Vec::new();
    // Línea lógica: caracteres con su lugar en pantalla y el hipervínculo de cada celda.
    let mut chars: Vec<char> = Vec::new();
    let mut cells: Vec<(usize, usize)> = Vec::new();
    let mut uris: Vec<Option<String>> = Vec::new();
    for row in 0..rows {
        let line = &grid[Line(row as i32 - offset)];
        for col in 0..cols {
            let cell = &line[Column(col)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            chars.push(cell.c);
            cells.push((row, col));
            uris.push(cell.hyperlink().map(|h| h.uri().to_string()));
        }
        let wraps = line[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        if !wraps || row + 1 == rows {
            collect(&chars, &cells, &uris, &mut links);
            chars.clear();
            cells.clear();
            uris.clear();
        }
    }
    links
}

/// Enlaces de una línea lógica: primero los OSC 8; las URL de texto, si no se pisan con ellos.
fn collect(chars: &[char], cells: &[(usize, usize)], uris: &[Option<String>], out: &mut Vec<Link>) {
    let mut taken = vec![false; chars.len()];
    let mut i = 0;
    while i < uris.len() {
        let Some(uri) = &uris[i] else {
            i += 1;
            continue;
        };
        let mut j = i;
        while j < uris.len() && uris[j].as_ref() == Some(uri) {
            taken[j] = true;
            j += 1;
        }
        out.push(Link { url: uri.clone(), spans: spans(&cells[i..j]) });
        i = j;
    }
    for (a, b) in find_urls(chars) {
        if taken[a..b].iter().any(|t| *t) {
            continue;
        }
        out.push(Link { url: chars[a..b].iter().collect(), spans: spans(&cells[a..b]) });
    }
}

/// Celdas seguidas agrupadas por fila.
fn spans(cells: &[(usize, usize)]) -> Vec<(usize, usize, usize)> {
    let mut out: Vec<(usize, usize, usize)> = Vec::new();
    for &(r, c) in cells {
        match out.last_mut() {
            Some((lr, _, end)) if *lr == r && *end + 1 >= c => *end = c,
            _ => out.push((r, c, c)),
        }
    }
    out
}

/// Abre el enlace con la app predeterminada de macOS (el navegador, para http/https).
pub fn open(url: &str) {
    if !openable(url) {
        log::warn!("enlace con un esquema que no se abre: {url}");
        return;
    }
    let url = url.to_string();
    // En un hilo que espera al proceso, para no dejar zombis.
    std::thread::spawn(move || {
        let _ = std::process::Command::new("open").arg(&url).status();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(s: &str) -> Vec<String> {
        let chars: Vec<char> = s.chars().collect();
        find_urls(&chars).into_iter().map(|(a, b)| chars[a..b].iter().collect()).collect()
    }

    #[test]
    fn urls_en_texto() {
        assert_eq!(urls("abre https://github.com/cbustos/telar ahora"), vec!["https://github.com/cbustos/telar"]);
        assert_eq!(urls("➜ Local: http://localhost:4200/"), vec!["http://localhost:4200/"]);
        assert_eq!(urls("ver https://x.co/a?b=1&c=2#d."), vec!["https://x.co/a?b=1&c=2#d"], "sin el punto final");
        assert_eq!(urls("(ver https://x.co/a)"), vec!["https://x.co/a"], "sin el paréntesis que no abrió");
        assert_eq!(urls("https://es.wikipedia.org/wiki/Rust_(lenguaje)"), vec!["https://es.wikipedia.org/wiki/Rust_(lenguaje)"]);
        assert_eq!(urls("[docs](https://docs.rs/x)"), vec!["https://docs.rs/x"]);
        assert_eq!(urls("│ https://a.io/b │"), vec!["https://a.io/b"], "los bordes de una caja no son parte");
        assert_eq!(urls("'https://a.io' y \"http://b.io\""), vec!["https://a.io", "http://b.io"]);
        assert_eq!(urls("dos: https://a.io, https://b.io"), vec!["https://a.io", "https://b.io"]);
        assert_eq!(urls("file:///Users/x/informe.html"), vec!["file:///Users/x/informe.html"]);
        assert_eq!(urls("HTTPS://A.IO"), vec!["HTTPS://A.IO"]);
        assert!(urls("xhttps://a.io https:// nada").is_empty());
    }

    #[test]
    fn tramos_por_fila() {
        assert_eq!(spans(&[(0, 78, 0), (0, 79, 0), (1, 0, 0), (1, 1, 0)].map(|(r, c, _)| (r, c))), vec![(0, 78, 79), (1, 0, 1)]);
        let link = Link { url: "u".into(), spans: vec![(0, 78, 79), (1, 0, 1)] };
        assert!(link.contains(79, 0) && link.contains(0, 1) && !link.contains(2, 1) && !link.contains(77, 0));
    }

    #[test]
    fn esquemas_que_se_abren() {
        assert!(openable("https://a.io") && openable("HTTP://a.io") && openable("file:///tmp/x") && openable("mailto:a@b.co"));
        assert!(!openable("javascript:alert(1)") && !openable("x-apple.systempreferences:"));
    }
}
