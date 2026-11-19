//! Mantener el Mac despierto (`caffeinate -dims`) desde Telar, sin correrlo en un tab.
//!
//! Modos:
//! - `Off`: nada.
//! - `On`: siempre, mientras Telar esté abierto.
//! - `Auto`: solo mientras algún Claude está trabajando.
//!
//! `caffeinate` corre con `-w <pid de Telar>`: si Telar se cierra o se cae, termina solo.

use std::process::{Child, Command, Stdio};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Off,
    On,
    Auto,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::On => "on",
            Mode::Auto => "auto",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" | "no" => Some(Mode::Off),
            "on" | "si" | "sí" => Some(Mode::On),
            "auto" => Some(Mode::Auto),
            _ => None,
        }
    }

    /// Orden del atajo ⌘⇧C y del clic: apagado → siempre → con Claude → apagado.
    pub fn next(self) -> Self {
        match self {
            Mode::Off => Mode::On,
            Mode::On => Mode::Auto,
            Mode::Auto => Mode::Off,
        }
    }

    /// Texto de la barra lateral («Claude»: mientras algún Claude trabaja).
    pub fn label(self) -> &'static str {
        match self {
            Mode::Off => "Despierto: no",
            Mode::On => "Despierto: sí",
            Mode::Auto => "Despierto: Claude",
        }
    }
}

#[derive(Default)]
pub struct Awake {
    pub mode: Mode,
    child: Option<Child>,
}

impl Awake {
    /// ¿Está corriendo `caffeinate` ahora?
    pub fn active(&self) -> bool {
        self.child.is_some()
    }

    /// Arranca o detiene `caffeinate` según el modo y si algún Claude está trabajando.
    pub fn update(&mut self, working: bool) {
        let want = match self.mode {
            Mode::Off => false,
            Mode::On => true,
            Mode::Auto => working,
        };
        // Si murió por fuera (p. ej. alguien lo mató), se vuelve a lanzar.
        if self.child.as_mut().is_some_and(|c| !matches!(c.try_wait(), Ok(None))) {
            self.child = None;
        }
        match (want, self.child.is_some()) {
            (true, false) => {
                let spawned = Command::new("/usr/bin/caffeinate")
                    .args(["-dims", "-w", &std::process::id().to_string()])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
                match spawned {
                    Ok(c) => self.child = Some(c),
                    Err(e) => log::error!("no se pudo lanzar caffeinate: {e}"),
                }
            }
            (false, true) => self.stop(),
            _ => {}
        }
    }

    fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for Awake {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ciclo_de_modos() {
        assert_eq!(Mode::Off.next(), Mode::On);
        assert_eq!(Mode::On.next(), Mode::Auto);
        assert_eq!(Mode::Auto.next(), Mode::Off);
    }

    #[test]
    fn nombres_ida_y_vuelta() {
        for m in [Mode::Off, Mode::On, Mode::Auto] {
            assert_eq!(Mode::parse(m.name()), Some(m));
        }
        assert_eq!(Mode::parse("sí"), Some(Mode::On));
        assert_eq!(Mode::parse("turbo"), None);
    }

    /// Lanza y detiene caffeinate de verdad según el modo.
    #[test]
    fn arranca_y_detiene_caffeinate() {
        let mut a = Awake::default();
        a.update(true);
        assert!(!a.active(), "apagado no corre aunque Claude trabaje");
        a.mode = Mode::Auto;
        a.update(false);
        assert!(!a.active());
        a.update(true);
        assert!(a.active());
        a.update(false);
        assert!(!a.active());
        a.mode = Mode::On;
        a.update(false);
        assert!(a.active());
        drop(a); // al soltarlo, termina
    }
}
