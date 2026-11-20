//! Firma local estable para `Telar.app`.
//!
//! macOS guarda los permisos de privacidad (Documentos, Escritorio, Acceso total al
//! disco…) atados al «requisito designado» de la firma. Con una firma ad hoc ese requisito
//! es la huella exacta del binario, así que cada actualización los invalida y macOS vuelve
//! a preguntar. Firmando siempre con el mismo certificado, el requisito pasa a ser
//! «identificador + certificado» y los permisos sobreviven a las actualizaciones.
//!
//! El certificado es autofirmado y se crea en este Mac la primera vez: vive en un llavero
//! propio (`~/.config/telar/firma/`) que no se agrega a la lista de llaveros del usuario.
//! Si algo falla se recurre a la firma ad hoc, como antes.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const NAME: &str = "Telar (firma local)";
const SECURITY: &str = "/usr/bin/security";
const CODESIGN: &str = "/usr/bin/codesign";
// LibreSSL del sistema: su PKCS#12 lo entiende `security import` (el de OpenSSL 3+ no).
const OPENSSL: &str = "/usr/bin/openssl";

fn dir() -> PathBuf {
    if let Some(p) = std::env::var_os("TELAR_SIGNING_DIR") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".config").join("telar").join("firma")
}

struct Identity {
    keychain: PathBuf,
    password: String,
    hash: String,
}

fn quiet(cmd: &mut Command) -> Option<std::process::Output> {
    cmd.stdin(Stdio::null()).output().ok()
}

fn ok(cmd: &mut Command) -> bool {
    quiet(cmd).is_some_and(|o| o.status.success())
}

/// El SHA-1 de la identidad de firma del llavero (desbloqueado), si existe.
fn find(keychain: &Path) -> Option<String> {
    let out = quiet(Command::new(SECURITY).args(["find-identity", "-p", "codesigning"]).arg(keychain))?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .filter(|l| l.contains(NAME))
        .flat_map(|l| l.split_whitespace())
        .find(|w| w.len() == 40 && w.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_string)
}

fn unlock(keychain: &Path, password: &str) -> bool {
    ok(Command::new(SECURITY).args(["unlock-keychain", "-p", password]).arg(keychain))
}

/// La identidad de firma, creándola la primera vez.
fn identity() -> Option<Identity> {
    let dir = dir();
    let keychain = dir.join("telar.keychain-db");
    let pass_file = dir.join("clave");
    if let Ok(password) = std::fs::read_to_string(&pass_file) {
        let password = password.trim().to_string();
        if keychain.is_file()
            && unlock(&keychain, &password)
            && let Some(hash) = find(&keychain)
        {
            return Some(Identity { keychain, password, hash });
        }
    }
    match create(&dir, &keychain, &pass_file) {
        Ok(id) => Some(id),
        Err(e) => {
            log::warn!("no se pudo crear la firma local: {e}");
            None
        }
    }
}

fn random_hex() -> Result<String, String> {
    use std::io::Read;
    let mut buf = [0u8; 24];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)).map_err(|e| e.to_string())?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

fn create(dir: &Path, keychain: &Path, pass_file: &Path) -> Result<Identity, String> {
    use std::os::unix::fs::PermissionsExt;
    // Empieza de cero: un llavero a medio crear o con otra clave no sirve.
    let _ = std::fs::remove_file(keychain);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    let password = random_hex()?;
    std::fs::write(pass_file, &password).map_err(|e| e.to_string())?;
    let _ = std::fs::set_permissions(pass_file, std::fs::Permissions::from_mode(0o600));

    let cnf = dir.join("cert.cnf");
    let key = dir.join("key.pem");
    let cert = dir.join("cert.pem");
    let p12 = dir.join("id.p12");
    std::fs::write(
        &cnf,
        format!(
            "[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n[dn]\nCN={NAME}\n\
             [ext]\nbasicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature\n\
             extendedKeyUsage=critical,codeSigning\n"
        ),
    )
    .map_err(|e| e.to_string())?;
    let step = |what: &str, cmd: &mut Command| if ok(cmd) { Ok(()) } else { Err(format!("falló {what}")) };
    step(
        "openssl req",
        Command::new(OPENSSL)
            .args(["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "36500", "-keyout"])
            .arg(&key)
            .arg("-out")
            .arg(&cert)
            .arg("-config")
            .arg(&cnf),
    )?;
    let p12_pass = random_hex()?;
    step(
        "openssl pkcs12",
        Command::new(OPENSSL)
            .args(["pkcs12", "-export", "-name", NAME, "-passout"])
            .arg(format!("pass:{p12_pass}"))
            .arg("-inkey")
            .arg(&key)
            .arg("-in")
            .arg(&cert)
            .arg("-out")
            .arg(&p12),
    )?;
    let result = (|| {
        step("security create-keychain", Command::new(SECURITY).args(["create-keychain", "-p", &password]).arg(keychain))?;
        // Sin bloqueo por tiempo ni al dormir; igual se desbloquea antes de cada firma.
        step("security set-keychain-settings", Command::new(SECURITY).arg("set-keychain-settings").arg(keychain))?;
        if !unlock(keychain, &password) {
            return Err("no se pudo desbloquear el llavero".into());
        }
        // Solo codesign puede usar la clave, y sin preguntar.
        step(
            "security import",
            Command::new(SECURITY).arg("import").arg(&p12).arg("-k").arg(keychain).args(["-P", &p12_pass, "-T", CODESIGN]),
        )?;
        find(keychain).ok_or_else(|| "el llavero no tiene la identidad".to_string())
    })();
    // La clave privada queda solo dentro del llavero.
    for f in [&cnf, &key, &cert, &p12] {
        let _ = std::fs::remove_file(f);
    }
    let hash = result?;
    log::info!("firma local creada: {hash}");
    Ok(Identity { keychain: keychain.to_path_buf(), password, hash })
}

/// Firma `path` con la identidad local; si no se puede, ad hoc. Devuelve si usó la local.
pub fn sign(path: &Path) -> bool {
    if let Some(id) = identity() {
        // El llavero se vuelve a bloquear al reiniciar el Mac.
        unlock(&id.keychain, &id.password);
        if ok(Command::new(CODESIGN).args(["--force", "--sign", &id.hash, "--keychain"]).arg(&id.keychain).arg(path)) {
            return true;
        }
        log::warn!("codesign con la firma local falló; se firma ad hoc");
    }
    let _ = Command::new(CODESIGN).args(["--force", "--sign", "-"]).arg(path).stderr(Stdio::null()).status();
    false
}

/// Si `path` ya está firmado con un certificado (no ad hoc), los permisos de macOS no
/// dependen de la huella del binario.
pub fn is_stable(path: &Path) -> bool {
    quiet(Command::new(CODESIGN).args(["-d", "-r-"]).arg(path))
        .is_some_and(|o| String::from_utf8_lossy(&o.stdout).contains("certificate leaf"))
}

/// El registro de permisos del usuario solo se puede leer con «Acceso total al disco»
/// (sin él, `open` falla en silencio, sin diálogo).
pub fn has_full_disk_access() -> bool {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    std::fs::File::open(home.join("Library/Application Support/com.apple.TCC/TCC.db")).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Crea una identidad en una carpeta temporal y firma una copia de un binario: el
    /// requisito designado debe ser estable (identificador + certificado) y seguir igual
    /// al volver a firmar un binario distinto.
    #[test]
    #[ignore = "crea un llavero y un certificado; correr a mano en macOS"]
    fn firma_estable() {
        let tmp = std::env::temp_dir().join(format!("telar-firma-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        // SAFETY: prueba aislada, sin otros hilos leyendo esta variable.
        unsafe { std::env::set_var("TELAR_SIGNING_DIR", tmp.join("firma")) };
        let a = tmp.join("a");
        let b = tmp.join("b");
        std::fs::copy("/bin/echo", &a).unwrap();
        std::fs::copy("/bin/ls", &b).unwrap();
        let leaf = |p: &Path| {
            let o = Command::new(CODESIGN).args(["-d", "-r-"]).arg(p).output().unwrap();
            let s = String::from_utf8_lossy(&o.stdout).to_string();
            s.split("certificate leaf = ").nth(1).map(|r| r.split_whitespace().next().unwrap_or("").to_string())
        };
        sign(&a);
        sign(&b);
        assert!(is_stable(&a) && is_stable(&b));
        // Misma identidad para binarios distintos: los permisos no dependen de la huella.
        assert!(leaf(&a).is_some());
        assert_eq!(leaf(&a), leaf(&b));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
