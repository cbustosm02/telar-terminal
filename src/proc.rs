//! Información del proceso en primer plano de un tab (macOS): pid, nombre y carpeta actual.

use std::ffi::{OsStr, c_void};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

/// Grupo de procesos en primer plano del PTY (la shell, o lo que esté corriendo en ella).
pub fn foreground(pty: &OwnedFd) -> Option<i32> {
    let pgrp = unsafe { libc::tcgetpgrp(pty.as_raw_fd()) };
    (pgrp > 0).then_some(pgrp)
}

pub fn cwd(pid: i32) -> Option<PathBuf> {
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as i32;
    let read = unsafe {
        libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, &mut info as *mut _ as *mut c_void, size)
    };
    if read != size {
        return None;
    }
    let path = &info.pvi_cdir.vip_path;
    let bytes = unsafe { std::slice::from_raw_parts(path.as_ptr() as *const u8, std::mem::size_of_val(path)) };
    let end = bytes.iter().position(|b| *b == 0)?;
    (end > 0).then(|| PathBuf::from(OsStr::from_bytes(&bytes[..end])))
}

pub fn name(pid: i32) -> Option<String> {
    let mut buf = [0u8; 256];
    let len = unsafe { libc::proc_name(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    let name = (len > 0).then(|| String::from_utf8_lossy(&buf[..len as usize]).into_owned())?;
    Some(display_name(name))
}

/// El binario nativo de Claude Code se llama como su versión (p. ej. «2.1.327»): se muestra «claude».
pub fn display_name(name: String) -> String {
    let is_version = name.split('.').count() >= 3 && name.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    if is_version { "claude".to_string() } else { name }
}

pub fn is_shell(name: &str) -> bool {
    matches!(name.trim_start_matches('-'), "zsh" | "bash" | "fish" | "sh" | "login")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_de_claude_se_muestra_como_claude() {
        assert_eq!(display_name("2.1.327".into()), "claude");
        assert_eq!(display_name("10.0.1.4".into()), "claude");
    }

    #[test]
    fn otros_nombres_quedan_igual() {
        for n in ["zsh", "node", "1.2", "v2.1.3", "2..1", "python3.13"] {
            assert_eq!(display_name(n.into()), n);
        }
    }

    #[test]
    fn shells() {
        for n in ["zsh", "-zsh", "bash", "fish", "sh", "login"] {
            assert!(is_shell(n), "{n}");
        }
        for n in ["claude", "node", "vim", "zshx"] {
            assert!(!is_shell(n), "{n}");
        }
    }

    /// El proceso de la prueba sí existe: tiene nombre y carpeta.
    #[test]
    fn proceso_propio() {
        let pid = std::process::id() as i32;
        assert!(name(pid).is_some());
        assert_eq!(cwd(pid), std::env::current_dir().ok());
    }
}
