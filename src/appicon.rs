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
use objc2::runtime::AnyObject;
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

/// Ruta de un `NSURL` de archivo. La cadena se copia mientras su `NSString` sigue vivo:
/// el puntero de `UTF8String` apunta adentro de ella (antes se soltaba primero y la ruta
/// salía vacía).
unsafe fn url_path(url: &AnyObject) -> Option<PathBuf> {
    let path: Option<Retained<AnyObject>> = unsafe { msg_send![url, path] };
    let path = path?;
    let utf8: *const std::ffi::c_char = unsafe { msg_send![&*path, UTF8String] };
    if utf8.is_null() {
        return None;
    }
    let s = unsafe { std::ffi::CStr::from_ptr(utf8) }.to_string_lossy().into_owned();
    drop(path);
    (!s.is_empty()).then(|| PathBuf::from(s))
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

/// Aplica el ícono `id` en el Dock ya mismo (el de Telar.app se escribe al salir).
/// Debe llamarse desde el hilo principal.
pub fn apply(id: &str) -> Result<(), String> {
    let png = png_bytes(id).ok_or_else(|| format!("no existe el ícono «{id}»"))?;
    let img = nsimage(&png).ok_or("no se pudo leer la imagen")?;
    unsafe {
        let app: Retained<AnyObject> = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![&*app, setApplicationIconImage: &*img];
    }
    // El de Telar.app (Finder, y Dock con Telar cerrado) se escribe al salir, en su
    // `Telar.icns`: tocar el paquete con la app abierta rompería su firma y macOS volvería a
    // pedir permisos. Ver `update::prepare_bundle`.
    Ok(())
}

/// `.icns` con el PNG del ícono `id` (entrada `ic10`, 1024 px; macOS reduce el resto).
pub fn icns_from_png(id: &str) -> Option<Vec<u8>> {
    let png = png_bytes(id)?;
    let entry = 8 + png.len() as u32;
    let total = 8 + entry;
    let mut out = Vec::with_capacity(total as usize);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&total.to_be_bytes());
    out.extend_from_slice(b"ic10");
    out.extend_from_slice(&entry.to_be_bytes());
    out.extend_from_slice(&png);
    Some(out)
}

/// Hace que macOS no mueva la ventana al arrastrar en la barra de título. Los tabs viven
/// ahí: sin esto, arrastrar un tab movía la ventana entera en vez de reordenarlo. La
/// ventana se sigue moviendo desde las zonas vacías de la barra (`Window::drag_window`,
/// que usa `performWindowDragWithEvent:` y funciona aunque la ventana no sea movible).
pub fn set_window_movable(window: &winit::window::Window, movable: bool) {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return };
    unsafe {
        let view = h.ns_view.as_ptr() as *const AnyObject;
        let ns_window: Option<Retained<AnyObject>> = msg_send![&*view, window];
        if let Some(w) = ns_window {
            let _: () = msg_send![&*w, setMovable: movable];
        }
    }
}

/// Selector de archivos de macOS para elegir una imagen. Bloquea hasta que se cierra.
/// Corre `f` en el hilo principal, en una vuelta posterior del run loop, fuera de los
/// manejadores de eventos de winit.
///
/// Los paneles de macOS (`NSOpenPanel runModal`) abren su propio ciclo de eventos. Si se
/// abren dentro de un manejador de winit (un clic), winit recibe eventos de la ventana
/// mientras todavía atiende el clic y aborta la app («tried to handle event while
/// another event is currently being handled»). Desde la cola principal de GCD no pasa:
/// winit ya terminó con el clic y atiende los eventos del panel con normalidad.
pub fn later_on_main(f: impl FnOnce() + 'static) {
    unsafe extern "C" {
        static _dispatch_main_q: u8;
        fn dispatch_async_f(queue: *const u8, context: *mut std::ffi::c_void, work: extern "C" fn(*mut std::ffi::c_void));
    }
    extern "C" fn run(context: *mut std::ffi::c_void) {
        let f = unsafe { Box::from_raw(context as *mut Box<dyn FnOnce()>) };
        f();
    }
    let boxed: Box<Box<dyn FnOnce()>> = Box::new(Box::new(f));
    unsafe { dispatch_async_f(&raw const _dispatch_main_q, Box::into_raw(boxed) as *mut std::ffi::c_void, run) };
}

/// Panel para elegir la imagen del ícono propio. Bloquea hasta que se elige o se cancela:
/// llamarlo con `later_on_main`, nunca dentro de un manejador de eventos.
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
        url_path(&*url?)
    }
}

/// Panel de macOS para elegir una carpeta (se puede crear una nueva). `None` si se cancela.
/// Como `pick_file`, va con `later_on_main`.
pub fn pick_folder(message: &str, start: Option<&std::path::Path>) -> Option<PathBuf> {
    unsafe {
        let panel: Retained<AnyObject> = msg_send![class!(NSOpenPanel), openPanel];
        let _: () = msg_send![&*panel, setCanChooseFiles: false];
        let _: () = msg_send![&*panel, setCanChooseDirectories: true];
        let _: () = msg_send![&*panel, setCanCreateDirectories: true];
        let _: () = msg_send![&*panel, setAllowsMultipleSelection: false];
        let _: () = msg_send![&*panel, setMessage: &*nsstring(message)];
        if let Some(dir) = start.and_then(|d| d.to_str()) {
            let url: Retained<AnyObject> = msg_send![class!(NSURL), fileURLWithPath: &*nsstring(dir)];
            let _: () = msg_send![&*panel, setDirectoryURL: &*url];
        }
        let response: isize = msg_send![&*panel, runModal];
        if response != 1 {
            return None; // cancelado
        }
        let url: Option<Retained<AnyObject>> = msg_send![&*panel, URL];
        url_path(&*url?)
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
