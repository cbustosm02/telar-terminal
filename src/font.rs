//! Fuentes: carga (mmap), rasterización con swash y atlas de glifos.
//!
//! Cada glifo se rasteriza una sola vez y queda en un atlas RGBA en la GPU.
//! Los glifos monocromos se guardan como (255, 255, 255, alfa) y se tiñen en el
//! shader; los de color (emoji) se guardan tal cual.

use std::collections::HashMap;

use swash::scale::image::Content;
use swash::scale::image::Image;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::shape::ShapeContext;
use swash::zeno::Format;
use swash::FontRef;

use crate::fontlist;

pub const ATLAS_SIZE: u32 = 2048;

/// Estilo de un glifo: índice en `primary`.
pub const REGULAR: u8 = 0;
pub const BOLD: u8 = 1;
pub const ITALIC: u8 = 2;
pub const BOLD_ITALIC: u8 = 3;

/// Respaldo para lo que no tenga la familia elegida. La fuente de emoji va última
/// (`emoji_presentation` la busca ahí); los símbolos Nerd Font van justo antes.
const FALLBACKS: &[&str] = &[
    "/System/Library/Fonts/Menlo.ttc",
    "/System/Library/Fonts/SFNSMono.ttf",
    "/System/Library/Fonts/Apple Symbols.ttf",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/System/Library/Fonts/Apple Color Emoji.ttc",
];

/// Texto de interfaz formado: glifos (x desde el inicio, y desde la línea base) y ancho.
pub type UiLayout = (Vec<(Glyph, f32, f32)>, f32);

#[derive(Clone, Copy, Debug)]
pub struct Glyph {
    /// Región en el atlas, en píxeles: x, y, ancho, alto.
    pub uv: [f32; 4],
    /// Desplazamiento desde el origen de la celda (izquierda) y la línea base (arriba).
    pub left: f32,
    pub top: f32,
    pub color: bool,
}

pub struct Upload {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

/// Empaquetador por filas (shelf) con 1 px de margen entre glifos.
struct Atlas {
    x: u32,
    y: u32,
    row_h: u32,
    uploads: Vec<Upload>,
}

impl Atlas {
    fn new() -> Self {
        Self { x: 1, y: 1, row_h: 0, uploads: Vec::new() }
    }

    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w + 2 > ATLAS_SIZE || h + 2 > ATLAS_SIZE {
            return None;
        }
        if self.x + w + 1 > ATLAS_SIZE {
            self.x = 1;
            self.y += self.row_h + 1;
            self.row_h = 0;
        }
        if self.y + h + 1 > ATLAS_SIZE {
            return None;
        }
        let pos = (self.x, self.y);
        self.x += w + 1;
        self.row_h = self.row_h.max(h);
        Some(pos)
    }
}

pub struct Fonts {
    /// Familia en uso (la configurada, o la por defecto si no se encontró).
    pub family: String,
    primary: [FontRef<'static>; 4],
    fallbacks: Vec<FontRef<'static>>,
    ctx: ScaleContext,
    pub size: f32,
    pub cell_w: f32,
    pub cell_h: f32,
    /// Distancia desde el borde superior de la celda hasta la línea base.
    pub baseline: f32,
    pub underline: f32,
    pub stroke: f32,
    cache: HashMap<(char, u8), Option<Glyph>>,
    atlas: Atlas,
    /// Fuente de la interfaz (SF Pro, variable) e íconos (Lucide).
    ui: FontRef<'static>,
    icons: FontRef<'static>,
    shaper: ShapeContext,
    /// Glifos de la interfaz a cualquier tamaño: (fuente, glifo, tamaño ×4, peso).
    sized: HashMap<(u8, u16, u32, u16), Option<Glyph>>,
    /// Imágenes subidas al atlas (vistas previas de íconos), por clave.
    images: HashMap<String, Option<Glyph>>,
    /// Textos de interfaz ya formados (texto, tamaño, peso): la barra lateral y los tabs
    /// se dibujan muchas veces por segundo con los mismos textos.
    layouts: HashMap<(String, u32, u16), UiLayout>,
    /// El atlas se vació en este frame: hay que volver a dibujar.
    pub atlas_reset: bool,
}

impl Fonts {
    /// Fuentes de la familia `family` (si no existe, la por defecto) a `size` píxeles.
    pub fn new(family: &str, size: f32) -> Self {
        let (family, primary) = match fontlist::load(family) {
            Some(faces) => (family.to_string(), faces),
            None => {
                log::warn!("no se encontró la fuente «{family}»; se usa {}", fontlist::DEFAULT_FAMILY);
                (fontlist::DEFAULT_FAMILY.to_string(), fontlist::load(fontlist::DEFAULT_FAMILY).expect("fuente incluida"))
            }
        };
        // Las mapeadas se comparten entre instancias: `fontlist::map` no repite archivos.
        let mut fallbacks: Vec<FontRef<'static>> = FALLBACKS
            .iter()
            .filter_map(|p| fontlist::map(std::path::Path::new(p)))
            .filter_map(|d| FontRef::from_index(d, 0))
            .collect();
        let emoji = fallbacks.pop();
        fallbacks.extend(FontRef::from_index(fontlist::NERD_SYMBOLS, 0));
        fallbacks.extend(emoji);
        let ui = fontlist::map(std::path::Path::new(UI_FONT))
            .and_then(|d| FontRef::from_index(d, 0))
            .unwrap_or(primary[0]);
        let icons = FontRef::from_index(ICONS, 0).expect("íconos incluidos");
        let mut fonts = Self {
            ui,
            icons,
            shaper: ShapeContext::new(),
            sized: HashMap::new(),
            images: HashMap::new(),
            layouts: HashMap::new(),
            family,
            primary,
            fallbacks,
            ctx: ScaleContext::new(),
            size: 0.0,
            cell_w: 0.0,
            cell_h: 0.0,
            baseline: 0.0,
            underline: 0.0,
            stroke: 1.0,
            cache: HashMap::new(),
            atlas: Atlas::new(),
            atlas_reset: false,
        };
        fonts.set_size(size);
        fonts
    }

    pub fn set_size(&mut self, size: f32) {
        self.size = size;
        let font = self.primary[0];
        let m = font.metrics(&[]).scale(size);
        let advance = font.glyph_metrics(&[]).scale(size).advance_width(font.charmap().map('M'));
        self.cell_w = advance.round().max(1.0);
        self.cell_h = (m.ascent + m.descent + m.leading).ceil().max(1.0);
        self.baseline = m.ascent.round();
        self.underline = (self.baseline - m.underline_offset).round();
        self.stroke = m.stroke_size.round().max(1.0);
        self.clear_atlas();
    }

    fn clear_atlas(&mut self) {
        self.cache.clear();
        self.sized.clear();
        self.images.clear();
        self.layouts.clear();
        self.atlas = Atlas::new();
        self.atlas_reset = true;
    }

    pub fn take_uploads(&mut self) -> Vec<Upload> {
        std::mem::take(&mut self.atlas.uploads)
    }

    pub fn glyph(&mut self, c: char, style: u8) -> Option<Glyph> {
        if let Some(g) = self.cache.get(&(c, style)) {
            return *g;
        }
        let g = self.rasterize(c, style);
        self.cache.insert((c, style), g);
        g
    }

    fn rasterize(&mut self, c: char, style: u8) -> Option<Glyph> {
        // Caracteres que se muestran como emoji por defecto (☕, ⚡, ✅…): primero la fuente
        // de emoji; si no, ganaría el glifo de texto de Menlo o Apple Symbols.
        let emoji = emoji_presentation(c).then(|| self.fallbacks.last().copied()).flatten();
        let candidates = emoji
            .into_iter()
            .chain(std::iter::once(self.primary[style as usize]))
            .chain(std::iter::once(self.primary[0]))
            .chain(self.fallbacks.iter().copied());

        for font in candidates {
            let id = font.charmap().map(c);
            if id == 0 {
                continue;
            }
            let mut scaler = self.ctx.builder(font).size(self.size).hint(true).build();
            let image = Render::new(&[
                Source::ColorOutline(0),
                Source::ColorBitmap(StrikeWith::BestFit),
                Source::Outline,
            ])
            .format(Format::Alpha)
            .render(&mut scaler, id)?;

            return self.upload(image);
        }
        None
    }

    /// Sube la imagen de un glifo al atlas.
    fn upload(&mut self, image: Image) -> Option<Glyph> {
        let (w, h) = (image.placement.width, image.placement.height);
        if w == 0 || h == 0 {
            return None; // glifo vacío (espacio)
        }
        let color = image.content == Content::Color;
        let data = match image.content {
            Content::Mask => image.data.iter().flat_map(|a| [255, 255, 255, *a]).collect(),
            Content::Color => image.data,
            Content::SubpixelMask => image.data.chunks(4).flat_map(|p| [255, 255, 255, p[1]]).collect(),
        };
        let (x, y) = match self.atlas.alloc(w, h) {
            Some(pos) => pos,
            None => {
                // Atlas lleno: se vacía y se empieza de nuevo (muy raro).
                self.clear_atlas();
                self.atlas.alloc(w, h)?
            }
        };
        self.atlas.uploads.push(Upload { x, y, w, h, data });
        Some(Glyph {
            uv: [x as f32, y as f32, w as f32, h as f32],
            left: image.placement.left as f32,
            top: image.placement.top as f32,
            color,
        })
    }

    /// Fuente para la clave de caché de `sized`: 0 interfaz, 1 íconos, 2 terminal, 3.. respaldos.
    fn font_by_key(&self, key: u8) -> FontRef<'static> {
        match key {
            0 => self.ui,
            1 => self.icons,
            2 => self.primary[0],
            k => self.fallbacks[(k - 3) as usize],
        }
    }

    /// Glifo `id` de la fuente `key` a `px` píxeles (con peso, si la fuente es variable).
    fn sized_glyph(&mut self, key: u8, id: u16, px: f32, weight: u16) -> Option<Glyph> {
        let k = (key, id, (px * 4.0).round() as u32, weight);
        if let Some(g) = self.sized.get(&k) {
            return *g;
        }
        let font = self.font_by_key(key);
        let mut scaler = self.ctx.builder(font).size(px).hint(false).variations(&ui_axes(weight)).build();
        let image = Render::new(&[Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit), Source::Outline])
            .format(Format::Alpha)
            .render(&mut scaler, id);
        let g = image.and_then(|img| self.upload(img));
        self.sized.insert(k, g);
        g
    }

    /// Altura de línea y distancia a la línea base de la fuente de interfaz a `px` píxeles.
    pub fn ui_metrics(&self, px: f32) -> (f32, f32) {
        let m = self.ui.metrics(&[]).scale(px);
        ((m.ascent + m.descent + m.leading).ceil(), m.ascent.round())
    }

    /// Texto de interfaz: glifos posicionados (x desde el inicio, y desde la línea base)
    /// y ancho total. SF Pro con kerning; lo que SF no tenga sale de los respaldos.
    pub fn ui_layout(&mut self, text: &str, px: f32, weight: u16) -> UiLayout {
        let key = (text.to_string(), px.to_bits(), weight);
        if let Some(l) = self.layouts.get(&key) {
            return l.clone();
        }
        let l = self.shape_ui(text, px, weight);
        // Textos que ya no se muestran (títulos que cambiaron) no se acumulan sin fin.
        if self.layouts.len() > 4096 {
            self.layouts.clear();
        }
        self.layouts.insert(key, l.clone());
        l
    }

    fn shape_ui(&mut self, text: &str, px: f32, weight: u16) -> UiLayout {
        let mut out = Vec::new();
        let mut pen = 0.0f32;
        let ui_cmap = self.ui.charmap();
        let mut rest = text;
        while let Some(c) = rest.chars().next() {
            if ui_cmap.map(c) != 0 {
                // Tramo que SF sabe dibujar: se da forma de una vez (kerning y ligaduras).
                let end = rest.char_indices().find(|(_, ch)| ui_cmap.map(*ch) == 0).map_or(rest.len(), |(i, _)| i);
                let run = &rest[..end];
                rest = &rest[end..];
                let mut glyphs = Vec::new();
                let mut shaper = self.shaper.builder(self.ui).size(px).variations(&ui_axes(weight)).build();
                shaper.add_str(run);
                shaper.shape_with(|cluster| {
                    for g in cluster.glyphs {
                        glyphs.push((g.id, g.x, g.y, g.advance));
                    }
                });
                for (id, gx, gy, adv) in glyphs {
                    if let Some(g) = self.sized_glyph(0, id, px, weight) {
                        out.push((g, pen + gx, -gy));
                    }
                    pen += adv;
                }
            } else {
                // Un carácter de otra fuente (✳, CJK, emoji…): terminal o respaldos.
                rest = &rest[c.len_utf8()..];
                let keys = std::iter::once(2u8).chain((0..self.fallbacks.len() as u8).map(|i| i + 3));
                let found = keys
                    .map(|k| (k, self.font_by_key(k)))
                    .find(|(_, f)| f.charmap().map(c) != 0);
                if let Some((key, font)) = found {
                    let id = font.charmap().map(c);
                    let adv = font.glyph_metrics(&[]).scale(px).advance_width(id);
                    if let Some(g) = self.sized_glyph(key, id, px, 400) {
                        out.push((g, pen, 0.0));
                    }
                    pen += adv;
                }
            }
        }
        (out, pen)
    }

    /// Imagen RGBA en el atlas, identificada por `key`. `load` solo se llama la primera vez
    /// (o si el atlas se vació): decodificar no se repite en cada frame.
    pub fn image(&mut self, key: &str, load: impl FnOnce() -> Option<(Vec<u8>, u32, u32)>) -> Option<Glyph> {
        if let Some(g) = self.images.get(key) {
            return *g;
        }
        let g = load().and_then(|(rgba, w, h)| {
            let (x, y) = match self.atlas.alloc(w, h) {
                Some(pos) => pos,
                None => {
                    self.clear_atlas();
                    self.atlas.alloc(w, h)?
                }
            };
            self.atlas.uploads.push(Upload { x, y, w, h, data: rgba });
            Some(Glyph { uv: [x as f32, y as f32, w as f32, h as f32], left: 0.0, top: 0.0, color: true })
        });
        self.images.insert(key.to_string(), g);
        g
    }

    /// Ícono de Lucide (`icons::*`) a `px` píxeles.
    pub fn icon(&mut self, icon: char, px: f32) -> Option<Glyph> {
        let id = self.icons.charmap().map(icon);
        if id == 0 {
            return None;
        }
        self.sized_glyph(1, id, px, 400)
    }
}

/// Ejes de SF Pro para la interfaz: el peso y el tamaño óptico «Text» (17). Por
/// defecto `opsz` vale 28 («Display», para títulos grandes) y el texto chico se ve apretado.
/// En fuentes sin esos ejes no tienen efecto.
fn ui_axes(weight: u16) -> [(&'static str, f32); 2] {
    [("wght", weight as f32), ("opsz", 17.0)]
}

/// Fuente de la interfaz: SF Pro (variable, con ejes de peso y tamaño óptico).
const UI_FONT: &str = "/System/Library/Fonts/SFNS.ttf";

/// Íconos Lucide (ISC), vectoriales: se tiñen como el texto.
static ICONS: &[u8] = include_bytes!("../assets/fonts/lucide.ttf");

/// Caracteres del plano básico con presentación de emoji por defecto (Unicode `Emoji_Presentation`).
/// Los de U+1F000 en adelante solo existen en la fuente de emoji, así que no hace falta listarlos.
fn emoji_presentation(c: char) -> bool {
    matches!(c as u32,
        0x231A..=0x231B | 0x23E9..=0x23EC | 0x23F0 | 0x23F3 | 0x25FD..=0x25FE | 0x2614..=0x2615
        | 0x2648..=0x2653 | 0x267F | 0x2693 | 0x26A1 | 0x26AA..=0x26AB | 0x26BD..=0x26BE
        | 0x26C4..=0x26C5 | 0x26CE | 0x26D4 | 0x26EA | 0x26F2..=0x26F3 | 0x26F5 | 0x26FA | 0x26FD
        | 0x2705 | 0x270A..=0x270B | 0x2728 | 0x274C | 0x274E | 0x2753..=0x2755 | 0x2757
        | 0x2795..=0x2797 | 0x27B0 | 0x27BF | 0x2B1B..=0x2B1C | 0x2B50 | 0x2B55)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentacion_de_emoji() {
        for c in ['☕', '⚡', '✅', '❌', '⭐', '⌚'] {
            assert!(emoji_presentation(c), "{c}");
        }
        for c in ['✓', '★', '☾', 'a', '⌘', '·'] {
            assert!(!emoji_presentation(c), "{c}");
        }
    }

    /// Con las fuentes del sistema: celdas con tamaño, texto en máscara y emoji a color.
    #[test]
    fn glifos() {
        let mut f = Fonts::new(fontlist::DEFAULT_FAMILY, 26.0);
        assert!(f.cell_w > 10.0 && f.cell_h > f.cell_w);
        let a = f.glyph('a', REGULAR).unwrap();
        assert!(!a.color);
        assert!(f.glyph(' ', REGULAR).is_none(), "el espacio no tiene glifo");
        assert!(f.glyph('☕', REGULAR).unwrap().color, "☕ sale de la fuente de emoji");
        assert!(f.glyph('日', REGULAR).is_some(), "CJK desde una fuente de respaldo");
    }
}
