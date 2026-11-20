//! Navegadores de los agentes en cada tab (ver `browsers`): registro por el socket, el
//! ícono que los trae al frente y la limpieza de los que se cerraron.

use crate::*;

impl State {
    /// Pedido `browser` de la CLI o de una herramienta del agente (p. ej. pirani-cli).
    pub(crate) fn ipc_browser(&mut self, body: &Value) -> Value {
        let tab = match self.resolve_tab(&body["tab"]) {
            Ok(id) => id,
            Err(e) => return json!({ "ok": false, "error": e }),
        };
        let event = match browsers::parse(body) {
            Ok(e) => e,
            Err(e) => return json!({ "ok": false, "error": e }),
        };
        let Some(t) = self.terms.get_mut(&tab) else { return json!({ "ok": false, "error": "tab inexistente" }) };
        if browsers::apply(&mut t.browsers, event) && self.ws.group().is_some_and(|g| g.tabs.contains(&tab)) {
            // El ícono pudo aparecer o irse bajo el puntero quieto.
            if self.drag.is_none() {
                let (x, y) = self.cursor;
                self.on_cursor_moved(winit::dpi::PhysicalPosition::new(x as f64, y as f64));
            }
            self.window.request_redraw();
        }
        json!({ "ok": true, "id": tab })
    }

    /// Clic en el ícono: corre su comando para mostrarlo (si lo trae) y activa su app.
    pub(crate) fn show_browser(&mut self, tab: u64) {
        let Some(b) = self.terms.get(&tab).and_then(|t| browsers::main(&t.browsers)).cloned() else { return };
        if b.show.is_empty() {
            appicon::activate_pid(b.pid);
            return;
        }
        // En un hilo: reabrir uno que corría sin ventana tarda unos segundos.
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let ran = std::process::Command::new(&b.show[0])
                .args(&b.show[1..])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            if let Err(e) = ran {
                log::warn!("no se pudo mostrar el navegador «{}»: {e}", b.name);
            }
            let _ = proxy.send_event(UserEvent::BrowserShown(tab, b.name));
        });
    }

    /// Terminó su comando `show`: se activa con el pid actual (pudo reabrirse con otro).
    pub(crate) fn browser_shown(&mut self, tab: u64, name: &str) {
        if let Some(b) = self.terms.get(&tab).and_then(|t| t.browsers.iter().find(|b| b.name == name)) {
            appicon::activate_pid(b.pid);
        }
    }

    /// Quita los navegadores cuyo proceso ya no existe. Devuelve si cambió algo.
    pub(crate) fn prune_browsers(&mut self) -> bool {
        let mut changed = false;
        for t in self.terms.values_mut() {
            let before = t.browsers.len();
            t.browsers.retain(|b| browsers::alive(b.pid));
            changed |= t.browsers.len() != before;
        }
        changed
    }

    /// Algún navegador del grupo a la vista está latiendo (el agente lo acaba de usar).
    pub(crate) fn browser_pulsing(&self) -> bool {
        self.ws.group().is_some_and(|g| g.tabs.iter().filter_map(|id| self.terms.get(id)).any(|t| t.browsers.iter().any(browsers::Browser::active)))
    }
}
