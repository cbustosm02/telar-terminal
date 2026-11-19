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
use crate::statsview;
use crate::kit::{Kit, Text, Variant, icons};
use crate::update::{self, Status};
use crate::font::{self, Fonts, Glyph};
use crate::renderer::{Quad, KIND_COLOR, KIND_MASK};
use crate::terminal::{AgentState, GridSize, Listener, Terminal};
use crate::theme::{self, Rgba};
use crate::workspace::Workspace;

// Medidas en puntos lógicos (se multiplican por el factor de escala).
const SIDEBAR_W: f32 = 210.0;
const TITLEBAR_H: f32 = 40.0;
/// Fila de «GRUPOS» (con + y ⚙), buscador y comienzo de la lista.
const HEADER_Y: f32 = 44.0;
const SEARCH_Y: f32 = 74.0;
const SEARCH_H: f32 = 28.0;
const LIST_TOP: f32 = 112.0;
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
    /// Versión y actualización, al pie de la barra lateral.
    Update,
    /// Engranaje: abre la configuración.
    Settings,
    /// Medidores de uso: abren el detalle.
    Stats,
    /// Campo de búsqueda de tabs y grupos.
    Search,
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
    /// Primera fila visible de la lista de grupos (o de resultados).
    pub list_scroll: usize,
    /// La lista termina con la fila «Nuevo grupo» (no durante una búsqueda).
    pub new_row: bool,
}

impl Layout {
    pub fn new(w: f32, h: f32, s: f32, fonts: &Fonts, n_tabs: usize) -> Self {
        Self::with_cell(w, h, s, fonts.cell_w, fonts.cell_h, n_tabs)
    }

    /// Layout para una ventana de `w`×`h` píxeles, escala `s` y celdas de `cell_w`×`cell_h`.
    pub fn with_cell(w: f32, h: f32, s: f32, cell_w: f32, cell_h: f32, n_tabs: usize) -> Self {
        let grid_x = (SIDEBAR_W + PAD_X) * s;
        let grid_y = (TITLEBAR_H + PAD_Y) * s;
        let cols = (((w - grid_x - PAD_X * s) / cell_w).floor() as usize).max(2);
        let rows = (((h - grid_y - PAD_Y * s) / cell_h).floor() as usize).max(1);
        let avail = w - (SIDEBAR_W + 8.0 + NEW_TAB_W + 8.0) * s;
        let n = n_tabs.max(1) as f32;
        let tab_w = (avail / n - TAB_GAP * s).clamp(TAB_MIN_W * s, TAB_MAX_W * s);
        Self { s, w, h, tab_w, cell_w, cell_h, grid_x, grid_y, cols, rows, list_scroll: 0, new_row: true }
    }

    /// Factor de escala (píxeles por punto).
    pub fn s(&self) -> f32 {
        self.s
    }

    /// Ancho y alto de la ventana en píxeles.
    pub fn w(&self) -> f32 {
        self.w
    }

    pub fn h(&self) -> f32 {
        self.h
    }

    /// Engranaje de configuración, a la derecha de «GRUPOS».
    fn settings_rect(&self) -> Rect {
        let s = self.s;
        Rect { x: (SIDEBAR_W - 36.0) * s, y: HEADER_Y * s, w: 26.0 * s, h: 24.0 * s }
    }

    /// «+» fijo junto a «GRUPOS»: grupo nuevo sin bajar hasta el final de la lista.
    fn new_group_button(&self) -> Rect {
        Rect { x: self.settings_rect().x - 28.0 * self.s, ..self.settings_rect() }
    }

    pub fn search_rect(&self) -> Rect {
        let s = self.s;
        Rect { x: 8.0 * s, y: SEARCH_Y * s, w: (SIDEBAR_W - 16.0) * s, h: SEARCH_H * s }
    }

    /// Filas de la lista que caben entre el buscador y los medidores.
    pub fn list_rows(&self) -> usize {
        let bottom = self.meters_rect().y - 6.0 * self.s;
        (((bottom - LIST_TOP * self.s) / (GROUP_ROW_H * self.s)).floor() as usize).max(1)
    }

    /// La fila `i` de la lista está a la vista.
    pub fn row_visible(&self, i: usize) -> bool {
        i >= self.list_scroll && i < self.list_scroll + self.list_rows()
    }

    pub fn grid_size(&self) -> GridSize {
        GridSize { cols: self.cols, rows: self.rows, cell_w: self.cell_w as u16, cell_h: self.cell_h as u16 }
    }

    pub fn sidebar_w(&self) -> f32 {
        SIDEBAR_W * self.s
    }

    fn titlebar_h(&self) -> f32 {
        TITLEBAR_H * self.s
    }

    /// Fila `i` de la lista (contando desde la primera, no desde la visible).
    pub fn group_rect(&self, i: usize) -> Rect {
        let s = self.s;
        let row = i as f32 - self.list_scroll as f32;
        Rect { x: 8.0 * s, y: (LIST_TOP + row * GROUP_ROW_H) * s, w: (SIDEBAR_W - 16.0) * s, h: (GROUP_ROW_H - 2.0) * s }
    }

    /// Fila de la lista bajo `y` (puede quedar fuera de la vista o ser negativa).
    pub fn row_at(&self, y: f32) -> isize {
        ((y - LIST_TOP * self.s) / (GROUP_ROW_H * self.s)).floor() as isize + self.list_scroll as isize
    }

    /// Lugar de la barra de tabs bajo `x`, entre 0 y `n_tabs - 1`.
    pub fn tab_at(&self, x: f32, n_tabs: usize) -> usize {
        let i = ((x - (SIDEBAR_W + 8.0) * self.s) / (self.tab_w + TAB_GAP * self.s)).floor();
        (i.max(0.0) as usize).min(n_tabs.saturating_sub(1))
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

    /// Bloque de versión y actualizaciones, al pie de la barra lateral.
    fn update_rect(&self) -> Rect {
        let s = self.s;
        Rect { x: 8.0 * s, y: self.h - 52.0 * s, w: (SIDEBAR_W - 16.0) * s, h: 46.0 * s }
    }

    /// Medidores de uso (CPU, RAM, GPU), encima de la línea del pie.
    fn meters_rect(&self) -> Rect {
        let s = self.s;
        Rect { x: 8.0 * s, y: self.awake_rect().y - 54.0 * s, w: (SIDEBAR_W - 16.0) * s, h: 40.0 * s }
    }

    /// Bloque de «Despierto», encima del de la versión.
    fn awake_rect(&self) -> Rect {
        let s = self.s;
        Rect { x: 8.0 * s, y: self.update_rect().y - 48.0 * s, w: (SIDEBAR_W - 16.0) * s, h: 46.0 * s }
    }

    pub fn hit(&self, x: f32, y: f32, n_groups: usize, n_tabs: usize) -> Hit {
        if x < self.sidebar_w() {
            if self.awake_rect().contains(x, y) {
                return Hit::Awake;
            }
            if self.update_rect().contains(x, y) {
                return Hit::Update;
            }
            if self.settings_rect().contains(x, y) {
                return Hit::Settings;
            }
            if self.meters_rect().contains(x, y) {
                return Hit::Stats;
            }
            if self.new_group_button().contains(x, y) {
                return Hit::NewGroup;
            }
            if self.search_rect().contains(x, y) {
                return Hit::Search;
            }
            if let Some(i) = (0..n_groups).find(|i| self.row_visible(*i) && self.group_rect(*i).contains(x, y)) {
                return Hit::Group(i);
            }
            if self.new_row && self.row_visible(n_groups) && self.group_rect(n_groups).contains(x, y) {
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

/// Qué se está renombrando.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Group(usize),
    Tab(u64),
}

/// Acciones del menú contextual.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Show,
    Suspend,
    Resume,
    RenameTab,
    Clear,
    CloseTab,
    NewTab,
    NewAgent,
    RenameGroup,
    SuspendGroup,
    ResumeGroup,
    /// Excluir o volver a incluir el grupo en la suspensión por inactividad.
    ToggleIdleExempt,
}

pub struct MenuItem {
    pub icon: char,
    pub label: &'static str,
    /// Atajo de teclado, a la derecha (una tecla por elemento).
    pub keys: &'static [&'static str],
    /// `None`: separador.
    pub action: Option<Action>,
}

impl MenuItem {
    pub fn new(icon: char, label: &'static str, keys: &'static [&'static str], action: Action) -> Self {
        Self { icon, label, keys, action: Some(action) }
    }

    pub fn separator() -> Self {
        Self { icon: ' ', label: "", keys: &[], action: None }
    }
}

/// Menú contextual (clic derecho en un tab o un grupo), dibujado sobre todo lo demás.
pub struct Menu {
    pub x: f32,
    pub y: f32,
    /// Tab sobre el que se abrió (si fue sobre un tab).
    pub tab: Option<u64>,
    pub group: usize,
    pub items: Vec<MenuItem>,
    pub hover: Option<usize>,
}

const MENU_W: f32 = 270.0;
const MENU_ITEM_H: f32 = 28.0;
const MENU_SEP_H: f32 = 9.0;
const MENU_PAD: f32 = 5.0;

impl Menu {
    /// Abre el menú en (x, y), corrido para que no se salga de la ventana.
    pub fn new(x: f32, y: f32, tab: Option<u64>, group: usize, items: Vec<MenuItem>, l: &Layout) -> Self {
        let mut m = Self { x, y, tab, group, items, hover: None };
        let b = m.bounds(l.s);
        m.x = m.x.min(l.w - b.w - 4.0 * l.s).max(0.0);
        m.y = m.y.min(l.h - b.h - 4.0 * l.s).max(0.0);
        m
    }

    pub fn bounds(&self, s: f32) -> Rect {
        let h: f32 = self.items.iter().map(|i| if i.action.is_some() { MENU_ITEM_H } else { MENU_SEP_H }).sum();
        Rect { x: self.x, y: self.y, w: MENU_W * s, h: (h + 2.0 * MENU_PAD) * s }
    }

    fn rows(&self, s: f32) -> impl Iterator<Item = (usize, Rect)> + '_ {
        let mut y = self.y + MENU_PAD * s;
        self.items.iter().enumerate().map(move |(i, item)| {
            let h = if item.action.is_some() { MENU_ITEM_H } else { MENU_SEP_H } * s;
            let r = Rect { x: self.x + MENU_PAD * s, y, w: (MENU_W - 2.0 * MENU_PAD) * s, h };
            y += h;
            (i, r)
        })
    }

    /// Opción bajo el puntero (los separadores no cuentan).
    pub fn item_at(&self, x: f32, y: f32, s: f32) -> Option<usize> {
        self.rows(s).find(|(i, r)| self.items[*i].action.is_some() && r.contains(x, y)).map(|(i, _)| i)
    }

    /// Mueve la selección con el teclado (`delta` = ±1), saltando separadores.
    pub fn step(&mut self, delta: isize) {
        let n = self.items.len() as isize;
        let mut i = self.hover.map_or(if delta > 0 { -1 } else { n }, |h| h as isize);
        for _ in 0..n {
            i = (i + delta).rem_euclid(n);
            if self.items[i as usize].action.is_some() {
                self.hover = Some(i as usize);
                return;
            }
        }
    }
}

pub struct View<'a> {
    pub ws: &'a Workspace,
    pub terms: &'a HashMap<u64, Terminal>,
    pub hover: Hit,
    pub editing: Option<(Edit, &'a str)>,
    pub preedit: &'a str,
    pub focused: bool,
    pub palette: &'a [Rgba; COUNT],
    /// Segundos desde que abrió la app (para animar el indicador de «trabajando»).
    pub time: f32,
    /// Modo de `caffeinate` y si está corriendo ahora.
    pub awake: (awake::Mode, bool),
    pub menu: Option<&'a Menu>,
    pub update: &'a Status,
    pub stats: Option<&'a crate::stats::Sample>,
    /// Búsqueda abierta: texto, resultado elegido y resultados.
    pub search: Option<(&'a str, usize, &'a [Found])>,
    /// Límite de memoria de Telar completo (MB, 0: sin límite) y grupos pasados del suyo.
    pub ram_limit_mb: u64,
    pub over_groups: &'a [bool],
    /// Grupo sobre el que se va a soltar un tab arrastrado.
    pub drop_group: Option<usize>,
}

/// Resultado de buscar en la barra lateral: un grupo, o un tab de un grupo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    pub group: usize,
    pub tab: Option<usize>,
}

/// Grupos y tabs cuyo nombre contiene `query` (sin distinguir mayúsculas). El grupo va
/// antes que sus tabs; un tab que coincide trae su grupo aunque el grupo no coincida.
pub fn search(ws: &Workspace, terms: &HashMap<u64, Terminal>, query: &str) -> Vec<Found> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (gi, g) in ws.groups.iter().enumerate() {
        if g.name.to_lowercase().contains(&q) {
            out.push(Found { group: gi, tab: None });
        }
        for (ti, id) in g.tabs.iter().enumerate() {
            let t = terms.get(id);
            let names = [t.and_then(|t| t.name.as_deref()), t.and_then(|t| t.title.as_deref()), t.and_then(|t| t.process.as_deref())];
            if names.iter().flatten().any(|n| n.to_lowercase().contains(&q)) {
                out.push(Found { group: gi, tab: Some(ti) });
            }
        }
    }
    out
}

/// Prioridad y color del indicador de un tab (None = sin indicador).
fn status(t: &Terminal, time: f32) -> Option<(u8, Rgba)> {
    match t.agent.state {
        AgentState::NeedsYou => Some((4, theme::t().attention)),
        AgentState::Done => Some((3, theme::t().done)),
        _ if t.attention => Some((2, theme::t().attention)),
        AgentState::Working => {
            let pulse = 0.5 + 0.5 * (time * std::f32::consts::TAU * 0.9).sin();
            let mut c = theme::t().accent;
            c[3] = 0.3 + 0.7 * pulse;
            Some((1, c))
        }
        _ => None,
    }
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
pub fn text(out: &mut Vec<Quad>, fonts: &mut Fonts, x: f32, y: f32, s: &str, color: Rgba, max_w: f32) -> f32 {
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
    let mut k = Kit::new(out, fonts, l.s);
    if let Some(f) = active.and_then(|t| t.frozen.as_ref()) {
        let claude = active.is_some_and(|t| t.agent.session_id.is_some() && t.agent.state != crate::terminal::AgentState::None);
        let sub = match &f.command {
            Some(cmd) => format!("Al reanudar se vuelve a ejecutar: {cmd}"),
            None if claude => "Al reanudar, Claude retoma la misma sesión".to_string(),
            None => "Al reanudar se abre la shell en su carpeta".to_string(),
        };
        draw_suspended(&mut k, l, &sub);
    }
    if let Some(m) = v.menu {
        draw_menu(&mut k, m);
    }
    if active.is_some_and(Terminal::is_suspended) { None } else { ime_cell }
}

fn draw_menu(k: &mut Kit, m: &Menu) {
    let s = k.s;
    let b = m.bounds(s);
    k.panel(b, 8.0);
    let th = theme::t();
    for (i, r) in m.rows(s) {
        let item = &m.items[i];
        if item.action.is_none() {
            k.rect(Rect { x: r.x + 6.0 * s, y: r.y + r.h / 2.0, w: r.w - 12.0 * s, h: s }, th.border);
            continue;
        }
        let hovered = m.hover == Some(i);
        if hovered {
            k.rounded(r, 5.0, th.active);
        }
        let color = if hovered { th.text } else { theme::mix(th.text, th.text_dim, 0.25) };
        k.icon(r.x + 16.0 * s, r.y + r.h / 2.0, item.icon, 14.0, color);
        let kw = k.kbd_width(item.keys);
        k.text_in(Rect { w: r.w - kw - 40.0 * s, ..r }, r.x + 32.0 * s, item.label, Text::body().color(color));
        if !item.keys.is_empty() {
            k.kbd(r.x + r.w - 8.0 * s, r.y + r.h / 2.0, item.keys);
        }
    }
}

/// Vista previa de un tab suspendido: su última pantalla, atenuada, con un aviso abajo.
fn draw_suspended(k: &mut Kit, l: &Layout, sub: &str) {
    let s = l.s;
    let th = theme::t();
    let x0 = l.sidebar_w();
    let top = l.titlebar_h();
    k.rect(Rect { x: x0, y: top, w: l.w - x0, h: l.h - top }, theme::alpha(th.bg, 0.55));

    let w = (430.0 * s).min(l.w - x0 - 24.0 * s);
    let h = 64.0 * s;
    let r = Rect { x: x0 + (l.w - x0 - w) / 2.0, y: l.h - h - 28.0 * s, w, h };
    k.panel(r, 10.0);
    let icon_box = Rect { x: r.x + 14.0 * s, y: r.y + (h - 34.0 * s) / 2.0, w: 34.0 * s, h: 34.0 * s };
    k.rounded(icon_box, 8.0, th.active);
    k.icon(icon_box.x + icon_box.w / 2.0, icon_box.y + icon_box.h / 2.0, icons::MOON, 17.0, th.text);
    let tx = icon_box.x + icon_box.w + 12.0 * s;
    k.text(tx, r.y + 13.0 * s, "Tab suspendido", Text::label(), r.w - 200.0 * s);
    k.text(tx, r.y + 33.0 * s, sub, Text::small(), r.w - 200.0 * s);
    let right = r.x + r.w - 14.0 * s;
    let x = k.kbd(right, r.y + 22.0 * s, &["↩"]);
    let tw = k.measure("Reanudar", Text::small());
    k.text(x - 6.0 * s - tw, r.y + 22.0 * s - k.line_h(Text::small()) / 2.0, "Reanudar", Text::small(), tw + 1.0);
    let x = k.kbd(right, r.y + 44.0 * s, &["⌘", "⇧", "S"]);
    let tw = k.measure("El grupo", Text::small());
    k.text(x - 6.0 * s - tw, r.y + 44.0 * s - k.line_h(Text::small()) / 2.0, "El grupo", Text::small(), tw + 1.0);
}

fn draw_sidebar(out: &mut Vec<Quad>, fonts: &mut Fonts, l: &Layout, v: &View) {
    let s = l.s;
    let th = theme::t();
    let mut k = Kit::new(out, fonts, s);
    k.rect(Rect { x: 0.0, y: 0.0, w: l.sidebar_w(), h: l.h }, th.chrome);
    k.rect(Rect { x: l.sidebar_w() - s, y: 0.0, w: s, h: l.h }, th.border);

    // Encabezado: «GRUPOS» y el engranaje de configuración.
    let g = l.settings_rect();
    k.text_in(g, 18.0 * s, "GRUPOS", Text::caption());
    k.icon_button(g, icons::SETTINGS, 15.0, v.hover == Hit::Settings);
    k.icon_button(l.new_group_button(), icons::PLUS, 15.0, v.hover == Hit::NewGroup);

    // Buscador: con la búsqueda cerrada muestra el atajo.
    let sr = l.search_rect();
    match v.search {
        Some((query, ..)) => k.input(sr, query, "Buscar tab o grupo", Some(icons::SEARCH), true),
        None => {
            k.input(sr, "", "Buscar", Some(icons::SEARCH), false);
            k.kbd(sr.x + sr.w - 6.0 * s, sr.y + sr.h / 2.0, &["⌘", "P"]);
        }
    }

    // Resultados de la búsqueda en lugar de la lista.
    if let Some((query, selected, results)) = v.search.filter(|(q, ..)| !q.trim().is_empty()) {
        if results.is_empty() {
            k.text(l.group_rect(0).x + 12.0 * s, l.group_rect(0).y + 7.0 * s, &format!("Nada coincide con «{}»", query.trim()), Text::small(), sr.w - 20.0 * s);
        }
        for (i, f) in results.iter().enumerate() {
            if !l.row_visible(i) {
                continue;
            }
            let r = l.group_rect(i);
            if i == selected {
                k.rounded(r, 6.0, th.active);
            } else if v.hover == Hit::Group(i) {
                k.rounded(r, 6.0, th.hover);
            }
            let Some(grp) = v.ws.groups.get(f.group) else { continue };
            let cy = r.y + r.h / 2.0;
            match f.tab {
                None => {
                    k.icon(r.x + 18.0 * s, cy, icons::FOLDER, 14.0, th.text_dim);
                    k.text_in(r, r.x + 32.0 * s, &grp.name, Text::label());
                }
                Some(ti) => {
                    let t = grp.tabs.get(ti).and_then(|id| v.terms.get(id));
                    let icon = match t {
                        Some(t) if t.is_suspended() => icons::MOON,
                        Some(t) if t.agent.state != AgentState::None => icons::SPARKLES,
                        _ => icons::TERMINAL,
                    };
                    k.icon(r.x + 18.0 * s, cy, icon, 14.0, th.text_dim);
                    let fallback = format!("Tab {}", ti + 1);
                    let label = t.and_then(Terminal::label).unwrap_or(&fallback);
                    let gw = k.measure(&grp.name, Text::small()).min(70.0 * s);
                    k.text_in(Rect { w: r.w - gw - 12.0 * s, ..r }, r.x + 32.0 * s, label, Text::body());
                    k.text_in(r, r.x + r.w - gw - 8.0 * s, &grp.name, Text::small());
                }
            }
        }
        scrollbar(&mut k, l, results.len());
        draw_footer(&mut k, l, v);
        return;
    }

    for (i, grp) in v.ws.groups.iter().enumerate() {
        if !l.row_visible(i) {
            continue;
        }
        let r = l.group_rect(i);
        let active = i == v.ws.active;
        if active {
            k.rounded(r, 6.0, th.active);
            k.rounded(Rect { x: r.x, y: r.y + 8.0 * s, w: 3.0 * s, h: r.h - 16.0 * s }, 1.5, th.accent);
        } else if v.hover == Hit::Group(i) {
            k.rounded(r, 6.0, th.hover);
        }
        if v.drop_group == Some(i) {
            k.bordered(r, 6.0, theme::mix(th.chrome, th.accent, 0.14), th.accent);
        }
        let asleep = grp.tabs.iter().all(|id| v.terms.get(id).is_some_and(Terminal::is_suspended));
        let count = grp.tabs.len().to_string();
        let cy = r.y + r.h / 2.0;
        // A la derecha: cantidad de tabs (tag) y, antes, el estado más urgente o la luna.
        let badge_style = Text::small().weight(510);
        let badge_w = k.measure(&count, badge_style) + 12.0 * s;
        let bx = r.x + r.w - 8.0 * s - badge_w;
        k.tag(bx, cy, &count, th.text_dim, if active { th.hover } else { th.active });
        let worst = grp.tabs.iter().filter_map(|id| v.terms.get(id)).filter_map(|t| status(t, v.time)).max_by_key(|(p, _)| *p);
        let mut name_right = bx - 6.0 * s;
        // Grupo pasado de su límite de memoria.
        if v.over_groups.get(i).copied().unwrap_or(false) {
            k.icon(name_right - 7.0 * s, cy, icons::CIRCLE_ALERT, 13.0, th.danger);
            name_right -= 18.0 * s;
        }
        if let Some((_, color)) = worst {
            k.dot(name_right - 3.0 * s, cy, 7.0, color);
            name_right -= 14.0 * s;
        } else if asleep {
            k.icon(name_right - 4.0 * s, cy, icons::MOON, 13.0, th.text_dim);
            name_right -= 16.0 * s;
        }
        // Carpeta: abierta y de color en el grupo activo.
        let (icon, icon_color) = if active { (icons::FOLDER_OPEN, th.accent) } else { (icons::FOLDER, th.text_dim) };
        k.icon(r.x + 18.0 * s, cy, icon, 14.0, if asleep { th.muted } else { icon_color });
        let tx = r.x + 32.0 * s;
        match v.editing {
            Some((Edit::Group(gi), buf)) if gi == i => {
                k.input(Rect { x: r.x + 4.0 * s, y: r.y + 3.0 * s, w: name_right - r.x - 4.0 * s, h: r.h - 6.0 * s }, buf, "Nombre del grupo", None, true);
            }
            _ => {
                let style = if active && !asleep { Text::label() } else { Text::body().color(th.text_dim) };
                k.text_in(Rect { w: name_right - r.x, ..r }, tx, &grp.name, style);
            }
        }
    }

    let n = v.ws.groups.len();
    if l.row_visible(n) {
        let r = l.group_rect(n);
        let hovered = v.hover == Hit::NewGroup;
        if hovered {
            k.rounded(r, 6.0, th.hover);
        }
        let color = if hovered { th.text } else { th.text_dim };
        k.icon(r.x + 18.0 * s, r.y + r.h / 2.0, icons::PLUS, 14.0, color);
        k.text_in(r, r.x + 32.0 * s, "Nuevo grupo", Text::body().color(color));
    }
    scrollbar(&mut k, l, n + 1);
    draw_footer(&mut k, l, v);
}

/// Barrita de desplazamiento de la lista, si no cabe entera.
fn scrollbar(k: &mut Kit, l: &Layout, total: usize) {
    let rows = l.list_rows();
    if total <= rows {
        return;
    }
    let s = l.s;
    let top = LIST_TOP * s;
    let track_h = rows as f32 * GROUP_ROW_H * s;
    let h = (track_h * rows as f32 / total as f32).max(20.0 * s);
    let y = top + (track_h - h) * l.list_scroll as f32 / (total - rows) as f32;
    k.rounded(Rect { x: l.sidebar_w() - 5.0 * s, y, w: 3.0 * s, h }, 1.5, theme::t().border);
}

/// Pie de la barra lateral: medidores, línea, Despierto y versión.
fn draw_footer(k: &mut Kit, l: &Layout, v: &View) {
    let s = l.s;
    let th = theme::t();
    draw_meters(k, l, v);
    let top = l.awake_rect().y - 8.0 * s;
    k.rect(Rect { x: 12.0 * s, y: top, w: l.sidebar_w() - 24.0 * s, h: s }, th.border);
    draw_awake(k, l, v);
    draw_update(k, l, v);
}

/// Medidores compactos de Telar completo: CPU y GPU en % de la máquina, RAM en GB.
fn draw_meters(k: &mut Kit, l: &Layout, v: &View) {
    let s = l.s;
    let th = theme::t();
    let r = l.meters_rect();
    if v.hover == Hit::Stats {
        k.rounded(r, 7.0, th.hover);
    }
    let Some(sample) = v.stats else {
        k.text_in(r, r.x + 12.0 * s, "Midiendo uso…", Text::small());
        return;
    };
    let levels = sample.levels();
    let values = [format!("{:.0}%", sample.cpu), crate::stats::human_bytes(sample.mem), format!("{:.0}%", sample.gpu)];
    let labels = ["CPU", "RAM", "GPU"];
    let pad = 10.0 * s;
    let gap = 10.0 * s;
    let col_w = (r.w - 2.0 * pad - 2.0 * gap) / 3.0;
    for i in 0..3 {
        let x = r.x + pad + i as f32 * (col_w + gap);
        let (warn, crit) = statsview::THRESHOLDS[i];
        let color = statsview::level_color(levels[i], warn, crit);
        k.text(x, r.y + 3.0 * s, labels[i], Text::caption(), col_w);
        k.text(x, r.y + 15.0 * s, &values[i], Text::small().weight(590).color(th.text), col_w);
        // RAM: contra el límite si lo hay; si no, la barra llena es la mitad de la memoria del Mac.
        let (fill, color) = if i == 1 {
            let (level, warn, crit, fill) = crate::limits::ram_meter(sample.mem, sample.mem_machine, v.ram_limit_mb);
            (fill, statsview::level_color(level, warn, crit))
        } else {
            (levels[i], color)
        };
        let bar = Rect { x, y: r.y + r.h - 12.0 * s, w: col_w, h: 4.0 * s };
        k.rounded(bar, 2.0, th.border);
        k.rounded(Rect { w: (bar.w * fill.clamp(0.0, 1.0)).max(bar.h), ..bar }, 2.0, color);
    }
}

/// Caja de ícono de 28 pt con un círculo de estado abajo a la derecha (pie de la barra).
fn status_icon(k: &mut Kit, r: Rect, icon: char, icon_color: Rgba, box_bg: Rgba, dot: Option<Rgba>, ring: Rgba) {
    let s = k.s;
    let b = Rect { x: r.x + 8.0 * s, y: r.y + (r.h - 28.0 * s) / 2.0, w: 28.0 * s, h: 28.0 * s };
    k.rounded(b, 7.0, box_bg);
    k.icon(b.x + b.w / 2.0, b.y + b.h / 2.0, icon, 15.0, icon_color);
    if let Some(c) = dot {
        let (cx, cy) = (b.x + b.w - s, b.y + b.h - s);
        k.dot(cx, cy, 11.0, ring);
        k.dot(cx, cy, 7.0, c);
    }
}

/// «Despierto» (caffeinate): taza, estado y atajo.
fn draw_awake(k: &mut Kit, l: &Layout, v: &View) {
    let s = l.s;
    let th = theme::t();
    let r = l.awake_rect();
    let hovered = v.hover == Hit::Awake;
    if hovered {
        k.rounded(r, 7.0, th.hover);
    }
    let bg = if hovered { th.hover } else { th.chrome };
    let (mode, running) = v.awake;
    let on = mode != awake::Mode::Off;
    let icon_color = if running { th.done } else if on { th.text } else { th.text_dim };
    let box_bg = if running { theme::mix(bg, th.done, 0.16) } else { th.active };
    status_icon(k, r, icons::COFFEE, icon_color, box_bg, Some(if running { th.done } else { th.muted }), bg);
    let tx = r.x + 46.0 * s;
    let kw = k.kbd_width(&["⌘", "⇧", "C"]);
    let max = r.x + r.w - tx - kw - 14.0 * s;
    let desc = match mode {
        awake::Mode::Off => "Apagado",
        awake::Mode::On => "Siempre",
        awake::Mode::Auto => "Con Claude",
    };
    k.text(tx, r.y + 7.0 * s, "Despierto", Text::label().color(if on { th.text } else { th.text_dim }), max);
    k.text(tx, r.y + 25.0 * s, desc, Text::small(), max);
    k.kbd(r.x + r.w - 8.0 * s, r.y + r.h / 2.0, &["⌘", "⇧", "C"]);
}

/// Versión y actualizaciones: ícono de estado, texto y botón según el estado.
fn draw_update(k: &mut Kit, l: &Layout, v: &View) {
    let s = l.s;
    let th = theme::t();
    let r = l.update_rect();
    let hovered = v.hover == Hit::Update;
    if hovered {
        k.rounded(r, 7.0, th.hover);
    }
    let bg = if hovered { th.hover } else { th.chrome };
    let pct;
    let (icon, icon_color, desc): (char, Rgba, &str) = match v.update {
        Status::Idle => (icons::TERMINAL, th.text_dim, "Buscar versión"),
        Status::Checking => (icons::TERMINAL, th.text_dim, "Buscando…"),
        Status::UpToDate => (icons::CIRCLE_CHECK, th.done, "Al día"),
        Status::Available(_) => (icons::ARROW_DOWN_CIRCLE, th.accent, "Versión nueva"),
        Status::Downloading(..) => {
            pct = format!("Descargando {:.0} %", v.update.progress().unwrap_or(0.0) * 100.0);
            (icons::DOWNLOAD, th.accent, pct.as_str())
        }
        Status::Applying(_) => (icons::DOWNLOAD, th.accent, "Aplicando…"),
        Status::Ready(_) => (icons::ROCKET, th.done, "Lista para usar"),
        Status::Error(e) => (icons::CIRCLE_ALERT, th.attention, e.as_str()),
    };
    status_icon(k, r, icon, icon_color, th.active, None, bg);
    let tx = r.x + 46.0 * s;

    // Botón a la derecha (o ninguno mientras trabaja).
    let bh = 24.0 * s;
    let by = r.y + (r.h - bh) / 2.0;
    let (label, variant) = match v.update {
        Status::Available(rel) => (Some(format!("Actualizar a {}", rel.version)), Variant::Primary),
        Status::Ready(_) => (Some("Reiniciar".to_string()), Variant::Primary),
        _ => (None, Variant::Ghost),
    };
    let mut right = r.x + r.w - 8.0 * s;
    match &label {
        Some(text) => {
            let short = if matches!(v.update, Status::Available(_)) { "Actualizar" } else { text.as_str() };
            let bw = k.measure(short, Text::label()) + 20.0 * s;
            let b = Rect { x: right - bw, y: by, w: bw, h: bh };
            k.button(b, short, None, variant, hovered);
            right = b.x - 8.0 * s;
        }
        None if matches!(v.update, Status::Idle | Status::UpToDate | Status::Error(_)) => {
            let b = Rect { x: right - bh, y: by, w: bh, h: bh };
            k.icon_button(b, icons::REFRESH, 14.0, hovered);
            right = b.x - 6.0 * s;
        }
        None => {}
    }
    let title = match v.update {
        Status::Available(rel) => format!("Telar {}", rel.version),
        _ => format!("Telar {}", update::VERSION),
    };
    let desc = if let Status::Available(_) = v.update { format!("Tienes la {}", update::VERSION) } else { desc.to_string() };
    k.text(tx, r.y + 7.0 * s, &title, Text::label(), right - tx);
    k.text(tx, r.y + 25.0 * s, &desc, Text::small().color(if matches!(v.update, Status::Error(_)) { th.attention } else { th.text_dim }), right - tx);

    if let Some(p) = v.update.progress() {
        k.progress(Rect { x: tx, y: r.y + r.h - 6.0 * s, w: r.x + r.w - 8.0 * s - tx, h: 3.0 * s }, p);
    }
}

fn draw_tabbar(out: &mut Vec<Quad>, fonts: &mut Fonts, l: &Layout, v: &View) -> Option<()> {
    let s = l.s;
    let th = theme::t();
    let mut k = Kit::new(out, fonts, s);
    let x0 = l.sidebar_w();
    k.rect(Rect { x: x0, y: 0.0, w: l.w - x0, h: l.titlebar_h() }, th.chrome);
    k.rect(Rect { x: x0, y: l.titlebar_h() - s, w: l.w - x0, h: s }, th.border);

    let g = v.ws.group()?;
    for (i, id) in g.tabs.iter().enumerate() {
        let r = l.tab_rect(i);
        let active = i == g.active;
        let hovered = matches!(v.hover, Hit::Tab(t) | Hit::CloseTab(t) if t == i);
        if active {
            k.rounded(r, 6.0, th.active);
        } else if hovered {
            k.rounded(r, 6.0, th.hover);
        }
        let fallback = format!("Tab {}", i + 1);
        let title = v.terms.get(id).and_then(Terminal::label).unwrap_or(&fallback);
        let close = l.tab_close_rect(i);
        let suspended = v.terms.get(id).is_some_and(Terminal::is_suspended);
        let style = if active && !suspended { Text::label() } else { Text::body().color(th.text_dim) };
        let cy = r.y + r.h / 2.0;
        let mut tx = r.x + 10.0 * s;
        if suspended {
            k.icon(tx + 6.0 * s, cy, icons::MOON, 13.0, th.text_dim);
            tx += 18.0 * s;
        } else if let Some((_, color)) = v.terms.get(id).and_then(|t| status(t, v.time)) {
            k.dot(tx + 3.5 * s, cy, 7.0, color);
            tx += 14.0 * s;
        }
        match v.editing {
            Some((Edit::Tab(e), buf)) if e == *id => {
                k.input(Rect { x: tx - 4.0 * s, y: r.y + 2.0 * s, w: close.x - tx, h: r.h - 4.0 * s }, buf, "Nombre del tab", None, true);
            }
            _ => {
                k.text_in(Rect { w: close.x - r.x - 4.0 * s, ..r }, tx, title, style);
            }
        }
        if active || hovered {
            k.icon_button(close, icons::X, 13.0, v.hover == Hit::CloseTab(i));
        }
    }
    let r = l.new_tab_rect(g.tabs.len());
    k.icon_button(r, icons::PLUS, 15.0, v.hover == Hit::NewTab);
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
            cbg = theme::t().selection;
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
        text(fg, fonts, x, y, v.preedit, theme::t().text, n * cw + 1.0);
        fg.push(Quad::rect(x, y + ch - 2.0 * fonts.stroke, n * cw, fonts.stroke, theme::t().accent));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Layout {
        Layout::with_cell(1200.0, 780.0, 1.0, 8.0, 16.0, 3)
    }

    #[test]
    fn grilla_descuenta_barra_lateral_y_titulo() {
        let l = layout();
        assert_eq!(l.cols, ((1200.0 - (SIDEBAR_W + 2.0 * PAD_X)) / 8.0) as usize);
        assert_eq!(l.rows, ((780.0 - (TITLEBAR_H + 2.0 * PAD_Y)) / 16.0) as usize);
    }

    #[test]
    fn zonas_de_clic() {
        let l = layout();
        let g = l.group_rect(1);
        assert_eq!(l.hit(g.x + 5.0, g.y + 5.0, 3, 3), Hit::Group(1));
        let n = l.group_rect(3);
        assert_eq!(l.hit(n.x + 5.0, n.y + 5.0, 3, 3), Hit::NewGroup);
        let t = l.tab_rect(2);
        assert_eq!(l.hit(t.x + 5.0, t.y + 5.0, 3, 3), Hit::Tab(2));
        let c = l.tab_close_rect(0);
        assert_eq!(l.hit(c.x + 2.0, c.y + 2.0, 3, 3), Hit::CloseTab(0));
        let a = l.awake_rect();
        assert_eq!(l.hit(a.x + 5.0, a.y + 5.0, 3, 3), Hit::Awake);
        assert_eq!(l.hit(600.0, 400.0, 3, 3), Hit::Terminal);
        assert_eq!(l.hit(1100.0, 10.0, 3, 3), Hit::Drag);
    }

    #[test]
    fn celda_bajo_el_puntero() {
        let l = layout();
        let (x, y) = l.cell_origin(3, 2);
        let (col, row, side) = l.cell_at(x + 1.0, y + 1.0);
        assert_eq!((col, row, side), (3, 2, Side::Left));
        assert_eq!(l.cell_at(x + 7.0, y).2, Side::Right);
        assert_eq!(l.cell_at(99999.0, 99999.0).0, l.cols - 1, "fuera de la grilla se acota");
    }

    fn menu(l: &Layout, x: f32, y: f32) -> Menu {
        let items = vec![
            MenuItem::new('a', "a", &[], Action::Suspend),
            MenuItem::separator(),
            MenuItem::new('b', "b", &[], Action::Clear),
            MenuItem::new('c', "c", &[], Action::CloseTab),
        ];
        Menu::new(x, y, Some(1), 0, items, l)
    }

    #[test]
    fn lista_con_desplazamiento() {
        let mut l = Layout::with_cell(1200.0, 780.0, 1.0, 8.0, 16.0, 1);
        let rows = l.list_rows();
        assert!(rows >= 5 && l.group_rect(rows - 1).y + l.group_rect(rows - 1).h <= l.meters_rect().y, "la lista no pisa los medidores");
        assert!(l.row_visible(0) && !l.row_visible(rows));
        l.list_scroll = 3;
        assert!(!l.row_visible(2) && l.row_visible(3));
        assert_eq!(l.group_rect(3).y, l.group_rect(0).y + 3.0 * GROUP_ROW_H, "la fila 3 queda arriba");
        assert_eq!(l.group_rect(3).y, LIST_TOP);
        // Con 17 grupos, la fila «Nuevo grupo» (17) solo responde si está a la vista.
        assert_eq!(l.hit(20.0, l.group_rect(3).y + 5.0, 17, 1), Hit::Group(3));
        assert_eq!(l.hit(20.0, l.search_rect().y + 5.0, 17, 1), Hit::Search);
    }

    #[test]
    fn menu_no_se_sale_de_la_ventana() {
        let l = layout();
        let m = menu(&l, 1190.0, 770.0);
        let b = m.bounds(1.0);
        assert!(b.x + b.w <= 1200.0 && b.y + b.h <= 780.0);
    }

    #[test]
    fn menu_flechas_saltan_separadores_y_dan_la_vuelta() {
        let l = layout();
        let mut m = menu(&l, 10.0, 10.0);
        m.step(1);
        assert_eq!(m.hover, Some(0));
        m.step(1);
        assert_eq!(m.hover, Some(2), "salta el separador");
        m.step(1);
        m.step(1);
        assert_eq!(m.hover, Some(0), "vuelve al principio");
        m.step(-1);
        assert_eq!(m.hover, Some(3));
    }

    #[test]
    fn menu_clic_en_separador_no_cuenta() {
        let l = layout();
        let m = menu(&l, 10.0, 10.0);
        let rows: Vec<(usize, Rect)> = m.rows(1.0).collect();
        let (_, sep) = rows[1];
        assert_eq!(m.item_at(sep.x + 5.0, sep.y + 2.0, 1.0), None);
        let (_, b) = rows[2];
        assert_eq!(m.item_at(b.x + 5.0, b.y + 5.0, 1.0), Some(2));
    }
}
