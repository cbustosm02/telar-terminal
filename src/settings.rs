//! Modal de configuración (⌘,). Barra de secciones a la izquierda y el contenido de la
//! sección a la derecha, dibujado con el kit (`kit.rs`). Por ahora hay una sección,
//! «Apariencia»: tema, fuente y tamaño. Para sumar otra: agregarla a `SECTIONS`, su
//! geometría y su dibujo.
//!
//! Los cambios se aplican al instante (la terminal de atrás sirve de vista previa)
//! y se guardan solos en `config.json`.

use crate::font::Fonts;
use crate::fontlist::{self, Family};
use crate::kit::{Kit, Text, Variant, icons};
use crate::renderer::Quad;
use crate::theme::{self, Theme};
use crate::ui::{Layout, Rect};

pub const SECTIONS: &[(char, &str)] = &[(icons::PALETTE, "Apariencia")];

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
    Geometry { panel, close, nav, content_x, content_w, title_y, themes_y, themes, font_label_y, search, list, rows, size_y, size_down, size_up }
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

    pub fn hit(&self, x: f32, y: f32, l: &Layout, _fonts: &Fonts) -> Hit {
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

    /// Dibuja el modal sobre todo lo demás. `family` y `size`: lo que está en uso.
    pub fn draw(&self, out: &mut Vec<Quad>, fonts: &mut Fonts, l: &Layout, family: &str, size: f32) {
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
                let tw = k.measure("incluida · como cmux", Text::small().weight(510)) + 12.0 * s;
                right -= tw;
                k.tag(right, r.y + r.h / 2.0, "incluida · como cmux", th.accent, theme::alpha(th.accent, 0.14));
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
        let st = Settings::default();
        let fonts = crate::font::Fonts::new("Menlo", 13.0);
        for (i, r) in g.themes.iter().enumerate() {
            assert_eq!(st.hit(r.x + 5.0, r.y + 5.0, &l, &fonts), Hit::Theme(i));
        }
    }
}
