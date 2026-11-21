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
    matches!(name.trim_start_matches('-'), "zsh" | "bash" | "fish" | "sh" | "dash" | "ksh" | "tcsh" | "csh" | "nu" | "login")
}

/// `PROC_PGRP_ONLY` de `<libproc.h>` (no está en el crate libc).
const PROC_PGRP_ONLY: u32 = 2;

/// Padre y grupo de procesos de `pid`.
fn bsd_info(pid: i32) -> Option<(i32, i32)> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let read = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut c_void, size) };
    (read == size).then_some((info.pbi_ppid as i32, info.pbi_pgid as i32))
}

/// Padre y línea de comando según `ps`, que puede leer procesos de root (`top`, que
/// es setuid, no deja leer sus datos con `proc_pidinfo` ni `KERN_PROCARGS2`).
fn ps(pid: i32) -> Option<(i32, String)> {
    let out = std::process::Command::new("/bin/ps").args(["-o", "ppid=,args=", "-p", &pid.to_string()]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (ppid, args) = text.trim().split_once(char::is_whitespace)?;
    Some((ppid.trim().parse().ok()?, args.trim().to_string()))
}

/// Proceso padre de `pid`.
fn parent(pid: i32) -> Option<i32> {
    bsd_info(pid).map(|i| i.0).or_else(|| ps(pid).map(|p| p.0))
}

/// Línea de comando de `pid`, lista para volver a ejecutarla.
fn command_of(pid: i32) -> Option<String> {
    argv(pid).map(|a| command_line(&a)).or_else(|| ps(pid).map(|p| p.1)).filter(|c| !c.is_empty())
}

/// Procesos del grupo `pgrp`.
fn pgrp_members(pgrp: i32) -> Vec<i32> {
    let mut buf = vec![0i32; 512];
    let bytes = unsafe {
        libc::proc_listpids(PROC_PGRP_ONLY, pgrp as u32, buf.as_mut_ptr() as *mut c_void, (buf.len() * 4) as i32)
    };
    buf.truncate((bytes.max(0) / 4) as usize);
    buf.retain(|p| *p > 0);
    buf
}

/// Hijos directos de `pid`.
pub fn children(pid: i32) -> Vec<i32> {
    let mut buf = vec![0i32; 256];
    let n = unsafe { libc::proc_listchildpids(pid, buf.as_mut_ptr() as *mut c_void, (buf.len() * 4) as i32) };
    buf.truncate(n.max(0) as usize);
    buf.retain(|p| *p > 0);
    buf
}

/// Si es una shell que lanzó la herramienta Bash de Claude Code: `zsh -c 'source
/// …/shell-snapshots/snapshot-….sh … && eval …'`. Los demás hijos de Claude (servidores
/// MCP, `caffeinate`, hooks) no cuentan.
pub fn is_claude_shell(args: &[String]) -> bool {
    args.len() >= 3 && is_shell(args[0].rsplit('/').next().unwrap_or("")) && args.iter().skip(1).any(|a| a.contains("shell-snapshots/snapshot-"))
}

/// Comandos de shell de Claude que siguen corriendo (en segundo plano si Claude está en
/// espera). `pid` es el proceso `claude`.
pub fn claude_shells(pid: i32) -> usize {
    children(pid).into_iter().filter(|c| argv(*c).is_some_and(|a| is_claude_shell(&a))).count()
}

/// Argumentos con los que se ejecutó `pid` (`KERN_PROCARGS2`: argc, la ruta del
/// ejecutable, relleno de ceros, argv y después el entorno).
pub fn argv(pid: i32) -> Option<Vec<String>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    unsafe {
        if libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) != 0 {
            return None;
        }
    }
    let mut buf = vec![0u8; size];
    unsafe {
        if libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr() as *mut c_void, &mut size, std::ptr::null_mut(), 0) != 0 {
            return None;
        }
    }
    buf.truncate(size);
    parse_procargs(&buf)
}

fn parse_procargs(buf: &[u8]) -> Option<Vec<String>> {
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?) as usize;
    let rest = &buf[4..];
    // Saltar la ruta del ejecutable y los ceros que la siguen.
    let exec_end = rest.iter().position(|b| *b == 0)?;
    let start = exec_end + rest[exec_end..].iter().position(|b| *b != 0)?;
    let args: Vec<String> = rest[start..].split(|b| *b == 0).take(argc).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    (args.len() == argc && argc > 0).then_some(args)
}

/// Un argumento listo para la shell: tal cual si no tiene caracteres especiales; si no,
/// entre comillas simples.
fn quote(arg: &str) -> String {
    let plain = !arg.is_empty() && arg.bytes().all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-~".contains(&b));
    if plain { arg.to_string() } else { format!("'{}'", arg.replace('\'', "'\\''")) }
}

/// Línea de comando de un proceso. Si lo lanzó un intérprete por el shebang de un
/// programa instalado (`node /opt/homebrew/bin/npm run dev`), queda el programa
/// (`/opt/homebrew/bin/npm run dev`).
pub fn command_line(args: &[String]) -> String {
    const INTERPRETERS: &[&str] = &["node", "python", "python3", "ruby", "perl", "bun", "deno", "bash", "sh", "zsh"];
    let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_string();
    let skip = args.len() > 1 && INTERPRETERS.contains(&base(&args[0]).as_str()) && args[1].starts_with('/') && args[1].contains("/bin/");
    let args = if skip { &args[1..] } else { args };
    args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
}

/// Partes de lo que corre en el grupo de procesos `pgrp` (sus procesos cuyo padre no es
/// del grupo: cada comando de un pipeline, sin sus subprocesos) y la shell que las lanzó.
/// El líder del grupo puede ya no existir (en `cat x | less`, `cat` termina enseguida).
fn top_level(pgrp: i32) -> (Option<i32>, Vec<i32>) {
    let mut members = pgrp_members(pgrp);
    if members.is_empty() {
        members.push(pgrp);
    }
    let parents: Vec<(i32, Option<i32>)> = members.iter().map(|p| (*p, parent(*p))).collect();
    let outside = |pp: &Option<i32>| pp.is_none_or(|pp| !members.contains(&pp));
    let mut parts: Vec<i32> = parents.iter().filter(|(_, pp)| outside(pp)).map(|(p, _)| *p).collect();
    parts.sort_unstable();
    let shell = parents.iter().filter_map(|(_, pp)| *pp).find(|pp| !members.contains(pp));
    (shell, parts)
}

/// La shell de una terminal cuyo grupo en primer plano es `pgrp`.
pub fn shell_of(pgrp: i32) -> Option<i32> {
    top_level(pgrp).0.filter(|s| name(*s).is_some_and(|n| is_shell(&n)))
}

/// Un comando de conexión remota (`ssh …`, `mosh …`, `autossh …`): el único que se
/// vuelve a abrir solo al reiniciar Telar.
pub fn is_remote_command(cmd: &str) -> bool {
    let first = cmd.split_whitespace().next().unwrap_or_default();
    let name = first.rsplit('/').next().unwrap_or(first);
    matches!(name, "ssh" | "mosh" | "autossh") && !cmd.contains('|')
}

/// Lo que corre en primer plano en una terminal cuyo grupo de procesos en primer plano
/// es `pgrp`, para volver a ejecutarlo: `None` si es la shell o un pipeline incompleto.
/// Un pipeline entero (`tail -f log | grep x`) se vuelve a armar con sus partes.
pub fn foreground_command(pgrp: i32) -> Option<String> {
    let (_, parts) = top_level(pgrp);
    // El primero de un pipeline ya terminó (`cat x | less`): sin él no se puede volver a
    // armar igual, y `less` solo no tendría qué mostrar. Mejor no ejecutar nada.
    if !parts.contains(&pgrp) && command_of(pgrp).is_none() {
        return None;
    }
    let cmds: Vec<String> = parts.iter().filter_map(|p| command_of(*p)).collect();
    let first = cmds.first()?;
    let first_name = parts.first().and_then(|p| name(*p)).unwrap_or_else(|| {
        let w = first.split_whitespace().next().unwrap_or_default();
        w.rsplit('/').next().unwrap_or(w).to_string()
    });
    if is_shell(&first_name) {
        return None;
    }
    Some(cmds.join(" | "))
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
    fn shells_de_claude() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(is_claude_shell(&args(&["/bin/zsh", "-c", "source /Users/x/.claude/shell-snapshots/snapshot-zsh-1-a.sh 2>/dev/null || true && eval 'npm run dev'"])));
        // Un servidor MCP, un hook o una shell cualquiera no.
        assert!(!is_claude_shell(&args(&["/usr/local/bin/node", "/Users/x/.claude/mcp/proxy.mjs"])));
        assert!(!is_claude_shell(&args(&["/bin/sh", "-c", "telar hook"])));
        assert!(!is_claude_shell(&args(&["-zsh"])));
    }

    #[test]
    fn comandos_remotos() {
        assert!(is_remote_command("ssh homelab"));
        assert!(is_remote_command("/usr/bin/ssh -p 2222 yo@srv"));
        assert!(is_remote_command("mosh srv"));
        assert!(!is_remote_command("npm run dev"));
        assert!(!is_remote_command("ssh srv | tee log"));
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

    #[test]
    fn linea_de_comando() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(command_line(&v(&["htop"])), "htop");
        assert_eq!(command_line(&v(&["vim", "mi archivo.txt"])), "vim 'mi archivo.txt'");
        assert_eq!(command_line(&v(&["echo", "it's"])), "echo 'it'\\''s'");
        assert_eq!(command_line(&v(&["node", "/opt/homebrew/bin/npm", "run", "dev"])), "/opt/homebrew/bin/npm run dev");
        assert_eq!(command_line(&v(&["python3", "manage.py", "runserver"])), "python3 manage.py runserver");
        assert_eq!(command_line(&v(&["ssh", "-p", "22", "user@host"])), "ssh -p 22 user@host");
    }

    #[test]
    fn argumentos_de_procargs2() {
        let mut buf = 2i32.to_ne_bytes().to_vec();
        buf.extend(b"/usr/bin/vim\0\0\0\0vim\0a.txt\0HOME=/x\0");
        assert_eq!(parse_procargs(&buf), Some(vec!["vim".to_string(), "a.txt".to_string()]));
        assert_eq!(parse_procargs(&[1, 0]), None);
    }

    /// Un proceso hijo propio: se leen sus argumentos y, como no es una shell, su comando.
    #[test]
    fn comando_de_un_proceso_real() {
        let mut child = std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pid = child.id() as i32;
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(argv(pid), Some(vec!["/bin/sleep".to_string(), "30".to_string()]));
        let _ = child.kill();
        let _ = child.wait();
    }

    /// El proceso de la prueba sí existe: tiene nombre y carpeta.
    #[test]
    fn proceso_propio() {
        let pid = std::process::id() as i32;
        assert!(name(pid).is_some());
        assert_eq!(cwd(pid), std::env::current_dir().ok());
    }
}
