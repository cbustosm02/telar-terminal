//! Layout, dibujo y hit-testing de la ventana.
//!
//!   ┌────────────┬──────────────────────────────────────┐
//!   │ ● ● ●      │ [tab 1] [tab 2] [+]                  │  ← barra de tabs del grupo activo
//!   │ GRUPOS     ├──────────────────────────────────────┤
//!   │ ▌Grupo 1 2 │                                      │
//!   │  Grupo 2 1 │   terminal del tab activo            │
//!   │  + Nuevo   │                                      │
//!   └────────────┴──────────────────────────────────────┘

use std::collections::HashMap;

use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::{Colors, COUNT};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};

use crate::awake;
use crate::font::{self, Fonts, Glyph};
use crate::renderer::{Quad, KIND_COLOR, KIND_MASK};
use crate::terminal::{AgentState, GridSize, Listener, Terminal};
use crate::theme::{self, Rgba};
use crate::workspace::Workspace;

// Medidas en puntos lógicos (se multiplican por el factor de escala).
const SIDEBAR_W: f32 = 210.0;
const TITLEBAR_H: f32 = 40.0;
const LIST_TOP: f32 = 76.0;
const GROUP_ROW_H: f32 = 32.0;
const PAD_X: f32 = 10.0;
const PAD_Y: f32 = 6.0;
const TAB_MIN_W: f32 = 90.0;
const TAB_MAX_W: f32 = 220.0;
const TAB_GAP: f32 = 4.0;
const NEW_TAB_W: f32 = 30.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Hit {
    #[default]
    None,
    Group(usize),
    NewGroup,
    Tab(usize),
    CloseTab(usize),
    NewTab,
    /// Interruptor de `caffeinate`, abajo en la barra lateral.
    Awake,
    /// Zona vacía de la barra de título: arrastra la ventana.
    Drag,
    Terminal,
}

pub struct Layout {
    s: f32,
    w: f32,
    h: f32,
    tab_w: f32,
    pub cell_w: f32,
    pub cell_h: f32,
    pub grid_x: f32,
    pub grid_y: f32,
    pub cols: usize,
    pub rows: usize,
}

impl Layout {
    pub fn new(w: f32, h: f32, s: f32, fonts: &Fonts, n_tabs: usize) -> Self {
        let grid_x = (SIDEBAR_W + PAD_X) * s;
        let grid_y = (TITLEBAR_H + PAD_Y) * s;
        let cols = (((w - grid_x - PAD_X * s) / fonts.cell_w).floor() as usize).max(2);
        let rows = (((h - grid_y - PAD_Y * s) / fonts.cell_h).floor() as usize).max(1);
        let avail = w - (SIDEBAR_W + 8.0 + NEW_TAB_W + 8.0) * s;
        let n = n_tabs.max(1) as f32;
        let tab_w = (avail / n - TAB_GAP * s).clamp(TAB_MIN_W * s, TAB_MAX_W * s);
        Self { s, w, h, tab_w, cell_w: fonts.cell_w, cell_h: fonts.cell_h, grid_x, grid_y, cols, rows }
    }

    pub fn grid_size(&self) -> GridSize {
        GridSize { cols: self.cols, rows: self.rows, cell_w: self.cell_w as u16, cell_h: self.cell_h as u16 }
    }

    fn sidebar_w(&self) -> f32 {
        SIDEBAR_W * self.s
    }

    fn titlebar_h(&self) -> f32 {
        TITLEBAR_H * self.s
    }

    pub fn group_rect(&self, i: usize) -> Rect {
        let s = self.s;
        Rect { x: 8.0 * s, y: (LIST_TOP + i as f32 * GROUP_ROW_H) * s, w: (SIDEBAR_W - 16.0) * s, h: (GROUP_ROW_H - 2.0) * s }
    }

    pub fn tab_rect(&self, i: usize) -> Rect {
        let s = self.s;
        Rect { x: (SIDEBAR_W + 8.0) * s + i as f32 * (self.tab_w + TAB_GAP * s), y: 7.0 * s, w: self.tab_w, h: (TITLEBAR_H - 14.0) * s }
    }

    fn tab_close_rect(&self, i: usize) -> Rect {
        let r = self.tab_rect(i);
        let side = r.h - 8.0 * self.s;
        Rect { x: r.x + r.w - side - 4.0 * self.s, y: r.y + 4.0 * self.s, w: side, h: side }
    }

    fn new_tab_rect(&self, n_tabs: usize) -> Rect {
        let r = self.tab_rect(n_tabs);
        Rect { w: NEW_TAB_W * self.s, ..r }
    }

    /// Fila de «Despierto», al pie de la barra lateral.
    fn awake_rect(&self) -> Rect {
        let s = self.s;
        Rect { x: 8.0 * s, y: self.h - (GROUP_ROW_H + 8.0) * s, w: (SIDEBAR_W - 16.0) * s, h: (GROUP_ROW_H - 2.0) * s }
    }

    pub fn hit(&self, x: f32, y: f32, n_groups: usize, n_tabs: usize) -> Hit {
        if x < self.sidebar_w() {
            if self.awake_rect().contains(x, y) {
                return Hit::Awake;
            }
            if let Some(i) = (0..n_groups).find(|i| self.group_rect(*i).contains(x, y)) {
                return Hit::Group(i);
            }
            if self.group_rect(n_groups).contains(x, y) {
                return Hit::NewGroup;
            }
            return if y < self.titlebar_h() { Hit::Drag } else { Hit::None };
        }
        if y < self.titlebar_h() {
            for i in 0..n_tabs {
                if self.tab_close_rect(i).contains(x, y) {
                    return Hit::CloseTab(i);
                }
                if self.tab_rect(i).contains(x, y) {
                    return Hit::Tab(i);
                }
            }
            if self.new_tab_rect(n_tabs).contains(x, y) {
                return Hit::NewTab;
            }
            return Hit::Drag;
        }
        Hit::Terminal
    }

    /// Celda bajo el puntero (columna, fila de pantalla, lado de la celda).
    pub fn cell_at(&self, x: f32, y: f32) -> (usize, usize, Side) {
        let fx = ((x - self.grid_x) / self.cell_w).max(0.0);
        let fy = ((y - self.grid_y) / self.cell_h).max(0.0);
        let col = (fx as usize).min(self.cols - 1);
        let row = (fy as usize).min(self.rows - 1);
        let side = if fx.fract() < 0.5 { Side::Left } else { Side::Right };
        (col, row, side)
    }

    /// Posición del cursor en píxeles (para la ventana del IME).
    pub fn cell_origin(&self, col: usize, row: usize) -> (f32, f32) {
        (self.grid_x + col as f32 * self.cell_w, self.grid_y + row as f32 * self.cell_h)
    }
}

pub struct View<'a> {
    pub ws: &'a Workspace,
    pub terms: &'a HashMap<u64, Terminal>,
    pub hover: Hit,
    pub editing: Option<(usize, &'a str)>,
    pub preedit: &'a str,
    pub focused: bool,
    pub palette: &'a [Rgba; COUNT],
    /// Segundos desde que abrió la app (para animar el indicador de «trabajando»).
    pub time: f32,
    /// Modo de `caffeinate` y si está corriendo ahora.
    pub awake: (awake::Mode, bool),
}

/// Prioridad y color del indicador de un tab (None = sin indicador).
fn status(t: &Terminal, time: f32) -> Option<(u8, Rgba)> {
    match t.agent.state {
        AgentState::NeedsYou => Some((4, theme::ATTENTION)),
        AgentState::Done => Some((3, theme::DONE)),
        _ if t.attention => Some((2, theme::ATTENTION)),
        AgentState::Working => {
            let pulse = 0.5 + 0.5 * (time * std::f32::consts::TAU * 0.9).sin();
            let mut c = theme::ACCENT;
            c[3] = 0.3 + 0.7 * pulse;
            Some((1, c))
        }
        _ => None,
    }
}

fn dot(out: &mut Vec<Quad>, x: f32, cy: f32, d: f32, color: Rgba) {
    out.push(Quad::rounded(x, cy - d / 2.0, d, d, d / 2.0, color));
}

fn push_glyph(out: &mut Vec<Quad>, fonts: &Fonts, g: Glyph, x: f32, y: f32, box_w: f32, color: Rgba) {
    let (w, h) = (g.uv[2], g.uv[3]);
    if g.color {
        // Emoji: se ajusta a la caja de la celda conservando la proporción.
        let k = (box_w / w).min(fonts.cell_h / h).min(1.0);
        let (dw, dh) = (w * k, h * k);
        out.push(Quad {
            rect: [x + (box_w - dw) / 2.0, y + (fonts.cell_h - dh) / 2.0, dw, dh],
            uv: g.uv,
            color,
            params: [KIND_COLOR, 0.0, 0.0, 0.0],
        });
    } else {
        out.push(Quad {
            rect: [x + g.left, y + fonts.baseline - g.top, w, h],
            uv: g.uv,
            color,
            params: [KIND_MASK, 0.0, 0.0, 0.0],
        });
    }
}

/// Texto de una línea; recorta con «…» si no cabe. Devuelve el ancho usado.
fn text(out: &mut Vec<Quad>, fonts: &mut Fonts, x: f32, y: f32, s: &str, color: Rgba, max_w: f32) -> f32 {
    let cw = fonts.cell_w;
    let max_chars = (max_w / cw).floor().max(0.0) as usize;
    let n = s.chars().count();
    let chars: Vec<char> = if n > max_chars {
        s.chars().take(max_chars.saturating_sub(1)).chain(std::iter::once('…')).collect()
    } else {
        s.chars().collect()
    };
    for (i, c) in chars.iter().enumerate() {
        if let Some(g) = fonts.glyph(*c, font::REGULAR) {
            push_glyph(out, fonts, g, x + i as f32 * cw, y, cw, color);
        }
    }
    chars.len() as f32 * cw
}

pub fn draw(out: &mut Vec<Quad>, fg: &mut Vec<Quad>, fonts: &mut Fonts, l: &Layout, v: &View) -> Option<(usize, usize)> {
    out.clear();
    fg.clear();
    draw_sidebar(out, fonts, l, v);
    draw_tabbar(out, fonts, l, v);
    let mut ime_cell = None;
    let active = v.ws.active_term().and_then(|id| v.terms.get(&id));
    if let Some(t) = active {
        let term = t.term.lock();
        ime_cell = draw_terminal(out, fg, fonts, l, &term, v);
    }
    out.append(fg);
    if active.is_some_and(Terminal::is_suspended) {
        draw_suspended(out, fonts, l);
        return None;
    }
    ime_cell
}

/// Marca de tab suspendido (en lugar del punto de estado).
const MOON: &str = "☾";

/// Vista previa de un tab suspendido: su última pantalla, atenuada, con un aviso abajo.
fn draw_suspended(out: &mut Vec<Quad>, fonts: &mut Fonts, l: &Layout) {
    let s = l.s;
    let x0 = l.sidebar_w();
    let top = l.titlebar_h();
    let mut veil = theme::CHROME_BG;
    veil[3] = 0.55;
    out.push(Quad::rect(x0, top, l.w - x0, l.h - top, veil));

    let msg = "Suspendido · Enter para reanudar · ⌘⇧S reanuda el grupo";
    let (cw, ch) = (fonts.cell_w, fonts.cell_h);
    let pad = 14.0 * s;
    let w = (msg.chars().count() as f32 * cw + 2.0 * pad).min(l.w - x0 - 24.0 * s);
    let h = ch + 2.0 * 9.0 * s;
    let x = x0 + (l.w - x0 - w) / 2.0;
    let y = l.h - h - 28.0 * s;
    out.push(Quad::rounded(x - s, y - s, w + 2.0 * s, h + 2.0 * s, 9.0 * s, theme::BORDER));
    out.push(Quad::rounded(x, y, w, h, 8.0 * s, theme::ITEM_ACTIVE));
    text(out, fonts, x + pad, y + (h - ch) / 2.0, msg, theme::TEXT, w - 2.0 * pad);
}

fn draw_sidebar(out: &mut Vec<Quad>, fonts: &mut Fonts, l: &Layout, v: &View) {
    let s = l.s;
    let ch = fonts.cell_h;
    out.push(Quad::rect(0.0, 0.0, l.sidebar_w(), l.h, theme::CHROME_BG));
    out.push(Quad::rect(l.sidebar_w() - s, 0.0, s, l.h, theme::BORDER));
    text(out, fonts, 18.0 * s, (LIST_TOP - 26.0) * s, "GRUPOS", theme::TEXT_DIM, (SIDEBAR_W - 30.0) * s);

    for (i, g) in v.ws.groups.iter().enumerate() {
        let r = l.group_rect(i);
        let active = i == v.ws.active;
        if active {
            out.push(Quad::rounded(r.x, r.y, r.w, r.h, 6.0 * s, theme::ITEM_ACTIVE));
            out.push(Quad::rounded(r.x, r.y + 7.0 * s, 3.0 * s, r.h - 14.0 * s, 1.5 * s, theme::ACCENT));
        } else if v.hover == Hit::Group(i) {
            out.push(Quad::rounded(r.x, r.y, r.w, r.h, 6.0 * s, theme::ITEM_HOVER));
        }
        let ty = r.y + (r.h - ch) / 2.0;
        let tx = r.x + 12.0 * s;
        let asleep = g.tabs.iter().all(|id| v.terms.get(id).is_some_and(Terminal::is_suspended));
        let count = g.tabs.len().to_string();
        let badge_w = count.len() as f32 * fonts.cell_w;
        let name_w = r.w - 24.0 * s - badge_w - 8.0 * s;

        match v.editing {
            Some((gi, buf)) if gi == i => {
                let w = text(out, fonts, tx, ty, buf, theme::TEXT, name_w);
                out.push(Quad::rect(tx + w + s, ty + 2.0 * s, 2.0 * s, ch - 4.0 * s, theme::ACCENT));
            }
            _ => {
                let color = if active && !asleep { theme::TEXT } else { theme::TEXT_DIM };
                text(out, fonts, tx, ty, &g.name, color, name_w);
            }
        }
        text(out, fonts, r.x + r.w - 12.0 * s - badge_w, ty, &count, theme::TEXT_DIM, badge_w + 1.0);
        // El grupo muestra el estado más urgente de sus tabs (o la luna si todos duermen).
        let worst = g.tabs.iter().filter_map(|id| v.terms.get(id)).filter_map(|t| status(t, v.time)).max_by_key(|(p, _)| *p);
        let d = 7.0 * s;
        if let Some((_, color)) = worst {
            dot(out, r.x + r.w - 12.0 * s - badge_w - d - 7.0 * s, r.y + r.h / 2.0, d, color);
        } else if asleep {
            text(out, fonts, r.x + r.w - 12.0 * s - badge_w - fonts.cell_w - 6.0 * s, ty, MOON, theme::TEXT_DIM, fonts.cell_w + 1.0);
        }
    }

    let r = l.group_rect(v.ws.groups.len());
    let hovered = v.hover == Hit::NewGroup;
    if hovered {
        out.push(Quad::rounded(r.x, r.y, r.w, r.h, 6.0 * s, theme::ITEM_HOVER));
    }
    let color = if hovered { theme::TEXT } else { theme::TEXT_DIM };
    text(out, fonts, r.x + 12.0 * s, r.y + (r.h - ch) / 2.0, "+ Nuevo grupo", color, r.w - 24.0 * s);

    // Pie: «Despierto» (caffeinate). Punto azul mientras está corriendo.
    let r = l.awake_rect();
    let hovered = v.hover == Hit::Awake;
    if hovered {
        out.push(Quad::rounded(r.x, r.y, r.w, r.h, 6.0 * s, theme::ITEM_HOVER));
    }
    let (mode, running) = v.awake;
    let color = if hovered || mode != awake::Mode::Off { theme::TEXT } else { theme::TEXT_DIM };
    let ty = r.y + (r.h - ch) / 2.0;
    text(out, fonts, r.x + 12.0 * s, ty, mode.label(), color, r.w - 40.0 * s);
    if running {
        let d = 7.0 * s;
        dot(out, r.x + r.w - 12.0 * s - d, r.y + r.h / 2.0, d, theme::ACCENT);
    }
}

fn draw_tabbar(out: &mut Vec<Quad>, fonts: &mut Fonts, l: &Layout, v: &View) -> Option<()> {
    let s = l.s;
    let ch = fonts.cell_h;
    let x0 = l.sidebar_w();
    out.push(Quad::rect(x0, 0.0, l.w - x0, l.titlebar_h(), theme::CHROME_BG));
    out.push(Quad::rect(x0, l.titlebar_h() - s, l.w - x0, s, theme::BORDER));

    let g = v.ws.group()?;
    for (i, id) in g.tabs.iter().enumerate() {
        let r = l.tab_rect(i);
        let active = i == g.active;
        let hovered = matches!(v.hover, Hit::Tab(t) | Hit::CloseTab(t) if t == i);
        if active {
            out.push(Quad::rounded(r.x, r.y, r.w, r.h, 6.0 * s, theme::ITEM_ACTIVE));
        } else if hovered {
            out.push(Quad::rounded(r.x, r.y, r.w, r.h, 6.0 * s, theme::ITEM_HOVER));
        }
        let fallback = format!("Tab {}", i + 1);
        let title = v
            .terms
            .get(id)
            .and_then(|t| t.title.as_deref().or(t.process.as_deref()))
            .unwrap_or(&fallback);
        let close = l.tab_close_rect(i);
        let suspended = v.terms.get(id).is_some_and(Terminal::is_suspended);
        let color = if active && !suspended { theme::TEXT } else { theme::TEXT_DIM };
        let ty = r.y + (r.h - ch) / 2.0;
        let mut tx = r.x + 10.0 * s;
        if suspended {
            text(out, fonts, tx - 1.0 * s, ty, MOON, theme::TEXT_DIM, fonts.cell_w + 1.0);
            tx += fonts.cell_w + 5.0 * s;
        } else if let Some((_, color)) = v.terms.get(id).and_then(|t| status(t, v.time)) {
            let d = 7.0 * s;
            dot(out, tx, r.y + r.h / 2.0, d, color);
            tx += d + 7.0 * s;
        }
        text(out, fonts, tx, ty, title, color, close.x - tx - 4.0 * s);
        if active || hovered {
            if v.hover == Hit::CloseTab(i) {
                out.push(Quad::rounded(close.x, close.y, close.w, close.h, 4.0 * s, theme::BORDER));
            }
            let cx = close.x + (close.w - fonts.cell_w) / 2.0;
            text(out, fonts, cx, close.y + (close.h - ch) / 2.0, "×", theme::TEXT_DIM, fonts.cell_w + 1.0);
        }
    }
    let r = l.new_tab_rect(g.tabs.len());
    if v.hover == Hit::NewTab {
        out.push(Quad::rounded(r.x, r.y, r.w, r.h, 6.0 * s, theme::ITEM_HOVER));
    }
    let cx = r.x + (r.w - fonts.cell_w) / 2.0;
    text(out, fonts, cx, r.y + (r.h - ch) / 2.0, "+", theme::TEXT_DIM, fonts.cell_w + 1.0);
    Some(())
}

fn resolve(color: Color, colors: &Colors, palette: &[Rgba; COUNT], flags: Flags) -> Rgba {
    let idx = match color {
        Color::Spec(rgb) => return [rgb.r as f32 / 255.0, rgb.g as f32 / 255.0, rgb.b as f32 / 255.0, 1.0],
        Color::Indexed(i) => i as usize,
        Color::Named(n) => {
            let n = if flags.contains(Flags::DIM) { n.to_dim() } else { n };
            n as usize
        }
    };
    match colors[idx] {
        Some(rgb) => [rgb.r as f32 / 255.0, rgb.g as f32 / 255.0, rgb.b as f32 / 255.0, 1.0],
        None => palette[idx],
    }
}

/// Dibuja la terminal. Devuelve la celda del cursor (para posicionar el IME).
fn draw_terminal(
    out: &mut Vec<Quad>,
    fg: &mut Vec<Quad>,
    fonts: &mut Fonts,
    l: &Layout,
    term: &Term<Listener>,
    v: &View,
) -> Option<(usize, usize)> {
    let content = term.renderable_content();
    let colors = content.colors;
    let offset = content.display_offset as i32;
    let (cw, ch) = (fonts.cell_w, fonts.cell_h);
    let default_bg = resolve(Color::Named(NamedColor::Background), colors, v.palette, Flags::empty());
    let cursor_color = resolve(Color::Named(NamedColor::Cursor), colors, v.palette, Flags::empty());

    let x0 = l.sidebar_w();
    out.push(Quad::rect(x0, l.titlebar_h(), l.w - x0, l.h - l.titlebar_h(), default_bg));

    let cursor = content.cursor;
    let show_cursor = cursor.shape != CursorShape::Hidden && content.mode.contains(TermMode::SHOW_CURSOR);
    let block_cursor = show_cursor && v.focused && cursor.shape == CursorShape::Block && v.preedit.is_empty();

    // Fondos: celdas contiguas del mismo color se fusionan en un solo rectángulo.
    let mut run: Option<(i32, f32, f32, Rgba)> = None;
    let flush = |out: &mut Vec<Quad>, run: &mut Option<(i32, f32, f32, Rgba)>, y: f32| {
        if let Some((_, rx0, rx1, c)) = run.take() {
            out.push(Quad::rect(rx0, y, rx1 - rx0, ch, c));
        }
    };

    for cell in content.display_iter {
        let p = cell.point;
        let row = p.line.0 + offset;
        if row < 0 || row as usize >= l.rows {
            continue;
        }
        let x = l.grid_x + p.column.0 as f32 * cw;
        let y = l.grid_y + row as f32 * ch;
        let flags = cell.flags;

        let mut cfg = resolve(cell.fg, colors, v.palette, flags);
        let mut cbg = resolve(cell.bg, colors, v.palette, Flags::empty());
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut cfg, &mut cbg);
        }
        if content.selection.is_some_and(|s| s.contains(p)) {
            cbg = theme::SELECTION;
        }
        if block_cursor && p == cursor.point {
            cbg = cursor_color;
            cfg = default_bg;
        }

        if let Some((rrow, _, rx1, rc)) = run {
            if rrow != row || rx1 != x || rc != cbg {
                let ry = l.grid_y + rrow as f32 * ch;
                flush(out, &mut run, ry);
            }
        }
        if cbg != default_bg {
            match &mut run {
                Some((_, _, rx1, _)) => *rx1 = x + cw,
                None => run = Some((row, x, x + cw, cbg)),
            }
        }

        let skip = Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER | Flags::HIDDEN;
        if cell.c != ' ' && cell.c != '\t' && !flags.intersects(skip) {
            let style = match (flags.contains(Flags::BOLD), flags.contains(Flags::ITALIC)) {
                (true, true) => font::BOLD_ITALIC,
                (true, false) => font::BOLD,
                (false, true) => font::ITALIC,
                _ => font::REGULAR,
            };
            if let Some(g) = fonts.glyph(cell.c, style) {
                let box_w = if flags.contains(Flags::WIDE_CHAR) { cw * 2.0 } else { cw };
                push_glyph(fg, fonts, g, x, y, box_w, cfg);
            }
        }
        if flags.intersects(Flags::ALL_UNDERLINES) {
            fg.push(Quad::rect(x, y + fonts.underline, cw, fonts.stroke, cfg));
        }
        if flags.contains(Flags::STRIKEOUT) {
            fg.push(Quad::rect(x, y + (ch / 2.0).round(), cw, fonts.stroke, cfg));
        }
    }
    if let Some((rrow, ..)) = run {
        flush(out, &mut run, l.grid_y + rrow as f32 * ch);
    }

    let crow = cursor.point.line.0 + offset;
    if crow < 0 || crow as usize >= l.rows {
        return None;
    }
    let ccol = cursor.point.column.0;
    let (x, y) = l.cell_origin(ccol, crow as usize);

    if !v.preedit.is_empty() {
        // Texto en composición (tecla muerta, p. ej. «´» antes de la vocal).
        let n = v.preedit.chars().count() as f32;
        fg.push(Quad::rect(x, y, n * cw, ch, default_bg));
        text(fg, fonts, x, y, v.preedit, theme::TEXT, n * cw + 1.0);
        fg.push(Quad::rect(x, y + ch - 2.0 * fonts.stroke, n * cw, fonts.stroke, theme::ACCENT));
    } else if show_cursor && !block_cursor {
        let t = (fonts.stroke * 2.0).max(2.0);
        match cursor.shape {
            CursorShape::Beam => fg.push(Quad::rect(x, y, t, ch, cursor_color)),
            CursorShape::Underline => fg.push(Quad::rect(x, y + ch - t, cw, t, cursor_color)),
            _ => {
                let s = fonts.stroke.max(1.0);
                fg.push(Quad::rect(x, y, cw, s, cursor_color));
                fg.push(Quad::rect(x, y + ch - s, cw, s, cursor_color));
                fg.push(Quad::rect(x, y, s, ch, cursor_color));
                fg.push(Quad::rect(x + cw - s, y, s, ch, cursor_color));
            }
        }
    }
    Some((ccol, crow as usize))
}

/// Punto de la grilla (con scrollback) para una celda de pantalla.
pub fn grid_point(term: &Term<Listener>, col: usize, row: usize) -> Point {
    let offset = term.grid().display_offset() as i32;
    Point::new(Line(row as i32 - offset), Column(col))
}
