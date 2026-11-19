//! Modal de configuración (⌘,). Barra de secciones a la izquierda y el contenido de la
//! sección a la derecha, dibujado con el kit (`kit.rs`). Para sumar una sección:
//! agregarla a `SECTIONS`, su geometría, sus clics y su dibujo.
//!
//! Los cambios se aplican al instante (la terminal de atrás sirve de vista previa)
//! y se guardan solos en `config.json`.

use std::path::Path;
use std::time::Duration;

use crate::appicon;
use crate::idle;
use crate::font::Fonts;
use crate::fontlist::{self, Family};
use crate::kit::{Kit, Text, Variant, icons};
use crate::renderer::Quad;
use crate::theme::{self, Theme};
use crate::ui::{Layout, Rect};

pub const SECTIONS: &[(char, &str)] = &[
    (icons::HOME, "General"),
    (icons::PALETTE, "Apariencia"),
    (icons::IMAGE, "Ícono"),
    (icons::GAUGE, "Rendimiento"),
    (icons::TIMER, "Inactividad"),
];

/// Sección General: la carpeta de inicio.
pub const GENERAL_SECTION: usize = 0;
/// Sección Apariencia: tema, fuente y tamaño (la única que usa el teclado).
pub const APPEARANCE_SECTION: usize = 1;

/// Sección de límites de memoria.
pub const PERF_SECTION: usize = 3;
const PERF_ROW_H: f32 = 40.0;

/// Datos de la sección Rendimiento.
pub struct Perf {
    pub total_limit_mb: u64,
    pub total_used: u64,
    pub over_total: bool,
    pub auto_suspend: bool,
    /// Nombre, límite (MB), uso (bytes) y si está pasado, por grupo.
    pub groups: Vec<(String, u64, u64, bool)>,
}

/// Sección de suspensión por inactividad.
pub const IDLE_SECTION: usize = 4;
const IDLE_ROW_H: f32 = 38.0;

/// Datos de la sección Inactividad.
pub struct IdleView {
    pub rules: idle::Rules,
    pub groups: Vec<IdleGroup>,
}

pub struct IdleGroup {
    pub name: String,
    pub exempt: bool,
    /// Es el grupo abierto (cuenta como en uso).
    pub open: bool,
    /// Tiempo desde el último uso de cualquiera de sus tabs.
    pub idle: Duration,
    pub suspended: usize,
    pub tabs: usize,
}

/// Lo que el modal muestra «en uso».
pub struct Ctx<'a> {
    pub family: &'a str,
    pub size: f32,
    pub app_icon: &'a str,
    pub perf: Perf,
    pub idle: IdleView,
    /// Carpeta de inicio elegida y si todavía existe.
    pub start_dir: Option<&'a Path>,
    /// Qué muestra la barra lateral abajo: el bloque de Claude y los medidores de consumo.
    pub show_claude: bool,
    pub show_meters: bool,
}

/// Sección del ícono de la app.
pub const ICON_SECTION: usize = 2;
const ICON_CARD_H: f32 = 168.0;

const PANEL_W: f32 = 760.0;
const PANEL_H: f32 = 620.0;
const NAV_W: f32 = 190.0;
const ROW_H: f32 = 28.0;
const THEME_COLS: usize = 3;
const THEME_H: f32 = 58.0;

/// Qué hay bajo el puntero dentro del modal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Outside,
    Inside,
    Close,
    Section(usize),
    Theme(usize),
    /// Fila de la lista de fuentes (índice dentro de la lista filtrada).
    Font(usize),
    SizeDown,
    SizeUp,
    /// Tarjeta de ícono: las incluidas y, al final, el propio.
    Icon(usize),
    /// − / + de un límite: 0 es Telar completo; 1.. cada grupo.
    LimitDown(usize),
    LimitUp(usize),
    /// Interruptor de suspensión automática.
    AutoSuspend,
    /// «Elegir archivo…» para el ícono propio.
    PickIcon,
    /// − / + del tiempo sin uso de un tab o de un grupo.
    IdleTabDown,
    IdleTabUp,
    IdleGroupDown,
    IdleGroupUp,
    /// Interruptores: incluir tabs donde Claude espera respuesta o con un comando corriendo.
    IdleWaiting,
    IdleRunning,
    /// Interruptor de un grupo: se suspende por inactividad o no.
    IdleExempt(usize),
    /// «Elegir carpeta…» y «Quitar» de la carpeta de inicio.
    PickStartDir,
    ClearStartDir,
    /// Interruptores de la barra lateral: bloque de Claude y medidores de consumo.
    ShowClaude,
    ShowMeters,
}

#[derive(Default)]
pub struct Settings {
    pub section: usize,
    /// Texto para filtrar la lista de fuentes (se escribe con el modal abierto).
    pub filter: String,
    /// Fila elegida con el teclado (índice dentro de la lista filtrada).
    pub selected: usize,
    /// Primera fila visible.
    pub scroll: usize,
    pub hover: Option<Hit>,
    /// Error al cambiar el ícono (se muestra en su sección).
    pub icon_error: Option<String>,
    /// Primera fila visible de la lista de grupos en Rendimiento.
    pub perf_scroll: usize,
    /// Primera fila visible de la lista de grupos en Inactividad.
    pub idle_scroll: usize,
}

/// Geometría de la sección Rendimiento.
struct PerfGeometry {
    total: Rect,
    total_down: Rect,
    total_up: Rect,
    groups_y: f32,
    /// Filas visibles: índice del grupo, fila, − y +.
    rows: Vec<(usize, Rect, Rect, Rect)>,
    capacity: usize,
    auto: Rect,
    switch: Rect,
    note_y: f32,
}

/// − y + a la derecha de una fila.
fn stepper(r: Rect, s: f32) -> (Rect, Rect) {
    let b = 26.0 * s;
    let up = Rect { x: r.x + r.w - 10.0 * s - b, y: r.y + (r.h - b) / 2.0, w: b, h: b };
    let down = Rect { x: up.x - 96.0 * s - b, ..up };
    (down, up)
}

fn perf_geometry(g: &Geometry, s: f32, n_groups: usize, scroll: usize) -> PerfGeometry {
    let (cx, cw) = (g.content_x, g.content_w);
    let total = Rect { x: cx, y: g.themes_y + 22.0 * s, w: cw, h: 50.0 * s };
    let (total_down, total_up) = stepper(total, s);
    let groups_y = total.y + total.h + 22.0 * s;
    let note_y = g.panel.y + g.panel.h - 46.0 * s;
    let auto = Rect { x: cx, y: note_y - 66.0 * s, w: cw, h: 54.0 * s };
    let list_top = groups_y + 22.0 * s;
    let capacity = (((auto.y - 12.0 * s - list_top) / (PERF_ROW_H * s)).floor() as usize).max(1);
    let rows = (scroll..n_groups.min(scroll + capacity))
        .map(|gi| {
            let r = Rect { x: cx, y: list_top + (gi - scroll) as f32 * PERF_ROW_H * s, w: cw, h: PERF_ROW_H * s - 4.0 * s };
            let (d, u) = stepper(r, s);
            (gi, r, d, u)
        })
        .collect();
    let switch = Rect { x: cx + 14.0 * s, y: auto.y + (auto.h - 20.0 * s) / 2.0, w: 34.0 * s, h: 20.0 * s };
    PerfGeometry { total, total_down, total_up, groups_y, rows, capacity, auto, switch, note_y }
}

/// Geometría de la sección Inactividad.
struct IdleGeometry {
    tab: Rect,
    tab_down: Rect,
    tab_up: Rect,
    group: Rect,
    group_down: Rect,
    group_up: Rect,
    waiting: Rect,
    running: Rect,
    groups_y: f32,
    /// Filas visibles: índice del grupo, fila e interruptor.
    rows: Vec<(usize, Rect, Rect)>,
    capacity: usize,
    note_y: f32,
}

/// Interruptor a la izquierda de una tarjeta.
fn left_switch(r: Rect, s: f32) -> Rect {
    Rect { x: r.x + 14.0 * s, y: r.y + (r.h - 20.0 * s) / 2.0, w: 34.0 * s, h: 20.0 * s }
}

fn idle_geometry(g: &Geometry, s: f32, n_groups: usize, scroll: usize) -> IdleGeometry {
    let (cx, cw) = (g.content_x, g.content_w);
    let tab = Rect { x: cx, y: g.themes_y + 22.0 * s, w: cw, h: 50.0 * s };
    let (tab_down, tab_up) = stepper(tab, s);
    let group = Rect { y: tab.y + tab.h + 8.0 * s, ..tab };
    let (group_down, group_up) = stepper(group, s);
    let waiting = Rect { y: group.y + group.h + 8.0 * s, h: 46.0 * s, ..tab };
    let running = Rect { y: waiting.y + waiting.h + 8.0 * s, ..waiting };
    let groups_y = running.y + running.h + 20.0 * s;
    let note_y = g.panel.y + g.panel.h - 34.0 * s;
    let list_top = groups_y + 22.0 * s;
    let capacity = (((note_y - 10.0 * s - list_top) / (IDLE_ROW_H * s)).floor() as usize).max(1);
    let rows = (scroll..n_groups.min(scroll + capacity))
        .map(|gi| {
            let r = Rect { x: cx, y: list_top + (gi - scroll) as f32 * IDLE_ROW_H * s, w: cw, h: IDLE_ROW_H * s - 4.0 * s };
            let sw = Rect { x: r.x + r.w - 10.0 * s - 34.0 * s, y: r.y + (r.h - 20.0 * s) / 2.0, w: 34.0 * s, h: 20.0 * s };
            (gi, r, sw)
        })
        .collect();
    IdleGeometry { tab, tab_down, tab_up, group, group_down, group_up, waiting, running, groups_y, rows, capacity, note_y }
}

/// Rectángulos del modal para una ventana dada (los usan el dibujo y los clics).
struct Geometry {
    panel: Rect,
    close: Rect,
    nav: Vec<Rect>,
    content_x: f32,
    content_w: f32,
    title_y: f32,
    themes_y: f32,
    themes: Vec<Rect>,
    font_label_y: f32,
    search: Rect,
    list: Rect,
    rows: usize,
    size_y: f32,
    size_down: Rect,
    size_up: Rect,
    icon_cards: Vec<Rect>,
    pick: Rect,
    /// Sección General: la tarjeta con la carpeta y sus dos botones.
    start_card: Rect,
    start_pick: Rect,
    start_clear: Rect,
    /// Sección General: los interruptores de la barra lateral.
    sidebar_y: f32,
    show_claude: Rect,
    show_meters: Rect,
}

fn geometry(l: &Layout) -> Geometry {
    let s = l.s();
    let (w, h) = ((PANEL_W * s).min(l.w() - 40.0 * s), (PANEL_H * s).min(l.h() - 40.0 * s));
    let panel = Rect { x: (l.w() - w) / 2.0, y: (l.h() - h) / 2.0, w, h };
    let close = Rect { x: panel.x + w - 38.0 * s, y: panel.y + 12.0 * s, w: 26.0 * s, h: 26.0 * s };
    let nav = (0..SECTIONS.len())
        .map(|i| Rect { x: panel.x + 12.0 * s, y: panel.y + 54.0 * s + i as f32 * 34.0 * s, w: (NAV_W - 24.0) * s, h: 30.0 * s })
        .collect();
    let content_x = panel.x + (NAV_W + 24.0) * s;
    let content_w = panel.x + w - 28.0 * s - content_x;

    let title_y = panel.y + 18.0 * s;
    let themes_y = panel.y + 74.0 * s;
    let gap = 8.0 * s;
    let tw = (content_w - gap * (THEME_COLS - 1) as f32) / THEME_COLS as f32;
    let themes: Vec<Rect> = (0..theme::themes().len())
        .map(|i| {
            let (c, r) = (i % THEME_COLS, i / THEME_COLS);
            Rect { x: content_x + c as f32 * (tw + gap), y: themes_y + 22.0 * s + r as f32 * (THEME_H * s + gap), w: tw, h: THEME_H * s }
        })
        .collect();
    let themes_end = themes.last().map_or(themes_y, |r| r.y + r.h);

    let font_label_y = themes_end + 20.0 * s;
    let search = Rect { x: content_x, y: font_label_y + 22.0 * s, w: content_w, h: 32.0 * s };
    let size_y = panel.y + h - 50.0 * s;
    let list_y = search.y + search.h + 6.0 * s;
    let rows = (((size_y - 14.0 * s - list_y) / (ROW_H * s)).floor() as usize).max(1);
    let list = Rect { x: content_x, y: list_y, w: content_w, h: rows as f32 * ROW_H * s };
    let btn = 28.0 * s;
    let size_down = Rect { x: content_x + 80.0 * s, y: size_y, w: btn, h: btn };
    let size_up = Rect { x: size_down.x + btn + 64.0 * s, y: size_y, w: btn, h: btn };
    // Sección «Ícono»: las incluidas y la propia, en tres columnas.
    let icon_w = (content_w - gap * 2.0) / 3.0;
    let icon_cards = (0..=appicon::CHOICES.len())
        .map(|i| {
            let (c, r) = (i % 3, i / 3);
            Rect { x: content_x + c as f32 * (icon_w + gap), y: themes_y + r as f32 * (ICON_CARD_H * s + gap), w: icon_w, h: ICON_CARD_H * s }
        })
        .collect::<Vec<_>>();
    let cards_end = icon_cards.last().map_or(themes_y, |r| r.y + r.h);
    let pick = Rect { x: content_x, y: cards_end + 18.0 * s, w: 170.0 * s, h: 32.0 * s };
    let start_card = Rect { x: content_x, y: themes_y + 22.0 * s, w: content_w, h: 56.0 * s };
    let start_pick = Rect { x: content_x, y: start_card.y + start_card.h + 12.0 * s, w: 170.0 * s, h: 32.0 * s };
    let start_clear = Rect { x: start_pick.x + start_pick.w + 8.0 * s, w: 100.0 * s, ..start_pick };
    let sidebar_y = start_pick.y + start_pick.h + 22.0 * s + 3.0 * 18.0 * s + 20.0 * s;
    let show_claude = Rect { x: content_x, y: sidebar_y + 22.0 * s, w: content_w, h: 46.0 * s };
    let show_meters = Rect { y: show_claude.y + show_claude.h + 8.0 * s, ..show_claude };
    Geometry {
        panel,
        close,
        nav,
        content_x,
        content_w,
        title_y,
        themes_y,
        themes,
        font_label_y,
        search,
        list,
        rows,
        size_y,
        size_down,
        size_up,
        icon_cards,
        pick,
        start_card,
        start_pick,
        start_clear,
        sidebar_y,
        show_claude,
        show_meters,
    }
}

impl Settings {
    /// Familias que coinciden con el filtro.
    pub fn visible(&self) -> Vec<&'static Family> {
        let f = self.filter.to_lowercase();
        fontlist::families().iter().filter(|fam| fam.name.to_lowercase().contains(&f)).collect()
    }

    /// Abre con la fuente en uso elegida y a la vista.
    pub fn open(current: &str) -> Self {
        let mut st = Self::default();
        st.selected = st.visible().iter().position(|f| f.name.eq_ignore_ascii_case(current)).unwrap_or(0);
        st.scroll = st.selected.saturating_sub(2);
        st
    }

    /// Filas de grupos visibles en Rendimiento (para desplazar con la rueda).
    pub fn perf_capacity(&self, l: &Layout, n_groups: usize) -> usize {
        perf_geometry(&geometry(l), l.s(), n_groups, self.perf_scroll).capacity
    }

    /// Filas de grupos visibles en Inactividad (para desplazar con la rueda).
    pub fn idle_capacity(&self, l: &Layout, n_groups: usize) -> usize {
        idle_geometry(&geometry(l), l.s(), n_groups, self.idle_scroll).capacity
    }

    pub fn hit(&self, x: f32, y: f32, l: &Layout, _fonts: &Fonts, n_groups: usize) -> Hit {
        let g = geometry(l);
        if !g.panel.contains(x, y) {
            return Hit::Outside;
        }
        if g.close.contains(x, y) {
            return Hit::Close;
        }
        if let Some(i) = g.nav.iter().position(|r| r.contains(x, y)) {
            return Hit::Section(i);
        }
        if self.section == PERF_SECTION {
            let pg = perf_geometry(&g, l.s(), n_groups, self.perf_scroll);
            if pg.total_down.contains(x, y) {
                return Hit::LimitDown(0);
            }
            if pg.total_up.contains(x, y) {
                return Hit::LimitUp(0);
            }
            for (gi, _, d, u) in &pg.rows {
                if d.contains(x, y) {
                    return Hit::LimitDown(gi + 1);
                }
                if u.contains(x, y) {
                    return Hit::LimitUp(gi + 1);
                }
            }
            return if pg.auto.contains(x, y) { Hit::AutoSuspend } else { Hit::Inside };
        }
        if self.section == IDLE_SECTION {
            let ig = idle_geometry(&g, l.s(), n_groups, self.idle_scroll);
            let hits = [
                (ig.tab_down, Hit::IdleTabDown),
                (ig.tab_up, Hit::IdleTabUp),
                (ig.group_down, Hit::IdleGroupDown),
                (ig.group_up, Hit::IdleGroupUp),
                (ig.waiting, Hit::IdleWaiting),
                (ig.running, Hit::IdleRunning),
            ];
            if let Some((_, h)) = hits.iter().find(|(r, _)| r.contains(x, y)) {
                return *h;
            }
            return ig.rows.iter().find(|(_, r, _)| r.contains(x, y)).map_or(Hit::Inside, |(gi, _, _)| Hit::IdleExempt(*gi));
        }
        if self.section == GENERAL_SECTION {
            return if g.start_pick.contains(x, y) {
                Hit::PickStartDir
            } else if g.start_clear.contains(x, y) {
                Hit::ClearStartDir
            } else if g.show_claude.contains(x, y) {
                Hit::ShowClaude
            } else if g.show_meters.contains(x, y) {
                Hit::ShowMeters
            } else {
                Hit::Inside
            };
        }
        if self.section == ICON_SECTION {
            if let Some(i) = g.icon_cards.iter().position(|r| r.contains(x, y)) {
                return Hit::Icon(i);
            }
            return if g.pick.contains(x, y) { Hit::PickIcon } else { Hit::Inside };
        }
        if let Some(i) = g.themes.iter().position(|r| r.contains(x, y)) {
            return Hit::Theme(i);
        }
        if g.size_down.contains(x, y) {
            return Hit::SizeDown;
        }
        if g.size_up.contains(x, y) {
            return Hit::SizeUp;
        }
        if g.list.contains(x, y) {
            let row = ((y - g.list.y) / (ROW_H * l.s())) as usize + self.scroll;
            if row < self.visible().len() {
                return Hit::Font(row);
            }
        }
        Hit::Inside
    }

    /// Filas visibles de la lista (para desplazar con flechas y rueda).
    pub fn page(&self, l: &Layout, _fonts: &Fonts) -> usize {
        geometry(l).rows
    }

    /// Mueve la selección y desplaza la lista para que siga a la vista.
    pub fn select(&mut self, index: usize, page: usize) {
        let n = self.visible().len();
        if n == 0 {
            return;
        }
        self.selected = index.min(n - 1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + page {
            self.scroll = self.selected + 1 - page;
        }
    }

    pub fn scroll_by(&mut self, rows: isize, page: usize) {
        let max = self.visible().len().saturating_sub(page);
        self.scroll = (self.scroll as isize + rows).clamp(0, max as isize) as usize;
    }

    /// Dibuja el modal sobre todo lo demás, con lo que está en uso (`ctx`).
    pub fn draw(&self, out: &mut Vec<Quad>, fonts: &mut Fonts, l: &Layout, ctx: &Ctx) {
        let (family, size, app_icon) = (ctx.family, ctx.size, ctx.app_icon);
        let s = l.s();
        let g = geometry(l);
        let th = theme::t();
        let mut k = Kit::new(out, fonts, s);
        let p = g.panel;

        // Fondo atenuado, panel y barra de secciones.
        k.rect(Rect { x: 0.0, y: 0.0, w: l.w(), h: l.h() }, theme::alpha([0.0, 0.0, 0.0, 1.0], if th.dark { 0.5 } else { 0.25 }));
        k.panel(p, 12.0);
        k.rounded(Rect { w: NAV_W * s, ..p }, 12.0, th.chrome);
        k.rect(Rect { x: p.x + NAV_W * s - 12.0 * s, y: p.y, w: 12.0 * s, h: p.h }, th.chrome);
        k.rect(Rect { x: p.x + NAV_W * s, y: p.y, w: s, h: p.h }, th.border);
        k.text(p.x + 22.0 * s, p.y + 20.0 * s, "CONFIGURACIÓN", Text::caption(), NAV_W * s - 30.0 * s);
        k.icon_button(g.close, icons::X, 15.0, self.hover == Some(Hit::Close));

        for (i, (r, (icon, name))) in g.nav.iter().zip(SECTIONS).enumerate() {
            let on = i == self.section;
            if on {
                k.rounded(*r, 6.0, th.active);
            } else if self.hover == Some(Hit::Section(i)) {
                k.rounded(*r, 6.0, th.hover);
            }
            let color = if on { th.text } else { th.text_dim };
            k.icon(r.x + 18.0 * s, r.y + r.h / 2.0, *icon, 15.0, color);
            k.text_in(*r, r.x + 34.0 * s, name, Text::label().color(color));
        }

        if self.section == GENERAL_SECTION {
            self.draw_general(&mut k, &g, ctx);
            return;
        }
        if self.section == ICON_SECTION {
            self.draw_icons(&mut k, &g, app_icon);
            return;
        }
        if self.section == PERF_SECTION {
            self.draw_perf(&mut k, &g, &ctx.perf);
            return;
        }
        if self.section == IDLE_SECTION {
            self.draw_idle(&mut k, &g, &ctx.idle);
            return;
        }

        // Apariencia.
        let cx = g.content_x;
        k.text(cx, g.title_y, SECTIONS[self.section].1, Text::title(), g.content_w - 40.0 * s);
        k.text(cx, g.title_y + 22.0 * s, "Se aplica al instante y se guarda sola", Text::small(), g.content_w);

        k.text(cx, g.themes_y, "TEMA", Text::caption(), g.content_w);
        for (i, (r, t)) in g.themes.iter().zip(theme::themes()).enumerate() {
            theme_card(&mut k, *r, t, t.name == th.name, self.hover == Some(Hit::Theme(i)));
        }

        k.text(cx, g.font_label_y, "FUENTE", Text::caption(), g.content_w);
        k.input(g.search, &self.filter, "Buscar fuente…", Some(icons::SEARCH), true);
        let list = self.visible();
        if list.is_empty() {
            k.text(cx + 10.0 * s, g.list.y + 6.0 * s, "Ninguna fuente coincide", Text::small(), g.list.w);
        }
        for (row, fam) in list.iter().enumerate().skip(self.scroll).take(g.rows) {
            let y = g.list.y + (row - self.scroll) as f32 * ROW_H * s;
            let r = Rect { x: g.list.x, y, w: g.list.w - 10.0 * s, h: ROW_H * s - 2.0 * s };
            let current = fam.name.eq_ignore_ascii_case(family);
            if row == self.selected {
                k.rounded(r, 6.0, th.active);
            } else if self.hover == Some(Hit::Font(row)) {
                k.rounded(r, 6.0, th.hover);
            }
            if current {
                k.icon(r.x + 14.0 * s, r.y + r.h / 2.0, icons::CHECK, 14.0, th.accent);
            }
            let mut right = r.x + r.w - 8.0 * s;
            if fam.embedded {
                let tw = k.measure("incluida", Text::small().weight(510)) + 12.0 * s;
                right -= tw;
                k.tag(right, r.y + r.h / 2.0, "incluida", th.accent, theme::alpha(th.accent, 0.14));
                right -= 8.0 * s;
            }
            let style = if current { Text::label() } else { Text::body().color(theme::mix(th.text, th.text_dim, 0.3)) };
            k.text_in(Rect { w: right - r.x, ..r }, r.x + 30.0 * s, &fam.name, style);
        }
        if list.len() > g.rows {
            let track = g.list;
            let h = (track.h * g.rows as f32 / list.len() as f32).max(24.0 * s);
            let y = track.y + (track.h - h) * self.scroll as f32 / (list.len() - g.rows) as f32;
            k.rounded(Rect { x: track.x + track.w - 4.0 * s, y, w: 4.0 * s, h }, 2.0, th.border);
        }

        // Tamaño: − 13 pt +, y los atajos.
        let cy = g.size_y + g.size_down.h / 2.0;
        k.text_in(Rect { x: cx, y: g.size_y, w: 80.0 * s, h: g.size_down.h }, cx, "TAMAÑO", Text::caption());
        k.button(g.size_down, "", Some(icons::MINUS), Variant::Subtle, self.hover == Some(Hit::SizeDown));
        k.button(g.size_up, "", Some(icons::PLUS), Variant::Subtle, self.hover == Some(Hit::SizeUp));
        let mid = Rect { x: g.size_down.x + g.size_down.w, y: g.size_y, w: g.size_up.x - g.size_down.x - g.size_down.w, h: g.size_down.h };
        k.text_center(mid, &format!("{size:.0} pt"), Text::label());
        let right = g.content_x + g.content_w;
        let x = k.kbd(right, cy, &["⌘", "0"]);
        let x = k.kbd(x - 10.0 * s, cy, &["⌘", "−"]);
        k.kbd(x - 6.0 * s, cy, &["⌘", "+"]);
    }
}

impl Settings {
    fn draw_perf(&self, k: &mut Kit, g: &Geometry, p: &Perf) {
        let s = k.s;
        let th = theme::t();
        let cx = g.content_x;
        let pg = perf_geometry(g, s, p.groups.len(), self.perf_scroll);
        k.text(cx, g.title_y, "Rendimiento", Text::title(), g.content_w - 40.0 * s);
        k.text(cx, g.title_y + 22.0 * s, "Límites de memoria para Telar completo y para cada grupo", Text::small(), g.content_w);
        k.text(cx, g.themes_y, "LÍMITE DE MEMORIA", Text::caption(), g.content_w);

        // Una fila: nombre, uso actual y el − valor + del límite.
        let row = |k: &mut Kit, r: Rect, d: Rect, u: Rect, name: &str, used: u64, limit: u64, over: bool, i: usize, card: bool| {
            if card {
                k.bordered(r, 8.0, th.chrome, if over { th.danger } else { th.border });
            } else if over {
                k.rounded(r, 6.0, theme::alpha(th.danger, 0.10));
            }
            let used_text = format!("usa {}", crate::stats::human_bytes(used));
            let name_style = if card { Text::label() } else { Text::body() };
            let right = d.x - 12.0 * s;
            let nw = k.measure(name, name_style).min(right - r.x - 140.0 * s);
            let ty = r.y + (r.h - k.line_h(name_style)) / 2.0;
            if over {
                k.icon(r.x + 16.0 * s, r.y + r.h / 2.0, icons::CIRCLE_ALERT, 14.0, th.danger);
            }
            let x = r.x + if over { 30.0 } else { 12.0 } * s;
            k.text(x, ty, name, name_style, nw);
            k.text(x + nw + 10.0 * s, ty + 1.5 * s, &used_text, Text::small().color(if over { th.danger } else { th.text_dim }), right - x - nw - 10.0 * s);
            k.button(d, "", Some(icons::MINUS), Variant::Subtle, self.hover == Some(Hit::LimitDown(i)));
            k.button(u, "", Some(icons::PLUS), Variant::Subtle, self.hover == Some(Hit::LimitUp(i)));
            let value = Rect { x: d.x + d.w, y: d.y, w: u.x - d.x - d.w, h: d.h };
            let style = if limit == 0 { Text::body().color(th.text_dim) } else { Text::label() };
            k.text_center(value, &crate::limits::label(limit), style);
        };
        row(k, pg.total, pg.total_down, pg.total_up, "Telar completo", p.total_used, p.total_limit_mb, p.over_total, 0, true);

        k.text(cx, pg.groups_y, "POR GRUPO", Text::caption(), g.content_w);
        for (gi, r, d, u) in &pg.rows {
            let (name, limit, used, over) = &p.groups[*gi];
            row(k, *r, *d, *u, name, *used, *limit, *over, gi + 1, false);
        }
        if p.groups.len() > pg.capacity {
            let top = pg.rows.first().map_or(pg.groups_y, |r| r.1.y);
            let track = pg.capacity as f32 * PERF_ROW_H * s;
            let h = (track * pg.capacity as f32 / p.groups.len() as f32).max(20.0 * s);
            let y = top + (track - h) * self.perf_scroll as f32 / (p.groups.len() - pg.capacity) as f32;
            k.rounded(Rect { x: cx + g.content_w + 6.0 * s, y, w: 3.0 * s, h }, 1.5, th.border);
        }

        // Suspensión automática.
        let a = pg.auto;
        k.bordered(a, 8.0, th.chrome, if self.hover == Some(Hit::AutoSuspend) { theme::mix(th.border, th.text, 0.25) } else { th.border });
        k.switch(pg.switch, p.auto_suspend);
        let tx = pg.switch.x + pg.switch.w + 14.0 * s;
        k.text(tx, a.y + 9.0 * s, "Suspender tabs inactivos al pasar un límite", Text::label(), a.x + a.w - tx - 10.0 * s);
        let sub = if p.auto_suspend {
            "Se suspende el que más memoria usa; nunca el visible ni donde Claude trabaja"
        } else {
            "Solo avisa: el límite marca los indicadores en ámbar y rojo"
        };
        k.text(tx, a.y + 29.0 * s, sub, Text::small(), a.x + a.w - tx - 10.0 * s);
        k.text(cx, pg.note_y, "macOS no permite un tope duro de memoria para otros procesos: Telar lo hace", Text::small(), g.content_w);
        k.text(cx, pg.note_y + 16.0 * s, "cumplir suspendiendo tabs, que liberan su memoria y se retoman con Enter.", Text::small(), g.content_w);
    }

    fn draw_idle(&self, k: &mut Kit, g: &Geometry, v: &IdleView) {
        let s = k.s;
        let th = theme::t();
        let cx = g.content_x;
        let ig = idle_geometry(g, s, v.groups.len(), self.idle_scroll);
        let r = v.rules;
        k.text(cx, g.title_y, "Inactividad", Text::title(), g.content_w - 40.0 * s);
        k.text(cx, g.title_y + 22.0 * s, "Suspende solo lo que no usas para liberar memoria; se retoma con Enter o un clic", Text::small(), g.content_w);
        k.text(cx, g.themes_y, "SUSPENDER AUTOMÁTICAMENTE", Text::caption(), g.content_w);

        // Tarjeta con − tiempo +.
        let card = |k: &mut Kit, r: Rect, d: Rect, u: Rect, title: &str, sub: &str, min: u64, hd: Hit, hu: Hit| {
            k.bordered(r, 8.0, th.chrome, th.border);
            let right = d.x - 12.0 * s;
            k.text(r.x + 14.0 * s, r.y + 9.0 * s, title, Text::label(), right - r.x - 14.0 * s);
            k.text(r.x + 14.0 * s, r.y + 28.0 * s, sub, Text::small(), right - r.x - 14.0 * s);
            k.button(d, "", Some(icons::MINUS), Variant::Subtle, self.hover == Some(hd));
            k.button(u, "", Some(icons::PLUS), Variant::Subtle, self.hover == Some(hu));
            let value = Rect { x: d.x + d.w, y: d.y, w: u.x - d.x - d.w, h: d.h };
            let style = if min == 0 { Text::body().color(th.text_dim) } else { Text::label() };
            k.text_center(value, &idle::label(min), style);
        };
        card(k, ig.tab, ig.tab_down, ig.tab_up, "Tab sin uso", "Ni lo ves, ni escribes en él, ni muestra nada", r.tab_min, Hit::IdleTabDown, Hit::IdleTabUp);
        card(k, ig.group, ig.group_down, ig.group_up, "Grupo sin uso", "Todos sus tabs, si no usaste ninguno", r.group_min, Hit::IdleGroupDown, Hit::IdleGroupUp);

        // Interruptores.
        let toggle = |k: &mut Kit, r: Rect, on: bool, title: &str, sub: &str, h: Hit| {
            let border = if self.hover == Some(h) { theme::mix(th.border, th.text, 0.25) } else { th.border };
            k.bordered(r, 8.0, th.chrome, border);
            let sw = left_switch(r, s);
            k.switch(sw, on);
            let tx = sw.x + sw.w + 14.0 * s;
            k.text(tx, r.y + 7.0 * s, title, Text::label(), r.x + r.w - tx - 10.0 * s);
            k.text(tx, r.y + 26.0 * s, sub, Text::small(), r.x + r.w - tx - 10.0 * s);
        };
        toggle(
            k,
            ig.waiting,
            r.include_waiting,
            "Incluir tabs donde Claude espera tu respuesta",
            "Se pierde la pregunta pendiente; al reanudar, Claude vuelve a la misma sesión",
            Hit::IdleWaiting,
        );
        toggle(
            k,
            ig.running,
            r.include_running,
            "Incluir tabs con un comando corriendo",
            "Suspender lo cierra: un servidor sin logs, ssh, vim…",
            Hit::IdleRunning,
        );

        // Por grupo.
        k.text(cx, ig.groups_y, "POR GRUPO", Text::caption(), g.content_w);
        for (gi, row, sw) in &ig.rows {
            let grp = &v.groups[*gi];
            if self.hover == Some(Hit::IdleExempt(*gi)) {
                k.rounded(*row, 6.0, th.hover);
            }
            let icon = if grp.exempt { icons::PIN } else { icons::FOLDER };
            k.icon(row.x + 16.0 * s, row.y + row.h / 2.0, icon, 14.0, if grp.exempt { th.accent } else { th.text_dim });
            let x = row.x + 32.0 * s;
            let name_style = Text::body();
            let ty = row.y + (row.h - k.line_h(name_style)) / 2.0;
            let right = sw.x - 12.0 * s;
            let nw = k.measure(&grp.name, name_style).min((right - x) * 0.55);
            k.text(x, ty, &grp.name, name_style, nw);
            let mut detail = if grp.open { "abierto".to_string() } else { format!("usado {}", idle::ago(grp.idle)) };
            if grp.suspended > 0 {
                detail += &format!(" · {} de {} suspendidos", grp.suspended, grp.tabs);
            }
            if grp.exempt {
                detail = format!("nunca se suspende · {detail}");
            }
            k.text(x + nw + 10.0 * s, ty + 1.5 * s, &detail, Text::small(), right - x - nw - 10.0 * s);
            k.switch(*sw, !grp.exempt);
        }
        if v.groups.len() > ig.capacity {
            let top = ig.rows.first().map_or(ig.groups_y, |r| r.1.y);
            let track = ig.capacity as f32 * IDLE_ROW_H * s;
            let h = (track * ig.capacity as f32 / v.groups.len() as f32).max(20.0 * s);
            let y = top + (track - h) * self.idle_scroll as f32 / (v.groups.len() - ig.capacity) as f32;
            k.rounded(Rect { x: cx + g.content_w + 6.0 * s, y, w: 3.0 * s, h }, 1.5, th.border);
        }
        let note = if r.enabled() {
            "Nunca se suspende el tab visible ni uno donde Claude trabaja"
        } else {
            "Desactivado: elige un tiempo para tabs o para grupos"
        };
        k.text(cx, ig.note_y, note, Text::small(), g.content_w);
    }

    /// Sección General: la carpeta de inicio.
    fn draw_general(&self, k: &mut Kit, g: &Geometry, ctx: &Ctx) {
        let start_dir = ctx.start_dir;
        let s = k.s;
        let th = theme::t();
        let cx = g.content_x;
        k.text(cx, g.title_y, "General", Text::title(), g.content_w - 40.0 * s);
        k.text(cx, g.title_y + 22.0 * s, "Se aplica al instante y se guarda sola", Text::small(), g.content_w);

        k.text(cx, g.themes_y, "CARPETA DE INICIO", Text::caption(), g.content_w);
        let r = g.start_card;
        k.bordered(r, 8.0, th.chrome, th.border);
        let exists = start_dir.is_some_and(Path::is_dir);
        let icon_color = if exists { th.accent } else { th.text_dim };
        k.icon(r.x + 24.0 * s, r.y + r.h / 2.0, if start_dir.is_some() { icons::FOLDER_OPEN } else { icons::FOLDER }, 18.0, icon_color);
        let tx = r.x + 46.0 * s;
        let tw = r.x + r.w - 14.0 * s - tx;
        let (line1, line2) = match start_dir {
            Some(dir) => {
                let name = dir.file_name().map_or_else(|| dir.display().to_string(), |n| n.to_string_lossy().into_owned());
                let detail = if exists { short_path(dir) } else { format!("{} · ya no existe", short_path(dir)) };
                (name, detail)
            }
            None => ("Sin carpeta de inicio".to_string(), "Los tabs nuevos abren en la carpeta del tab que tienes a la vista".to_string()),
        };
        k.text(tx, r.y + 10.0 * s, &line1, Text::label(), tw);
        let detail_color = if start_dir.is_some() && !exists { th.danger } else { th.text_dim };
        k.text(tx, r.y + 30.0 * s, &line2, Text::small().color(detail_color), tw);

        k.button(g.start_pick, "Elegir carpeta…", Some(icons::FOLDER_INPUT), Variant::Subtle, self.hover == Some(Hit::PickStartDir));
        if start_dir.is_some() {
            k.button(g.start_clear, "Quitar", Some(icons::FOLDER_X), Variant::Ghost, self.hover == Some(Hit::ClearStartDir));
        }

        let mut y = g.start_pick.y + g.start_pick.h + 22.0 * s;
        let notes = [
            "Los tabs y grupos nuevos abren en esta carpeta.",
            "Un grupo con carpeta propia (la de su proyecto) sigue usando la suya.",
            "Los tabs que ya existen, y los que se restauran al abrir Telar, no cambian.",
        ];
        for note in notes {
            k.text(cx, y, note, Text::small(), g.content_w);
            y += 18.0 * s;
        }

        // Lo que muestra la barra lateral abajo.
        k.text(cx, g.sidebar_y, "BARRA LATERAL", Text::caption(), g.content_w);
        let toggle = |k: &mut Kit, r: Rect, on: bool, title: &str, sub: &str, h: Hit| {
            let border = if self.hover == Some(h) { theme::mix(th.border, th.text, 0.25) } else { th.border };
            k.bordered(r, 8.0, th.chrome, border);
            let sw = left_switch(r, s);
            k.switch(sw, on);
            let tx = sw.x + sw.w + 14.0 * s;
            k.text(tx, r.y + 7.0 * s, title, Text::label(), r.x + r.w - tx - 10.0 * s);
            k.text(tx, r.y + 26.0 * s, sub, Text::small(), r.x + r.w - tx - 10.0 * s);
        };
        toggle(k, g.show_claude, ctx.show_claude, "Uso de Claude", "Sesión y semana con su reinicio, correo y plan de la cuenta", Hit::ShowClaude);
        toggle(k, g.show_meters, ctx.show_meters, "Consumo de recursos", "CPU, RAM y GPU de Telar y lo que corre en sus tabs", Hit::ShowMeters);
    }

    fn draw_icons(&self, k: &mut Kit, g: &Geometry, current: &str) {
        let s = k.s;
        let th = theme::t();
        let cx = g.content_x;
        k.text(cx, g.title_y, "Ícono de la app", Text::title(), g.content_w - 40.0 * s);
        k.text(cx, g.title_y + 22.0 * s, "Cambia en el Dock al instante y queda guardado en Telar.app", Text::small(), g.content_w);
        let custom_key = std::fs::metadata(appicon::custom_path())
            .and_then(|m| m.modified())
            .ok()
            .map(|t| format!("icono:propio:{:?}", t));
        for (i, r) in g.icon_cards.iter().enumerate() {
            let (id, name) = match appicon::CHOICES.get(i) {
                Some(c) => (c.id, c.name),
                None => (appicon::CUSTOM, "Propio"),
            };
            let selected = id == current;
            let hovered = self.hover == Some(Hit::Icon(i));
            let border = if selected { th.accent } else if hovered { theme::mix(th.border, th.text, 0.25) } else { th.border };
            k.bordered(*r, 10.0, th.chrome, border);
            if selected {
                k.bordered(Rect { x: r.x + s, y: r.y + s, w: r.w - 2.0 * s, h: r.h - 2.0 * s }, 9.0, th.chrome, th.accent);
                k.icon(r.x + r.w - 16.0 * s, r.y + 16.0 * s, icons::CHECK, 14.0, th.accent);
            }
            let size = 104.0 * s;
            let img_r = Rect { x: r.x + (r.w - size) / 2.0, y: r.y + 14.0 * s, w: size, h: size };
            let px = (size.round() as u32).max(32);
            let glyph = if id == appicon::CUSTOM {
                custom_key.as_ref().and_then(|key| k.fonts.image(&format!("{key}:{px}"), || appicon::preview(id, px)))
            } else {
                k.fonts.image(&format!("icono:{id}:{px}"), || appicon::preview(id, px))
            };
            match glyph {
                Some(gl) => k.image(img_r, gl),
                None => {
                    k.bordered(img_r, 22.0, th.surface, th.border);
                    k.icon(img_r.x + img_r.w / 2.0, img_r.y + img_r.h / 2.0, icons::UPLOAD, 26.0, th.text_dim);
                }
            }
            let style = if selected { Text::label() } else { Text::body().color(theme::mix(th.text, th.text_dim, 0.3)) };
            k.text_center(Rect { x: r.x, y: img_r.y + img_r.h + 10.0 * s, w: r.w, h: 18.0 * s }, name, style);
            if id == appicon::CUSTOM && glyph.is_none() {
                k.text_center(Rect { x: r.x, y: img_r.y + img_r.h + 28.0 * s, w: r.w, h: 14.0 * s }, "Sin imagen", Text::small());
            }
        }
        k.button(g.pick, "Elegir archivo…", Some(icons::UPLOAD), Variant::Subtle, self.hover == Some(Hit::PickIcon));
        let note_x = g.pick.x + g.pick.w + 12.0 * s;
        let note = "PNG, JPEG, HEIC o ICNS · mejor cuadrada de 1024 px";
        k.text_in(Rect { x: note_x, w: g.content_x + g.content_w - note_x, ..g.pick }, note_x, note, Text::small());
        if let Some(err) = &self.icon_error {
            k.text(g.content_x, g.pick.y + g.pick.h + 12.0 * s, err, Text::small().color(th.danger), g.content_w);
        }
    }
}

/// Tarjeta de un tema: su fondo con muestras de colores y el nombre.
fn theme_card(k: &mut Kit, r: Rect, t: &Theme, current: bool, hovered: bool) {
    let s = k.s;
    let th = theme::t();
    let border = if current { th.accent } else if hovered { theme::mix(th.border, th.text, 0.25) } else { th.border };
    k.bordered(r, 8.0, th.chrome, border);
    if current {
        k.bordered(Rect { x: r.x + s, y: r.y + s, w: r.w - 2.0 * s, h: r.h - 2.0 * s }, 7.0, th.chrome, th.accent);
    }
    // Muestra: el fondo del tema con su texto y cinco colores.
    let sw = Rect { x: r.x + 8.0 * s, y: r.y + 8.0 * s, w: 46.0 * s, h: r.h - 16.0 * s };
    k.bordered(sw, 6.0, t.bg, theme::mix(t.bg, t.fg, 0.15));
    k.rounded(Rect { x: sw.x + 7.0 * s, y: sw.y + 8.0 * s, w: 24.0 * s, h: 3.0 * s }, 1.5, t.fg);
    for (j, c) in [t.ansi[1], t.ansi[2], t.ansi[3], t.ansi[4], t.ansi[5]].iter().enumerate() {
        k.dot(sw.x + 9.0 * s + j as f32 * 7.0 * s, sw.y + sw.h - 10.0 * s, 5.0, *c);
    }
    let tx = sw.x + sw.w + 10.0 * s;
    let style = if current { Text::label() } else { Text::body().color(theme::mix(th.text, th.text_dim, 0.3)) };
    k.text(tx, r.y + 12.0 * s, t.name, style, r.x + r.w - tx - 22.0 * s);
    k.text(tx, r.y + 30.0 * s, if t.dark { "Oscuro" } else { "Claro" }, Text::small(), r.x + r.w - tx - 8.0 * s);
    if current {
        k.icon(r.x + r.w - 14.0 * s, r.y + 14.0 * s, icons::CHECK, 13.0, th.accent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abre_con_la_fuente_en_uso() {
        let st = Settings::open("Menlo");
        assert_eq!(st.visible()[st.selected].name, "Menlo");
        let st = Settings::open("No existe");
        assert_eq!(st.selected, 0);
    }

    #[test]
    fn filtro_sin_distinguir_mayusculas() {
        let st = Settings { filter: "MENL".into(), ..Default::default() };
        assert!(st.visible().iter().all(|f| f.name.to_lowercase().contains("menl")));
        assert!(!st.visible().is_empty());
    }

    #[test]
    fn seleccion_y_desplazamiento() {
        let mut st = Settings::default();
        let n = st.visible().len();
        st.select(n + 10, 5);
        assert_eq!(st.selected, n - 1);
        assert_eq!(st.scroll, n.saturating_sub(5));
        st.select(0, 5);
        assert_eq!(st.scroll, 0);
        st.scroll_by(-3, 5);
        assert_eq!(st.scroll, 0);
    }

    /// Cada tarjeta de tema responde al clic en su lugar.
    #[test]
    fn clic_en_las_tarjetas_de_tema() {
        let l = Layout::with_cell(1400.0, 900.0, 1.0, 8.0, 16.0, 1);
        let g = geometry(&l);
        let st = Settings { section: APPEARANCE_SECTION, ..Default::default() };
        let fonts = crate::font::Fonts::new("Menlo", 13.0);
        for (i, r) in g.themes.iter().enumerate() {
            assert_eq!(st.hit(r.x + 5.0, r.y + 5.0, &l, &fonts, 0), Hit::Theme(i));
        }
    }
}

/// Ruta con `~` en lugar de la carpeta personal.
fn short_path(p: &Path) -> String {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    match home.as_deref().and_then(|h| p.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    }
}
