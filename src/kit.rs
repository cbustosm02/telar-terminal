//! Kit de componentes de la interfaz (estilo *immediate mode*): cada componente se dibuja
//! con una llamada que recibe su posición y estado, y devuelve lo que ocupó. Los colores
//! salen del tema activo (`theme::t()`), así que todo cambia junto al cambiar de tema.
//!
//! - Texto: `text`, `measure` (SF Pro, con kerning), con estilos `Text::*`.
//! - `icon`: íconos Lucide (`icons::*`), vectoriales.
//! - `kbd`: atajo de teclado como teclas en tags (⌘ ⇧ C).
//! - `tag`, `dot`, `button`, `icon_button`, `progress`, `switch`, `input`, `panel`.
//!
//! Las medidas se dan en puntos; el kit multiplica por la escala de la pantalla.

use crate::font::{Fonts, Glyph};
use crate::renderer::{KIND_COLOR, KIND_MASK, Quad};
use crate::theme::{self, Rgba};
use crate::ui::Rect;

/// Códigos de los íconos de Lucide que usa Telar (fuente `assets/fonts/lucide.ttf`).
/// Para sumar uno: buscar su código en `font/info.json` del paquete `lucide-static`.
#[allow(dead_code)] // catálogo: no todos se usan todavía
pub mod icons {
    pub const ACTIVITY: char = '\u{e038}';
    pub const CPU: char = '\u{e0a9}';
    pub const FOLDER: char = '\u{e0d7}';
    pub const GAUGE: char = '\u{e1bf}';
    pub const IMAGE: char = '\u{e0f6}';
    pub const UPLOAD: char = '\u{e19e}';
    pub const MEMORY: char = '\u{e445}';
    pub const ARROW_DOWN_CIRCLE: char = '\u{e078}';
    pub const BOT: char = '\u{e1bb}';
    pub const CHECK: char = '\u{e06c}';
    pub const CIRCLE_ALERT: char = '\u{e077}';
    pub const CIRCLE_CHECK: char = '\u{e226}';
    pub const COFFEE: char = '\u{e096}';
    pub const DOWNLOAD: char = '\u{e0b2}';
    pub const ERASER: char = '\u{e28f}';
    pub const EYE: char = '\u{e0ba}';
    pub const LOADER: char = '\u{e10a}';
    pub const MINUS: char = '\u{e11c}';
    pub const MOON: char = '\u{e11e}';
    pub const PALETTE: char = '\u{e1dd}';
    pub const PENCIL: char = '\u{e1f9}';
    pub const PLAY: char = '\u{e13c}';
    pub const PLUS: char = '\u{e13d}';
    pub const REFRESH: char = '\u{e145}';
    pub const ROCKET: char = '\u{e286}';
    pub const SEARCH: char = '\u{e151}';
    pub const SETTINGS: char = '\u{e154}';
    pub const SPARKLES: char = '\u{e412}';
    pub const SQUARE_PEN: char = '\u{e172}';
    pub const TERMINAL: char = '\u{e181}';
    pub const TRASH: char = '\u{e18e}';
    pub const TYPE: char = '\u{e198}';
    pub const X: char = '\u{e1b2}';
}

/// Estilo de texto: tamaño en puntos, peso (400 normal, 510 medio, 590 seminegrita) y color.
#[derive(Clone, Copy, Debug)]
pub struct Text {
    pub size: f32,
    pub weight: u16,
    pub color: Rgba,
}

impl Text {
    pub fn body() -> Self {
        Self { size: 12.5, weight: 400, color: theme::t().text }
    }
    pub fn label() -> Self {
        Self { size: 12.5, weight: 510, color: theme::t().text }
    }
    pub fn title() -> Self {
        Self { size: 14.0, weight: 590, color: theme::t().text }
    }
    pub fn small() -> Self {
        Self { size: 11.0, weight: 400, color: theme::t().text_dim }
    }
    /// Encabezados de sección en mayúsculas («GRUPOS»).
    pub fn caption() -> Self {
        Self { size: 10.5, weight: 590, color: theme::t().text_dim }
    }
    pub fn color(mut self, c: Rgba) -> Self {
        self.color = c;
        self
    }
    pub fn weight(mut self, w: u16) -> Self {
        self.weight = w;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// Fondo de acento: la acción principal.
    Primary,
    /// Fondo suave.
    Subtle,
    /// Sin fondo hasta pasar el puntero.
    Ghost,
}

pub struct Kit<'a> {
    pub out: &'a mut Vec<Quad>,
    pub fonts: &'a mut Fonts,
    /// Escala (píxeles por punto).
    pub s: f32,
}

impl<'a> Kit<'a> {
    pub fn new(out: &'a mut Vec<Quad>, fonts: &'a mut Fonts, s: f32) -> Self {
        Self { out, fonts, s }
    }

    pub fn px(&self, pt: f32) -> f32 {
        pt * self.s
    }

    // ── Formas ────────────────────────────────────────────────────────────────

    pub fn rect(&mut self, r: Rect, color: Rgba) {
        self.out.push(Quad::rect(r.x, r.y, r.w, r.h, color));
    }

    pub fn rounded(&mut self, r: Rect, radius: f32, color: Rgba) {
        self.out.push(Quad::rounded(r.x, r.y, r.w, r.h, radius * self.s, color));
    }

    /// Rectángulo redondeado con borde de 1 pt.
    pub fn bordered(&mut self, r: Rect, radius: f32, fill: Rgba, border: Rgba) {
        let s = self.s;
        self.out.push(Quad::rounded(r.x - s, r.y - s, r.w + 2.0 * s, r.h + 2.0 * s, (radius + 1.0) * s, border));
        self.out.push(Quad::rounded(r.x, r.y, r.w, r.h, radius * s, fill));
    }

    pub fn dot(&mut self, cx: f32, cy: f32, d: f32, color: Rgba) {
        let d = d * self.s;
        self.out.push(Quad::rounded(cx - d / 2.0, cy - d / 2.0, d, d, d / 2.0, color));
    }

    /// Panel (menús, modales): borde, sombra suave y fondo de superficie.
    pub fn panel(&mut self, r: Rect, radius: f32) {
        let s = self.s;
        let shadow = theme::alpha([0.0, 0.0, 0.0, 1.0], if theme::t().dark { 0.35 } else { 0.12 });
        self.out.push(Quad::rounded(r.x - 3.0 * s, r.y + s, r.w + 6.0 * s, r.h + 6.0 * s, (radius + 3.0) * s, shadow));
        self.bordered(r, radius, theme::t().surface, theme::t().border);
    }

    // ── Texto e íconos ────────────────────────────────────────────────────────

    /// Altura de una línea del estilo.
    pub fn line_h(&self, style: Text) -> f32 {
        self.fonts.ui_metrics(self.px(style.size)).0
    }

    pub fn measure(&mut self, s: &str, style: Text) -> f32 {
        self.fonts.ui_layout(s, self.px(style.size), style.weight).1
    }

    /// Texto con su caja empezando en (x, y). Si no cabe en `max_w`, se corta con «…».
    /// Devuelve el ancho dibujado.
    pub fn text(&mut self, x: f32, y: f32, s: &str, style: Text, max_w: f32) -> f32 {
        let px = self.px(style.size);
        let (mut glyphs, mut w) = self.fonts.ui_layout(s, px, style.weight);
        if w > max_w {
            let chars: Vec<char> = s.chars().collect();
            let mut n = chars.len();
            while n > 0 && w > max_w {
                n -= 1;
                let cut: String = chars[..n].iter().collect::<String>().trim_end().to_string() + "…";
                (glyphs, w) = self.fonts.ui_layout(&cut, px, style.weight);
            }
        }
        let baseline = (y + self.fonts.ui_metrics(px).1).round();
        for (g, gx, gy) in glyphs {
            self.glyph(g, (x + gx).round(), baseline + gy, style.color);
        }
        w
    }

    /// Texto centrado verticalmente dentro de `r`, desde `x`.
    pub fn text_in(&mut self, r: Rect, x: f32, s: &str, style: Text) -> f32 {
        let y = r.y + (r.h - self.line_h(style)) / 2.0;
        self.text(x, y, s, style, r.x + r.w - x)
    }

    /// Texto centrado (horizontal y vertical) en `r`.
    pub fn text_center(&mut self, r: Rect, s: &str, style: Text) {
        let w = self.measure(s, style).min(r.w);
        let y = r.y + (r.h - self.line_h(style)) / 2.0;
        self.text(r.x + (r.w - w) / 2.0, y, s, style, r.w);
    }

    fn glyph(&mut self, g: Glyph, x: f32, baseline: f32, color: Rgba) {
        let (w, h) = (g.uv[2], g.uv[3]);
        let (kind, color) = if g.color { (KIND_COLOR, [1.0, 1.0, 1.0, color[3]]) } else { (KIND_MASK, color) };
        self.out.push(Quad { rect: [x + g.left, baseline - g.top, w, h], uv: g.uv, color, params: [kind, 0.0, 0.0, 0.0] });
    }

    /// Imagen del atlas (`Fonts::image`) escalada a `r`.
    pub fn image(&mut self, r: Rect, g: Glyph) {
        self.out.push(Quad { rect: [r.x, r.y, r.w, r.h], uv: g.uv, color: [1.0; 4], params: [KIND_COLOR, 0.0, 0.0, 0.0] });
    }

    /// Ícono de `size` puntos centrado en (cx, cy).
    pub fn icon(&mut self, cx: f32, cy: f32, icon: char, size: f32, color: Rgba) {
        let Some(g) = self.fonts.icon(icon, self.px(size)) else { return };
        let (w, h) = (g.uv[2], g.uv[3]);
        let rect = [(cx - w / 2.0).round(), (cy - h / 2.0).round(), w, h];
        self.out.push(Quad { rect, uv: g.uv, color, params: [KIND_MASK, 0.0, 0.0, 0.0] });
    }

    // ── Componentes ───────────────────────────────────────────────────────────

    /// Atajo de teclado: cada tecla en su tag. Termina en `right` (alineado a la derecha)
    /// y centrado en `cy`. Devuelve el x donde empieza.
    pub fn kbd(&mut self, right: f32, cy: f32, keys: &[&str]) -> f32 {
        let s = self.s;
        let style = Text { size: 10.5, weight: 510, color: theme::t().text_dim };
        let h = 18.0 * s;
        let gap = 3.0 * s;
        let widths: Vec<f32> = keys.iter().map(|k| (self.measure(k, style) + 10.0 * s).max(h)).collect();
        let total: f32 = widths.iter().sum::<f32>() + gap * (keys.len().saturating_sub(1)) as f32;
        let mut x = right - total;
        let start = x;
        for (k, w) in keys.iter().zip(widths) {
            let r = Rect { x, y: cy - h / 2.0, w, h };
            let th = theme::t();
            self.bordered(r, 4.0, theme::mix(th.surface, th.text, 0.06), theme::mix(th.surface, th.text, 0.14));
            self.text_center(r, k, style);
            x += w + gap;
        }
        start
    }

    /// Ancho que ocupa `kbd` con esas teclas.
    pub fn kbd_width(&mut self, keys: &[&str]) -> f32 {
        let s = self.s;
        let style = Text { size: 10.5, weight: 510, color: theme::t().text_dim };
        let h = 18.0 * s;
        keys.iter().map(|k| (self.measure(k, style) + 10.0 * s).max(h)).sum::<f32>() + 3.0 * s * (keys.len().saturating_sub(1)) as f32
    }

    /// Etiqueta pequeña con fondo (p. ej. «incluida»). Empieza en `x`, centrada en `cy`.
    pub fn tag(&mut self, x: f32, cy: f32, label: &str, fg: Rgba, bg: Rgba) -> f32 {
        let s = self.s;
        let style = Text { size: 10.5, weight: 510, color: fg };
        let w = self.measure(label, style) + 12.0 * s;
        let r = Rect { x, y: cy - 9.0 * s, w, h: 18.0 * s };
        self.rounded(r, 9.0, bg);
        self.text_center(r, label, style);
        w
    }

    /// Botón con texto y, opcional, ícono a la izquierda.
    pub fn button(&mut self, r: Rect, label: &str, icon: Option<char>, variant: Variant, hovered: bool) {
        let th = theme::t();
        let (bg, fg) = match variant {
            Variant::Primary => (if hovered { theme::mix(th.accent, th.text, 0.12) } else { th.accent }, th.on_accent),
            Variant::Subtle => (if hovered { th.active } else { th.hover }, th.text),
            Variant::Ghost => (if hovered { th.hover } else { theme::alpha(th.hover, 0.0) }, if hovered { th.text } else { th.text_dim }),
        };
        self.rounded(r, 6.0, bg);
        let style = Text::label().color(fg);
        let tw = self.measure(label, style);
        let iw = if icon.is_some() { 14.0 * self.s + if label.is_empty() { 0.0 } else { 6.0 * self.s } } else { 0.0 };
        let x = r.x + (r.w - tw - iw) / 2.0;
        if let Some(ic) = icon {
            self.icon(x + 7.0 * self.s, r.y + r.h / 2.0, ic, 14.0, fg);
        }
        if !label.is_empty() {
            self.text_in(r, x + iw, label, style);
        }
    }

    /// Botón cuadrado con solo un ícono.
    pub fn icon_button(&mut self, r: Rect, icon: char, size: f32, hovered: bool) {
        let th = theme::t();
        if hovered {
            self.rounded(r, 6.0, th.hover);
        }
        self.icon(r.x + r.w / 2.0, r.y + r.h / 2.0, icon, size, if hovered { th.text } else { th.text_dim });
    }

    /// Barra de progreso (0 a 1).
    pub fn progress(&mut self, r: Rect, value: f32) {
        let th = theme::t();
        self.out.push(Quad::rounded(r.x, r.y, r.w, r.h, r.h / 2.0, th.border));
        let w = (r.w * value.clamp(0.0, 1.0)).max(r.h);
        self.out.push(Quad::rounded(r.x, r.y, w, r.h, r.h / 2.0, th.accent));
    }

    /// Interruptor encendido/apagado.
    #[allow(dead_code)] // parte del kit; aún sin uso
    pub fn switch(&mut self, r: Rect, on: bool) {
        let th = theme::t();
        self.out.push(Quad::rounded(r.x, r.y, r.w, r.h, r.h / 2.0, if on { th.accent } else { th.border }));
        let d = r.h - 4.0 * self.s;
        let x = if on { r.x + r.w - d - 2.0 * self.s } else { r.x + 2.0 * self.s };
        self.out.push(Quad::rounded(x, r.y + 2.0 * self.s, d, d, d / 2.0, [1.0, 1.0, 1.0, 1.0]));
    }

    /// Campo de texto (de una línea): valor o texto de ayuda, con cursor si tiene foco.
    pub fn input(&mut self, r: Rect, value: &str, placeholder: &str, icon: Option<char>, focused: bool) {
        let th = theme::t();
        self.bordered(r, 6.0, th.chrome, if focused { theme::mix(th.border, th.accent, 0.6) } else { th.border });
        let mut x = r.x + 10.0 * self.s;
        if let Some(ic) = icon {
            self.icon(x + 7.0 * self.s, r.y + r.h / 2.0, ic, 14.0, th.text_dim);
            x += 22.0 * self.s;
        }
        if value.is_empty() {
            self.text_in(r, x, placeholder, Text::body().color(th.text_dim));
            if focused {
                self.rect(Rect { x, y: r.y + 7.0 * self.s, w: 1.5 * self.s, h: r.h - 14.0 * self.s }, th.accent);
            }
        } else {
            let w = self.text_in(r, x, value, Text::body());
            if focused {
                self.rect(Rect { x: x + w + self.s, y: r.y + 7.0 * self.s, w: 1.5 * self.s, h: r.h - 14.0 * self.s }, th.accent);
            }
        }
    }
}
