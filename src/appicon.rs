//! Ícono de la app: el de por defecto (ventana con grupos), otras propuestas incluidas o
//! uno propio desde un archivo. Se elige en Configuración › Ícono.
//!
//! Al cambiarlo:
//! - el Dock lo muestra al instante (`NSApplication.applicationIconImage`);
//! - si Telar corre desde su `.app`, queda guardado en ella para el Finder y el Dock
//!   aunque Telar esté cerrado (`NSWorkspace setIcon:forFile:`). Con el de por defecto se
//!   quita el ícono propio y vuelve el `.icns` del paquete.
//!
//! Los PNG salen de `scripts/iconos.py`.

use std::path::{Path, PathBuf};
use std::process::Command;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};

pub struct Choice {
    pub id: &'static str,
    pub name: &'static str,
    pub png: &'static [u8],
}

pub const DEFAULT: &str = "ventana";
pub const CUSTOM: &str = "propio";

pub const CHOICES: &[Choice] = &[
    Choice { id: "ventana", name: "Ventana con grupos", png: include_bytes!("../assets/icons/ventana.png") },
    Choice { id: "tejido", name: "Tejido", png: include_bytes!("../assets/icons/tejido.png") },
    Choice { id: "prompt", name: "Prompt tejido", png: include_bytes!("../assets/icons/prompt.png") },
    Choice { id: "hilos", name: "T de hilos", png: include_bytes!("../assets/icons/hilos.png") },
];

/// `.icns` del paquete (el de por defecto).
pub static ICNS: &[u8] = include_bytes!("../assets/icons/Telar.icns");

/// Copia del ícono propio, convertida a PNG de 1024 px.
pub fn custom_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".config").join("telar").join("icono.png")
}

pub fn has_custom() -> bool {
    custom_path().is_file()
}

/// Bytes PNG del ícono `id` (incluido o el propio).
fn png_bytes(id: &str) -> Option<Vec<u8>> {
    if id == CUSTOM {
        return std::fs::read(custom_path()).ok();
    }
    CHOICES.iter().find(|c| c.id == id).map(|c| c.png.to_vec())
}

/// Vista previa RGBA de `size`×`size` (reduce el PNG con un promedio por bloques).
pub fn preview(id: &str, size: u32) -> Option<(Vec<u8>, u32, u32)> {
    let bytes = png_bytes(id)?;
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16 | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    let channels = info.color_type.samples();
    let mut out = vec![0u8; (size * size * 4) as usize];
    for oy in 0..size {
        for ox in 0..size {
            let (x0, x1) = (ox * w / size, ((ox + 1) * w / size).max(ox * w / size + 1));
            let (y0, y1) = (oy * h / size, ((oy + 1) * h / size).max(oy * h / size + 1));
            let mut acc = [0u32; 4];
            let mut n = 0;
            for y in y0..y1.min(h) {
                for x in x0..x1.min(w) {
                    let p = ((y * w + x) as usize) * channels;
                    let px = match channels {
                        4 => [buf[p], buf[p + 1], buf[p + 2], buf[p + 3]],
                        2 => [buf[p], buf[p], buf[p], buf[p + 1]],
                        _ => [buf[p], buf[p + 1.min(channels - 1)], buf[p + 2.min(channels - 1)], 255],
                    };
                    // Premultiplicado, para que los bordes transparentes no se oscurezcan.
                    let a = px[3] as u32;
                    acc[0] += px[0] as u32 * a;
                    acc[1] += px[1] as u32 * a;
                    acc[2] += px[2] as u32 * a;
                    acc[3] += a;
                    n += 1;
                }
            }
            let o = ((oy * size + ox) * 4) as usize;
            if acc[3] > 0 {
                out[o] = (acc[0] / acc[3]) as u8;
                out[o + 1] = (acc[1] / acc[3]) as u8;
                out[o + 2] = (acc[2] / acc[3]) as u8;
            }
            out[o + 3] = (acc[3] / n.max(1)) as u8;
        }
    }
    Some((out, size, size))
}

/// Copia `src` (PNG, JPEG, HEIC, ICNS…) como el ícono propio, en PNG de 1024 px.
pub fn import(src: &Path) -> Result<(), String> {
    import_to(src, &custom_path())
}

fn import_to(src: &Path, dst: &Path) -> Result<(), String> {
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let st = Command::new("/usr/bin/sips")
        .args(["-s", "format", "png", "-Z", "1024"])
        .arg(src)
        .arg("--out")
        .arg(dst)
        .output()
        .map_err(|e| format!("sips: {e}"))?;
    if !st.status.success() || !dst.is_file() {
        return Err(format!("no se pudo leer la imagen {}", src.display()));
    }
    Ok(())
}

// ── AppKit ────────────────────────────────────────────────────────────────────

fn nsstring(s: &str) -> Retained<AnyObject> {
    let c = std::ffi::CString::new(s).unwrap_or_default();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

/// NSImage a partir de bytes PNG.
fn nsimage(png: &[u8]) -> Option<Retained<AnyObject>> {
    unsafe {
        let data: Retained<AnyObject> = msg_send![class!(NSData), dataWithBytes: png.as_ptr() as *const std::ffi::c_void, length: png.len()];
        let alloc: Allocated<AnyObject> = msg_send![class!(NSImage), alloc];
        let img: Option<Retained<AnyObject>> = msg_send![alloc, initWithData: &*data];
        img
    }
}

/// La `.app` desde la que corre Telar, si corre desde una.
fn bundle_path() -> Option<PathBuf> {
    let exe = std::fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

/// Aplica el ícono `id` en el Dock ya mismo y, con `persist` y si corre desde la `.app`,
/// también en ella (Finder y Dock con Telar cerrado). Debe llamarse desde el hilo principal.
pub fn apply(id: &str, persist: bool) -> Result<(), String> {
    let png = png_bytes(id).ok_or_else(|| format!("no existe el ícono «{id}»"))?;
    let img = nsimage(&png).ok_or("no se pudo leer la imagen")?;
    unsafe {
        let app: Retained<AnyObject> = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![&*app, setApplicationIconImage: &*img];
    }
    if let Some(bundle) = bundle_path().filter(|_| persist) {
        let path = nsstring(&bundle.display().to_string());
        unsafe {
            let ws: Retained<AnyObject> = msg_send![class!(NSWorkspace), sharedWorkspace];
            // Con el de por defecto se quita el ícono propio: vuelve el .icns del paquete.
            let image: Option<&AnyObject> = if id == DEFAULT { None } else { Some(&*img) };
            let ok: Bool = msg_send![&*ws, setIcon: image, forFile: &*path, options: 0usize];
            if !ok.as_bool() {
                return Err("macOS no dejó cambiar el ícono de la app".into());
            }
        }
    }
    Ok(())
}

/// Selector de archivos de macOS para elegir una imagen. Bloquea hasta que se cierra.
pub fn pick_file() -> Option<PathBuf> {
    unsafe {
        let panel: Retained<AnyObject> = msg_send![class!(NSOpenPanel), openPanel];
        let _: () = msg_send![&*panel, setCanChooseFiles: true];
        let _: () = msg_send![&*panel, setCanChooseDirectories: false];
        let _: () = msg_send![&*panel, setAllowsMultipleSelection: false];
        let _: () = msg_send![&*panel, setMessage: &*nsstring("Elige una imagen para el ícono de Telar (PNG, JPEG, HEIC o ICNS; mejor cuadrada y de 1024 px)")];
        let types = ["png", "jpg", "jpeg", "heic", "icns", "tiff"].map(nsstring);
        let refs: Vec<&AnyObject> = types.iter().map(|t| &**t).collect();
        let array: Retained<AnyObject> = msg_send![class!(NSArray), arrayWithObjects: refs.as_ptr(), count: refs.len()];
        let _: () = msg_send![&*panel, setAllowedFileTypes: &*array];
        let response: isize = msg_send![&*panel, runModal];
        if response != 1 {
            return None; // cancelado
        }
        let url: Option<Retained<AnyObject>> = msg_send![&*panel, URL];
        let path: Option<Retained<AnyObject>> = msg_send![&*url?, path];
        let utf8: *const std::ffi::c_char = msg_send![&*path?, UTF8String];
        (!utf8.is_null()).then(|| PathBuf::from(std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opciones_incluidas() {
        assert_eq!(CHOICES[0].id, DEFAULT);
        for c in CHOICES {
            assert_eq!(&c.png[..8], b"\x89PNG\r\n\x1a\n", "{} es PNG", c.id);
            let (rgba, w, h) = preview(c.id, 64).unwrap();
            assert_eq!((w, h, rgba.len()), (64, 64, 64 * 64 * 4));
            // Esquinas transparentes (forma de macOS) y centro opaco.
            assert_eq!(rgba[3], 0, "{}", c.id);
            assert_eq!(rgba[((32 * 64 + 32) * 4 + 3) as usize], 255, "{}", c.id);
        }
        assert_eq!(&ICNS[..4], b"icns");
        assert!(preview("no-existe", 64).is_none());
    }

    #[test]
    fn importar_una_imagen() {
        let dir = std::env::temp_dir().join(format!("telar-icono-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("a.png");
        std::fs::write(&src, CHOICES[1].png).unwrap();
        let dst = dir.join("sub").join("icono.png");
        import_to(&src, &dst).unwrap();
        assert_eq!(&std::fs::read(&dst).unwrap()[..4], b"\x89PNG");
        assert!(import_to(&dir.join("no-existe.png"), &dst.with_file_name("x.png")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
