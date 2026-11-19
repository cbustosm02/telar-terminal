//! Modal de configuración (⌘,). Una barra de secciones a la izquierda y el contenido de
//! la sección a la derecha. Por ahora hay una sección, «Apariencia» (fuente y tamaño);
//! para sumar otra se agrega a `SECTIONS` y se dibuja en `draw`.
//!
//! Los cambios se aplican al instante (la terminal de atrás sirve de vista previa)
//! y se guardan solos en `config.json`.

use crate::font::Fonts;
use crate::fontlist::{self, Family};
use crate::renderer::Quad;
use crate::theme;
use crate::ui::{self, Layout, Rect};

pub const SECTIONS: &[&str] = &["Apariencia"];

const PANEL_W: f32 = 700.0;
const PANEL_H: f32 = 480.0;
const NAV_W: f32 = 170.0;
const ROW_H: f32 = 26.0;

/// Qué hay bajo el puntero dentro del modal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Outside,
    Inside,
    Close,
    Section(usize),
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

/// Rectángulos del modal para una ventana dada.
struct Geometry {
    panel: Rect,
    close: Rect,
    nav: Vec<Rect>,
    search: Rect,
    list: Rect,
    rows: usize,
    size_down: Rect,
    size_up: Rect,
    size_y: f32,
}

fn geometry(l: &Layout, fonts: &Fonts) -> Geometry {
    let s = l.s();
    let (w, h) = ((PANEL_W * s).min(l.w() - 40.0 * s), (PANEL_H * s).min(l.h() - 40.0 * s));
    let panel = Rect { x: (l.w() - w) / 2.0, y: (l.h() - h) / 2.0, w, h };
    let close = Rect { x: panel.x + w - 34.0 * s, y: panel.y + 10.0 * s, w: 24.0 * s, h: 24.0 * s };
    let nav = (0..SECTIONS.len())
        .map(|i| Rect { x: panel.x + 10.0 * s, y: panel.y + 52.0 * s + i as f32 * 32.0 * s, w: NAV_W * s - 20.0 * s, h: 28.0 * s })
        .collect();
    let cx = panel.x + NAV_W * s + 20.0 * s;
    let cw = panel.x + w - 24.0 * s - cx;
    let ch = fonts.cell_h;
    let search = Rect { x: cx, y: panel.y + 80.0 * s, w: cw, h: ch + 12.0 * s };
    let size_y = panel.y + h - 52.0 * s;
    let list_y = search.y + search.h + 8.0 * s;
    let list_h = size_y - 16.0 * s - list_y;
    let rows = ((list_h / (ROW_H * s)).floor() as usize).max(1);
    let list = Rect { x: cx, y: list_y, w: cw, h: rows as f32 * ROW_H * s };
    let btn = 26.0 * s;
    let size_down = Rect { x: cx + 90.0 * s, y: size_y, w: btn, h: btn };
    let size_up = Rect { x: size_down.x + btn + 60.0 * s, y: size_y, w: btn, h: btn };
    Geometry { panel, close, nav, search, list, rows, size_down, size_up, size_y }
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
        st.scroll = st.selected.saturating_sub(3);
        st
    }

    pub fn hit(&self, x: f32, y: f32, l: &Layout, fonts: &Fonts) -> Hit {
        let g = geometry(l, fonts);
        if !g.panel.contains(x, y) {
            return Hit::Outside;
        }
        if g.close.contains(x, y) {
            return Hit::Close;
        }
        if let Some(i) = g.nav.iter().position(|r| r.contains(x, y)) {
            return Hit::Section(i);
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
    pub fn page(&self, l: &Layout, fonts: &Fonts) -> usize {
        geometry(l, fonts).rows
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
        let g = geometry(l, fonts);
        let ch = fonts.cell_h;
        let cw = fonts.cell_w;
        let p = g.panel;

        // Fondo atenuado y panel.
        out.push(Quad::rect(0.0, 0.0, l.w(), l.h(), [0.0, 0.0, 0.0, 0.45]));
        out.push(Quad::rounded(p.x - s, p.y - s, p.w + 2.0 * s, p.h + 2.0 * s, 13.0 * s, theme::BORDER));
        out.push(Quad::rounded(p.x, p.y, p.w, p.h, 12.0 * s, theme::CHROME_BG));
        out.push(Quad::rect(p.x + NAV_W * s, p.y + 12.0 * s, s, p.h - 24.0 * s, theme::BORDER));
        ui::text(out, fonts, p.x + 20.0 * s, p.y + 18.0 * s, "Configuración", theme::TEXT_DIM, NAV_W * s - 30.0 * s);

        // Cerrar.
        if self.hover == Some(Hit::Close) {
            out.push(Quad::rounded(g.close.x, g.close.y, g.close.w, g.close.h, 5.0 * s, theme::ITEM_HOVER));
        }
        ui::text(out, fonts, g.close.x + (g.close.w - cw) / 2.0, g.close.y + (g.close.h - ch) / 2.0, "×", theme::TEXT_DIM, cw + 1.0);

        // Secciones.
        for (i, (r, name)) in g.nav.iter().zip(SECTIONS).enumerate() {
            if i == self.section {
                out.push(Quad::rounded(r.x, r.y, r.w, r.h, 6.0 * s, theme::ITEM_ACTIVE));
                out.push(Quad::rounded(r.x, r.y + 6.0 * s, 3.0 * s, r.h - 12.0 * s, 1.5 * s, theme::ACCENT));
            } else if self.hover == Some(Hit::Section(i)) {
                out.push(Quad::rounded(r.x, r.y, r.w, r.h, 6.0 * s, theme::ITEM_HOVER));
            }
            let color = if i == self.section { theme::TEXT } else { theme::TEXT_DIM };
            ui::text(out, fonts, r.x + 12.0 * s, r.y + (r.h - ch) / 2.0, name, color, r.w - 20.0 * s);
        }

        // Apariencia.
        let cx = g.search.x;
        ui::text(out, fonts, cx, p.y + 18.0 * s, SECTIONS[self.section], theme::TEXT, g.search.w);
        ui::text(out, fonts, cx, p.y + 54.0 * s, "Fuente de la terminal y la interfaz · se guarda sola", theme::TEXT_DIM, g.search.w);

        // Búsqueda.
        let r = g.search;
        out.push(Quad::rounded(r.x, r.y, r.w, r.h, 6.0 * s, theme::BORDER));
        out.push(Quad::rounded(r.x + s, r.y + s, r.w - 2.0 * s, r.h - 2.0 * s, 5.0 * s, theme::ITEM_HOVER));
        let ty = r.y + (r.h - ch) / 2.0;
        if self.filter.is_empty() {
            ui::text(out, fonts, r.x + 10.0 * s, ty, "Escribe para buscar…", theme::TEXT_DIM, r.w - 20.0 * s);
        } else {
            let w = ui::text(out, fonts, r.x + 10.0 * s, ty, &self.filter, theme::TEXT, r.w - 20.0 * s);
            out.push(Quad::rect(r.x + 10.0 * s + w + s, ty + 2.0 * s, 2.0 * s, ch - 4.0 * s, theme::ACCENT));
        }

        // Lista.
        let list = self.visible();
        if list.is_empty() {
            ui::text(out, fonts, cx + 10.0 * s, g.list.y + 6.0 * s, "Ninguna fuente coincide", theme::TEXT_DIM, g.list.w);
        }
        for (row, fam) in list.iter().enumerate().skip(self.scroll).take(g.rows) {
            let y = g.list.y + (row - self.scroll) as f32 * ROW_H * s;
            let rr = Rect { x: g.list.x, y, w: g.list.w - 10.0 * s, h: ROW_H * s - 2.0 * s };
            let current = fam.name.eq_ignore_ascii_case(family);
            if row == self.selected {
                out.push(Quad::rounded(rr.x, rr.y, rr.w, rr.h, 5.0 * s, theme::ITEM_ACTIVE));
            } else if self.hover == Some(Hit::Font(row)) {
                out.push(Quad::rounded(rr.x, rr.y, rr.w, rr.h, 5.0 * s, theme::ITEM_HOVER));
            }
            let ty = rr.y + (rr.h - ch) / 2.0;
            ui::text(out, fonts, rr.x + 8.0 * s, ty, if current { "✓" } else { " " }, theme::ACCENT, cw + 1.0);
            let tag = if fam.embedded { "incluida · como cmux" } else { "" };
            let tag_w = tag.chars().count() as f32 * cw;
            let color = if current { theme::TEXT } else { theme::TEXT_DIM };
            ui::text(out, fonts, rr.x + 8.0 * s + 2.0 * cw, ty, &fam.name, color, rr.w - tag_w - 3.0 * cw - 24.0 * s);
            ui::text(out, fonts, rr.x + rr.w - 10.0 * s - tag_w, ty, tag, theme::TEXT_DIM, tag_w + 1.0);
        }
        // Barra de desplazamiento.
        if list.len() > g.rows {
            let track = g.list;
            let h = (track.h * g.rows as f32 / list.len() as f32).max(20.0 * s);
            let y = track.y + (track.h - h) * self.scroll as f32 / (list.len() - g.rows) as f32;
            out.push(Quad::rounded(track.x + track.w - 5.0 * s, y, 4.0 * s, h, 2.0 * s, theme::BORDER));
        }

        // Tamaño.
        let ty = g.size_y + (g.size_down.h - ch) / 2.0;
        ui::text(out, fonts, cx, ty, "Tamaño", theme::TEXT_DIM, 80.0 * s);
        for (r, label, hit) in [(g.size_down, "−", Hit::SizeDown), (g.size_up, "+", Hit::SizeUp)] {
            let bg = if self.hover == Some(hit) { theme::ITEM_ACTIVE } else { theme::ITEM_HOVER };
            out.push(Quad::rounded(r.x, r.y, r.w, r.h, 6.0 * s, bg));
            ui::text(out, fonts, r.x + (r.w - cw) / 2.0, r.y + (r.h - ch) / 2.0, label, theme::TEXT, cw + 1.0);
        }
        let value = format!("{size:.0} pt");
        let vw = value.chars().count() as f32 * cw;
        let mid = (g.size_down.x + g.size_down.w + g.size_up.x) / 2.0;
        ui::text(out, fonts, mid - vw / 2.0, ty, &value, theme::TEXT, vw + 1.0);
        // Ayuda, alineada a la derecha y recortada si no cabe.
        let hint = "↑↓ elige · Esc cierra";
        let from = g.size_up.x + g.size_up.w + 20.0 * s;
        let avail = p.x + p.w - 24.0 * s - from;
        let hw = (hint.chars().count() as f32 * cw).min(avail);
        ui::text(out, fonts, p.x + p.w - 24.0 * s - hw, ty, hint, theme::TEXT_DIM, avail);
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
}
