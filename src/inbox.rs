//! Bandeja de agentes (⌘⇧J o el botón junto a «GRUPOS»): todas las sesiones de Claude
//! Code de todos los grupos en un panel, de la más urgente a la menos. Cada una muestra
//! dónde está, en qué estado y desde hace cuánto, y la pregunta pendiente o su último
//! paso. Un permiso pendiente se responde desde aquí con las opciones que muestra Claude
//! (1, 2, 3…), sin cambiar de tab.

use std::collections::HashMap;

use crate::font::Fonts;
use crate::kit::{Kit, Text, Variant, icons};
use crate::renderer::Quad;
use crate::terminal::{AgentState, Terminal};
use crate::theme;
use crate::ui::{Layout, Rect};
use crate::workspace::Workspace;

const PANEL_W: f32 = 760.0;
const PANEL_H: f32 = 640.0;
const ROW_H: f32 = 98.0;
const BUTTON_H: f32 = 26.0;
/// Líneas del final de la pantalla donde se buscan la pregunta y sus opciones.
const SCAN_LINES: usize = 40;
const MAX_OPTIONS: usize = 4;

/// Una sesión de Claude en la bandeja.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub tab: u64,
    /// «grupo › título».
    pub place: String,
    pub state: AgentState,
    pub secs: u64,
    /// La pregunta (te necesita) o el último paso (la última línea con ⏺).
    pub detail: Option<String>,
    /// Opciones de un permiso pendiente: la tecla y su texto.
    pub options: Vec<(char, String)>,
    /// Comandos de shell que lanzó Claude y siguen corriendo (ver `proc::claude_shells`).
    pub shells: usize,
}

impl Entry {
    /// En espera, pero con comandos de shell suyos corriendo (un servidor, una espera…).
    pub fn waiting_shell(&self) -> bool {
        self.state == AgentState::Idle && self.shells > 0
    }
}

/// Qué tan arriba va: primero lo que te necesita, después lo que terminó, lo que trabaja,
/// lo que espera una shell y lo que solo espera.
fn rank(e: &Entry) -> u8 {
    match e.state {
        AgentState::NeedsYou => 0,
        AgentState::Done => 1,
        AgentState::Working => 2,
        _ if e.waiting_shell() => 3,
        _ => 4,
    }
}

/// Las sesiones de Claude abiertas (no las suspendidas), ordenadas por urgencia y, dentro
/// de cada estado, la que lleva más tiempo esperando primero.
pub fn entries(ws: &Workspace, terms: &HashMap<u64, Terminal>) -> Vec<Entry> {
    let mut out = Vec::new();
    for g in &ws.groups {
        for (i, id) in g.tabs.iter().enumerate() {
            let Some(t) = terms.get(id) else { continue };
            if t.agent.state == AgentState::None || t.is_suspended() {
                continue;
            }
            let title = t.label().map(String::from).unwrap_or_else(|| format!("Tab {}", i + 1));
            let lines: Vec<String> = t.text(SCAN_LINES).lines().map(String::from).collect();
            let needs = t.agent.state == AgentState::NeedsYou;
            let options = if needs { options(&lines) } else { Vec::new() };
            let detail = if needs {
                question(&lines, !options.is_empty()).or_else(|| t.agent.message.clone())
            } else {
                last_step(&lines)
            };
            out.push(Entry {
                tab: *id,
                place: format!("{} › {title}", g.name),
                state: t.agent.state,
                secs: t.agent.since.elapsed().as_secs(),
                detail,
                options,
                shells: t.claude_shells,
            });
        }
    }
    out.sort_by(|a, b| rank(a).cmp(&rank(b)).then(b.secs.cmp(&a.secs)));
    out
}

/// Bordes de caja y espacios a los lados de una línea («│ texto │» → «texto»).
fn unboxed(line: &str) -> &str {
    line.trim_matches(|c: char| c.is_whitespace() || "│╭╮╰╯─".contains(c))
}

/// Una línea de opción de Claude («❯ 1. Yes», «  2. No»): su tecla y su texto.
fn option_line(line: &str) -> Option<(char, String)> {
    let rest = unboxed(line).trim_start_matches(['❯', '›', '>']).trim_start();
    let mut chars = rest.chars();
    let digit = chars.next().filter(|c| c.is_ascii_digit() && *c != '0')?;
    let text = chars.as_str().strip_prefix(". ")?.trim();
    (!text.is_empty()).then(|| (digit, text.to_string()))
}

/// Opciones del permiso pendiente: el último bloque de líneas numeradas 1, 2, 3… de la
/// pantalla (las más recientes; una lista vieja más arriba no cuenta).
pub fn options(lines: &[String]) -> Vec<(char, String)> {
    let mut found: Vec<(usize, (char, String))> = lines.iter().enumerate().filter_map(|(i, l)| Some((i, option_line(l)?))).collect();
    // El bloque empieza en la última «1.».
    let Some(start) = found.iter().rposition(|(_, (d, _))| *d == '1') else { return Vec::new() };
    found.drain(..start);
    let mut out: Vec<(char, String)> = Vec::new();
    for (_, (digit, text)) in found {
        // Seguidas: 1, 2, 3… (otra numeración corta el bloque).
        if digit as u8 != b'1' + out.len() as u8 {
            break;
        }
        out.push((digit, text));
    }
    if out.len() < 2 {
        return Vec::new();
    }
    out.truncate(MAX_OPTIONS);
    out
}

/// La pregunta del permiso: la última línea con texto antes de las opciones (o, sin
/// opciones, la última línea que termina en «?»).
fn question(lines: &[String], with_options: bool) -> Option<String> {
    let clean = |l: &String| unboxed(l).to_string();
    if with_options {
        let first = lines.iter().rposition(|l| option_line(l).is_some_and(|(d, _)| d == '1'))?;
        return lines[..first].iter().rev().map(clean).find(|l| !l.is_empty());
    }
    lines.iter().rev().map(clean).find(|l| l.ends_with('?'))
}

/// El último paso de Claude: la última línea que empieza con ⏺ (sin el símbolo).
fn last_step(lines: &[String]) -> Option<String> {
    lines.iter().rev().find_map(|l| l.trim_start().strip_prefix('⏺').map(|s| s.trim().to_string())).filter(|s| !s.is_empty())
}

/// «12 s», «3 min», «2 h 5 min».
pub fn ago(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min", s / 60),
        s => {
            let (h, m) = (s / 3600, s % 3600 / 60);
            if m == 0 { format!("{h} h") } else { format!("{h} h {m} min") }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Outside,
    Inside,
    Close,
    /// Una fila (ir a su tab).
    Row(usize),
    /// «Ir al tab» de una fila.
    Go(usize),
    /// Una opción de un permiso: fila y su tecla.
    Option(usize, char),
}

#[derive(Default)]
pub struct Inbox {
    pub scroll: usize,
    /// Fila elegida con el teclado.
    pub selected: usize,
    pub hover: Option<Hit>,
}

struct Geometry {
    panel: Rect,
    close: Rect,
    list: Rect,
    rows: usize,
}

fn geometry(l: &Layout) -> Geometry {
    let s = l.s();
    let (w, h) = ((PANEL_W * s).min(l.w() - 40.0 * s), (PANEL_H * s).min(l.h() - 40.0 * s));
    let panel = Rect { x: (l.w() - w) / 2.0, y: (l.h() - h) / 2.0, w, h };
    let close = Rect { x: panel.x + w - 38.0 * s, y: panel.y + 12.0 * s, w: 26.0 * s, h: 26.0 * s };
    let list_y = panel.y + 70.0 * s;
    let rows = (((panel.y + h - 16.0 * s - list_y) / (ROW_H * s)).floor() as usize).max(1);
    let list = Rect { x: panel.x + 20.0 * s, y: list_y, w: w - 40.0 * s, h: rows as f32 * ROW_H * s };
    Geometry { panel, close, list, rows }
}

/// `text` recortado con «…» para que quepa en `max_w` (estilo de los botones).
fn fit(k: &mut Kit, text: &str, max_w: f32) -> String {
    if k.measure(text, Text::label()) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let cut = |n: usize| chars[..n].iter().collect::<String>().trim_end().to_string() + "…";
    let (mut lo, mut hi) = (0, chars.len().saturating_sub(1));
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if k.measure(&cut(mid), Text::label()) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    cut(lo)
}

/// Rectángulo de la fila visible `i` y sus botones: «Ir al tab» y las opciones.
fn row_parts(k: &mut Kit, g: &Geometry, row: usize, scroll: usize, e: &Entry) -> (Rect, Rect, Vec<(Rect, char, String)>) {
    let s = k.s;
    let r = Rect { x: g.list.x, y: g.list.y + (row - scroll) as f32 * ROW_H * s, w: g.list.w, h: ROW_H * s - 6.0 * s };
    let by = r.y + r.h - 9.0 * s - BUTTON_H * s;
    let go_w = k.measure("Ir al tab", Text::label()) + 46.0 * s;
    let go = Rect { x: r.x + r.w - 12.0 * s - go_w, y: by, w: go_w, h: BUTTON_H * s };
    let mut x = r.x + 34.0 * s;
    let room = go.x - 10.0 * s;
    let mut opts = Vec::new();
    let n = e.options.len().max(1) as f32;
    // Cada opción con un ancho máximo parejo; las largas se recortan con «…».
    let max_w = ((room - x - 6.0 * s * (n - 1.0)) / n).min(240.0 * s);
    for (digit, text) in &e.options {
        let label = fit(k, &format!("{digit}  {text}"), max_w - 24.0 * s);
        let w = k.measure(&label, Text::label()) + 24.0 * s;
        if w < 40.0 * s || x + w > room {
            break;
        }
        opts.push((Rect { x, y: by, w, h: BUTTON_H * s }, *digit, label));
        x += w + 6.0 * s;
    }
    (r, go, opts)
}

impl Inbox {
    /// Mantiene la fila elegida dentro de la lista y a la vista.
    pub fn clamp(&mut self, n: usize, l: &Layout) {
        let rows = geometry(l).rows;
        self.selected = self.selected.min(n.saturating_sub(1));
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
        self.scroll = self.scroll.min(n.saturating_sub(rows));
    }

    pub fn scroll_by(&mut self, delta: isize, n: usize, l: &Layout) {
        let max = n.saturating_sub(geometry(l).rows);
        self.scroll = (self.scroll as isize + delta).clamp(0, max as isize) as usize;
    }

    pub fn hit(&self, x: f32, y: f32, l: &Layout, fonts: &mut Fonts, entries: &[Entry]) -> Hit {
        let g = geometry(l);
        if !g.panel.contains(x, y) {
            return Hit::Outside;
        }
        if g.close.contains(x, y) {
            return Hit::Close;
        }
        let mut out = Vec::new();
        let mut k = Kit::new(&mut out, fonts, l.s());
        for (i, e) in entries.iter().enumerate().skip(self.scroll).take(g.rows) {
            let (r, go, opts) = row_parts(&mut k, &g, i, self.scroll, e);
            if go.contains(x, y) {
                return Hit::Go(i);
            }
            if let Some((_, d, _)) = opts.iter().find(|(o, _, _)| o.contains(x, y)) {
                return Hit::Option(i, *d);
            }
            if r.contains(x, y) {
                return Hit::Row(i);
            }
        }
        Hit::Inside
    }

    pub fn draw(&self, out: &mut Vec<Quad>, fonts: &mut Fonts, l: &Layout, entries: &[Entry], time: f32) {
        let s = l.s();
        let g = geometry(l);
        let th = theme::t();
        let mut k = Kit::new(out, fonts, s);
        let p = g.panel;
        k.rect(Rect { x: 0.0, y: 0.0, w: l.w(), h: l.h() }, theme::alpha([0.0, 0.0, 0.0, 1.0], if th.dark { 0.5 } else { 0.25 }));
        k.panel(p, 12.0);
        k.icon(p.x + 34.0 * s, p.y + 30.0 * s, icons::INBOX, 17.0, th.accent);
        k.text(p.x + 52.0 * s, p.y + 20.0 * s, "Agentes", Text::title(), p.w - 120.0 * s);
        let count = |st: AgentState| entries.iter().filter(|e| e.state == st).count();
        let shells = entries.iter().filter(|e| e.waiting_shell()).count();
        let mut summary = format!(
            "{} te necesitan · {} terminaron · {} trabajando · {} en espera",
            count(AgentState::NeedsYou),
            count(AgentState::Done),
            count(AgentState::Working),
            count(AgentState::Idle) - shells
        );
        if shells > 0 {
            summary.push_str(&format!(" · {shells} esperando una shell"));
        }
        k.text(p.x + 52.0 * s, p.y + 40.0 * s, &summary, Text::small(), p.w - 120.0 * s);
        k.icon_button(g.close, icons::X, 15.0, self.hover == Some(Hit::Close));

        if entries.is_empty() {
            let r = Rect { y: g.list.y + 40.0 * s, h: 40.0 * s, ..g.list };
            k.text_center(r, "No hay sesiones de Claude abiertas", Text::body().color(th.text_dim));
            return;
        }
        for (i, e) in entries.iter().enumerate().skip(self.scroll).take(g.rows) {
            let (r, go, opts) = row_parts(&mut k, &g, i, self.scroll, e);
            let hovered = matches!(self.hover, Some(Hit::Row(h) | Hit::Go(h) | Hit::Option(h, _)) if h == i);
            let shell_label = match e.shells {
                1 => "esperando una shell".to_string(),
                n => format!("esperando {n} shells"),
            };
            let (label, color) = match e.state {
                AgentState::NeedsYou => ("te necesita", th.attention),
                AgentState::Done => ("terminó", th.done),
                AgentState::Working => ("trabajando", crate::ui::status_pulse(time)),
                _ if e.waiting_shell() => (shell_label.as_str(), th.accent),
                _ => ("en espera", th.text_dim),
            };
            let border = if i == self.selected { th.accent } else if hovered { theme::mix(th.border, th.text, 0.25) } else { th.border };
            k.bordered(r, 8.0, th.chrome, border);
            let ty = r.y + 10.0 * s;
            if e.waiting_shell() {
                // El mismo ícono que en la barra lateral.
                k.icon(r.x + 18.0 * s, ty + 8.0 * s, icons::SQUARE_TERMINAL, 12.0, color);
            } else {
                k.dot(r.x + 18.0 * s, ty + 8.0 * s, 8.0, color);
            }
            let when = format!("{label} · hace {}", ago(e.secs));
            let when_w = k.measure(&when, Text::small());
            let right = r.x + r.w - 14.0 * s;
            k.text(right - when_w, ty + 1.5 * s, &when, Text::small().color(if e.state == AgentState::Idle { th.text_dim } else { color }), when_w + 1.0);
            k.text(r.x + 34.0 * s, ty, &e.place, Text::label(), right - when_w - 16.0 * s - (r.x + 34.0 * s));
            if let Some(d) = &e.detail {
                let style = if e.state == AgentState::NeedsYou { Text::body() } else { Text::body().color(th.text_dim) };
                k.text(r.x + 34.0 * s, ty + 21.0 * s, d, style, r.w - 48.0 * s);
            }
            for (o, digit, text) in &opts {
                k.button(*o, text, None, Variant::Subtle, self.hover == Some(Hit::Option(i, *digit)));
            }
            k.button(go, "Ir al tab", Some(icons::CORNER_DOWN_LEFT), Variant::Ghost, self.hover == Some(Hit::Go(i)));
        }
        if entries.len() > g.rows {
            let track = g.list;
            let h = (track.h * g.rows as f32 / entries.len() as f32).max(24.0 * s);
            let y = track.y + (track.h - h) * self.scroll as f32 / (entries.len() - g.rows) as f32;
            k.rounded(Rect { x: track.x + track.w + 6.0 * s, y, w: 3.0 * s, h }, 1.5, th.border);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esperando_una_shell_va_antes_que_en_espera() {
        let e = |state, shells| Entry { tab: 1, place: String::new(), state, secs: 0, detail: None, options: Vec::new(), shells };
        assert!(e(AgentState::Idle, 1).waiting_shell());
        assert!(!e(AgentState::Working, 1).waiting_shell());
        assert!(!e(AgentState::Idle, 0).waiting_shell());
        assert!(rank(&e(AgentState::Working, 0)) < rank(&e(AgentState::Idle, 2)));
        assert!(rank(&e(AgentState::Idle, 2)) < rank(&e(AgentState::Idle, 0)));
    }

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn opciones_de_un_permiso() {
        let pantalla = lines(&[
            "⏺ Bash(npm test)",
            "",
            "╭──────────────────────────╮",
            "│ Bash command             │",
            "│   npm test               │",
            "│ Do you want to proceed?  │",
            "│ ❯ 1. Yes                 │",
            "│   2. Yes, and don't ask again for npm test commands │",
            "│   3. No, and tell Claude what to do differently (esc) │",
            "╰──────────────────────────╯",
        ]);
        // Con o sin los bordes de la caja.
        let o = options(&pantalla);
        assert_eq!(o.iter().map(|(d, _)| *d).collect::<String>(), "123");
        assert_eq!(o[0].1, "Yes");
        assert_eq!(question(&pantalla, true).as_deref(), Some("Do you want to proceed?"));
    }

    #[test]
    fn una_lista_vieja_no_cuenta_y_una_sola_opcion_tampoco() {
        let vieja = lines(&["1. uno", "2. dos", "texto", "⏺ Listo", "1. solo una"]);
        assert!(options(&vieja).is_empty());
        let saltada = lines(&["1. a", "3. c"]);
        assert!(options(&saltada).is_empty());
    }

    #[test]
    fn ultimo_paso_y_tiempo() {
        let p = lines(&["⏺ Leí el archivo", "  algo", "⏺ Corrí las pruebas: 12 pasan", "> "]);
        assert_eq!(last_step(&p).as_deref(), Some("Corrí las pruebas: 12 pasan"));
        assert_eq!(last_step(&lines(&["nada"])), None);
        assert_eq!(ago(42), "42 s");
        assert_eq!(ago(125), "2 min");
        assert_eq!(ago(7200), "2 h");
        assert_eq!(ago(7500), "2 h 5 min");
    }
}
