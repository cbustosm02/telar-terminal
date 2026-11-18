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
    // El binario nativo de Claude Code se llama como su versión (p. ej. «2.1.327»).
    let is_version = name.split('.').count() >= 3 && name.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    Some(if is_version { "claude".to_string() } else { name })
}

pub fn is_shell(name: &str) -> bool {
    matches!(name.trim_start_matches('-'), "zsh" | "bash" | "fish" | "sh" | "login")
}
