//! Familias de fuentes monoespaciadas: la incluida en Telar (JetBrains Mono, la misma
//! que trae Ghostty/cmux) y las instaladas en el sistema.
//!
//! Para listar las del sistema se mapea cada archivo y se leen solo sus tablas de nombre
//! y métricas; no se carga el archivo entero (Apple Color Emoji pesa ~180 MB).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use swash::{Attributes, FontDataRef, FontRef, Style, StringId};

/// Familia por defecto, incluida en el binario.
pub const DEFAULT_FAMILY: &str = "JetBrains Mono";

static EMBEDDED: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Italic.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-BoldItalic.ttf"),
];

/// Íconos de prompts y herramientas (Nerd Fonts), como respaldo de cualquier familia.
pub static NERD_SYMBOLS: &[u8] = include_bytes!("../assets/fonts/SymbolsNerdFontMono-Regular.ttf");

const DIRS: &[&str] = &["/System/Library/Fonts", "/System/Library/Fonts/Supplemental", "/Library/Fonts"];

#[derive(Clone, Debug, PartialEq)]
struct Face {
    path: PathBuf,
    index: usize,
    weight: u16,
    italic: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Family {
    pub name: String,
    /// Incluida en Telar (no depende de lo instalado).
    pub embedded: bool,
    faces: Vec<Face>,
}

/// Mapea un archivo y lo deja vivo toda la app (una sola vez por ruta). Las páginas
/// solo se leen cuando hacen falta.
pub fn map(path: &Path) -> Option<&'static [u8]> {
    static MAPPED: OnceLock<Mutex<HashMap<PathBuf, &'static [u8]>>> = OnceLock::new();
    let mut mapped = MAPPED.get_or_init(Default::default).lock().ok()?;
    if let Some(data) = mapped.get(path) {
        return Some(data);
    }
    let file = std::fs::File::open(path).ok()?;
    let mmap = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    let mmap: &'static memmap2::Mmap = Box::leak(Box::new(mmap));
    mapped.insert(path.to_path_buf(), &mmap[..]);
    Some(&mmap[..])
}

/// Nombre de familia: el tipográfico (agrupa pesos como «Medium») o, si no hay, el básico.
fn family_name(font: &FontRef) -> Option<String> {
    let strings = font.localized_strings();
    let name = strings
        .find_by_id(StringId::TypographicFamily, Some("en"))
        .or_else(|| strings.find_by_id(StringId::Family, Some("en")))?
        .to_string();
    let name = name.trim().to_string();
    (!name.is_empty() && !name.starts_with('.')).then_some(name)
}

/// Monoespaciada de verdad: el mismo ancho para letras angostas y anchas. No basta la
/// marca de la fuente (`isFixedPitch`): algunas la declaran mal (p. ej. Hoefler Text).
fn is_monospace(font: &FontRef) -> bool {
    let cmap = font.charmap();
    let g = font.glyph_metrics(&[]);
    let mut width = None;
    for c in ['i', 'M', 'W', 'l', '.', 'a', '0'] {
        let id = cmap.map(c);
        if id == 0 {
            return false; // fuentes de símbolos o sin latín
        }
        let w = g.advance_width(id);
        match width {
            None if w > 0.0 => width = Some(w),
            Some(first) if (w - first).abs() < 0.01 => {}
            _ => return false,
        }
    }
    true
}

fn style(attrs: Attributes) -> (u16, bool) {
    (attrs.weight().0, !matches!(attrs.style(), Style::Normal))
}

fn scan_file(path: &Path, out: &mut BTreeMap<String, Vec<Face>>) {
    let Ok(file) = std::fs::File::open(path) else { return };
    // Mapeo temporal: solo para leer las tablas; se suelta al terminar.
    let Ok(mmap) = (unsafe { memmap2::Mmap::map(&file) }) else { return };
    let Some(data) = FontDataRef::new(&mmap) else { return };
    for (index, font) in data.fonts().enumerate() {
        if !is_monospace(&font) {
            continue;
        }
        let Some(name) = family_name(&font) else { continue };
        let (weight, italic) = style(font.attributes());
        out.entry(name).or_default().push(Face { path: path.to_path_buf(), index, weight, italic });
    }
}

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = DIRS.iter().map(PathBuf::from).collect();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("Library/Fonts"));
    }
    dirs
}

/// Todas las familias monoespaciadas: primero la incluida, luego las del sistema por nombre.
/// La primera llamada recorre las carpetas de fuentes; las siguientes usan lo ya leído.
pub fn families() -> &'static [Family] {
    static CACHE: OnceLock<Vec<Family>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut found = BTreeMap::new();
        for dir in font_dirs() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                let ext = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
                if matches!(ext.as_deref(), Some("ttf" | "otf" | "ttc")) {
                    scan_file(&p, &mut found);
                }
            }
        }
        let mut list = vec![Family { name: DEFAULT_FAMILY.into(), embedded: true, faces: Vec::new() }];
        list.extend(
            found
                .into_iter()
                .filter(|(name, _)| !name.eq_ignore_ascii_case(DEFAULT_FAMILY))
                .map(|(name, faces)| Family { name, embedded: false, faces }),
        );
        list
    })
}

/// La cara más cercana al peso pedido, con o sin itálica.
fn pick(faces: &[Face], weight: u16, italic: bool) -> Option<&Face> {
    faces.iter().filter(|f| f.italic == italic).min_by_key(|f| (f.weight as i32 - weight as i32).abs())
}

/// Las cuatro caras (normal, negrita, itálica, negrita itálica) de una familia.
/// Si faltan negrita o itálica se usa la más parecida. `None` si no existe la familia.
pub fn load(name: &str) -> Option<[FontRef<'static>; 4]> {
    if name.eq_ignore_ascii_case(DEFAULT_FAMILY) {
        return Some(EMBEDDED.map(|d| FontRef::from_index(d, 0).expect("fuente incluida")));
    }
    let family = families().iter().find(|f| f.name.eq_ignore_ascii_case(name))?;
    let regular = pick(&family.faces, 400, false).or_else(|| family.faces.first())?;
    // Una «negrita» que no es más gruesa que la normal no sirve: se usa la normal.
    let bold = pick(&family.faces, 700, false).filter(|f| f.weight > regular.weight).unwrap_or(regular);
    let italic = pick(&family.faces, 400, true).unwrap_or(regular);
    let bold_italic = pick(&family.faces, 700, true)
        .filter(|f| f.weight > italic.weight)
        .unwrap_or(if italic == regular { bold } else { italic });
    let face = |f: &Face| FontRef::from_index(map(&f.path)?, f.index);
    Some([face(regular)?, face(bold)?, face(italic)?, face(bold_italic)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_incluida_carga_sus_cuatro_estilos() {
        let [r, b, i, bi] = load("jetbrains mono").unwrap();
        assert!(!style(r.attributes()).1);
        assert!(style(b.attributes()).0 > style(r.attributes()).0);
        assert!(style(i.attributes()).1 && style(bi.attributes()).1);
    }

    #[test]
    fn lista_del_sistema() {
        let list = families();
        assert_eq!(list[0].name, DEFAULT_FAMILY);
        assert!(list[0].embedded);
        // macOS siempre trae Menlo, y es monoespaciada.
        assert!(list.iter().any(|f| f.name == "Menlo"), "{:?}", list.iter().map(|f| &f.name).collect::<Vec<_>>());
        // No se cuelan proporcionales ni de símbolos.
        for not in ["Helvetica", "Apple Color Emoji", "Apple Symbols", "Times"] {
            assert!(!list.iter().any(|f| f.name == not), "{not}");
        }
        let menlo = load("Menlo").unwrap();
        assert!(style(menlo[1].attributes()).0 > style(menlo[0].attributes()).0, "Menlo tiene negrita");
        assert!(load("No Existe Mono").is_none());
    }

    #[test]
    fn elegir_caras() {
        let f = |weight, italic| Face { path: PathBuf::new(), index: 0, weight, italic };
        let faces = [f(300, false), f(400, false), f(700, false), f(400, true)];
        assert_eq!(pick(&faces, 400, false).unwrap().weight, 400);
        assert_eq!(pick(&faces, 700, false).unwrap().weight, 700);
        assert_eq!(pick(&faces, 700, true).unwrap().weight, 400);
        assert!(pick(&faces[..3], 400, true).is_none());
    }
}
