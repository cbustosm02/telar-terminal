//! Pegar imágenes (y soltar archivos) en un tab con SSH. Por SSH, Claude Code corre en la
//! otra máquina: el «pegado vacío» que le dice que lea la imagen del portapapeles no sirve
//! (lee el portapapeles de allá). Telar sube el archivo al servidor con la misma conexión
//! que ya usa el tab (las opciones del `ssh` en primer plano) y pega su ruta remota, que
//! Claude Code reconoce y adjunta.

use std::io::Write;
use std::process::{Command, Stdio};

/// Carpeta remota donde quedan los archivos subidos.
const REMOTE_DIR: &str = "/tmp/telar-pegado";
/// Más grande que esto no se sube (es para imágenes y archivos chicos).
pub const MAX_BYTES: usize = 50 * 1024 * 1024;

/// Opciones de `ssh` que llevan valor (ver `man ssh`).
const WITH_VALUE: &str = "BbcDEeFIiJLlmOoPpQRSWw";
/// Las que no se repiten al subir: reenvíos de puertos (chocarían con los de la conexión
/// abierta), bitácora, control y escape.
const SKIP: &str = "LRDWwEeOQS";
/// Banderas sin valor que no se repiten: terminal, segundo plano, sin comando, verboso…
const SKIP_FLAGS: &str = "tTNfnGVvqsMxXYyAaKkg";

/// Opciones y destino del `ssh` del tab, sin el comando remoto ni lo que no hace falta
/// para abrir otra sesión al mismo servidor. `None` si no se encuentra el destino.
pub fn target(argv: &[String]) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut it = argv.iter().skip(1).peekable();
    while let Some(arg) = it.next() {
        if arg == "--" {
            return it.next().map(|dest| {
                out.push(dest.clone());
                out
            });
        }
        let Some(flags) = arg.strip_prefix('-').filter(|f| !f.is_empty()) else {
            // El primer argumento suelto es el destino; lo que sigue es el comando remoto.
            out.push(arg.clone());
            return Some(out);
        };
        // Banderas juntas («-tt», «-p22», «-4C»).
        let mut chars = flags.char_indices();
        while let Some((i, c)) = chars.next() {
            if WITH_VALUE.contains(c) {
                let inline = &flags[i + c.len_utf8()..];
                let value = if inline.is_empty() { it.next()?.clone() } else { inline.to_string() };
                if !SKIP.contains(c) {
                    out.push(format!("-{c}"));
                    out.push(value);
                }
                break;
            }
            if !SKIP_FLAGS.contains(c) {
                out.push(format!("-{c}"));
            }
        }
    }
    None
}

/// Nombre único para el archivo remoto, con su extensión.
fn remote_name(ext: &str) -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    format!("telar-{}-{:09}{}", t.as_secs(), t.subsec_nanos(), ext)
}

/// Comillas simples para la shell remota.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Lo que corre la shell del servidor: guarda stdin en `path` (solo para el usuario).
fn remote_command(path: &str) -> String {
    format!("umask 077; mkdir -p {REMOTE_DIR} && cat > {} && echo ok", quote(path))
}

/// Sube `bytes` al servidor de `target` y devuelve la ruta remota.
pub fn upload(target: &[String], bytes: &[u8], ext: &str) -> Result<String, String> {
    if bytes.len() > MAX_BYTES {
        return Err(format!("el archivo pasa de {} MB", MAX_BYTES / 1024 / 1024));
    }
    let path = format!("{REMOTE_DIR}/{}", remote_name(ext));
    let remote = remote_command(&path);
    let (opts, dest) = target.split_at(target.len() - 1);
    let mut child = Command::new("/usr/bin/ssh")
        .args(opts)
        // Sin preguntar nada (no hay dónde): con llave o con una conexión compartida.
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-T"])
        .args(dest)
        .arg(remote)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("no se pudo ejecutar ssh: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(bytes).map_err(|e| format!("no se pudo enviar el archivo: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "ok" {
        return Ok(path);
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(if err.contains("Permission denied") || err.contains("password") {
        "el servidor pide contraseña: con una llave SSH (o ControlMaster) Telar puede subirlo".into()
    } else if err.is_empty() {
        "no se pudo subir al servidor".into()
    } else {
        err.lines().last().unwrap_or_default().to_string()
    })
}

/// La imagen del portapapeles (RGBA), si hay una. Se codifica después, fuera del hilo
/// principal (`png`).
pub fn clipboard_image(cb: &mut arboard::Clipboard) -> Option<arboard::ImageData<'static>> {
    cb.get_image().ok().map(|i| i.to_owned_img())
}

/// RGBA → PNG.
pub fn png(img: &arboard::ImageData) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width as u32, img.height as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().ok()?;
        w.write_image_data(&img.bytes).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn opciones_y_destino() {
        assert_eq!(target(&v(&["ssh", "homelab"])), Some(v(&["homelab"])));
        assert_eq!(target(&v(&["ssh", "-p", "2222", "-i", "~/.ssh/k", "yo@srv", "tmux", "a"])), Some(v(&["-p", "2222", "-i", "~/.ssh/k", "yo@srv"])));
        // Banderas juntas, valor pegado, y lo que no se repite (terminal, reenvíos).
        assert_eq!(target(&v(&["ssh", "-tt", "-p22", "-L", "8080:localhost:80", "-4C", "srv"])), Some(v(&["-p", "22", "-4", "-C", "srv"])));
        assert_eq!(target(&v(&["ssh", "-o", "ControlPath=~/.ssh/cm", "-J", "salto", "srv"])), Some(v(&["-o", "ControlPath=~/.ssh/cm", "-J", "salto", "srv"])));
        assert_eq!(target(&v(&["ssh", "-p"])), None);
        assert_eq!(target(&v(&["ssh", "--", "srv"])), Some(v(&["srv"])));
    }

    /// El comando remoto, corrido con la shell de este Mac: guarda el archivo tal cual.
    #[test]
    fn comando_remoto_guarda_el_archivo() {
        let path = format!("{REMOTE_DIR}/{}", remote_name(".png"));
        let bytes: Vec<u8> = (0..=255u8).cycle().take(70_000).collect();
        let mut child = Command::new("/bin/sh").arg("-c").arg(remote_command(&path)).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(&bytes).unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn comillas_remotas() {
        assert_eq!(quote("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(quote("it's"), r"'it'\''s'");
    }
}
