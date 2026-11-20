//! Bloque de Claude de la barra lateral: cuenta, uso de cada cuenta y su menú.

use crate::*;

impl State {
    /// Lee en un hilo el correo y el plan de la cuenta de Claude Code.
    pub(crate) fn refresh_claude_account(&mut self) {
        if !self.config.show_claude {
            return;
        }
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            // La primera vez se traen las cuentas que ya tenía cswap.
            if cuentas::list().is_empty() {
                let _ = cuentas::import_cswap();
            }
            // La copia de la cuenta activa se mantiene al día (Claude Code rota el token).
            cuentas::sync_current();
            let _ = proxy.send_event(UserEvent::Accounts(cuentas::list(), cuentas::current().map(|c| c.key)));
            let _ = proxy.send_event(UserEvent::ClaudeAccount(claude_usage::account()));
        });
        self.fetch_usage(false);
    }

    /// Consulta en un hilo el uso de todas las cuentas (la API, con el token de cada una).
    pub(crate) fn refresh_usage(&mut self) {
        self.fetch_usage(true);
    }

    /// Consulta el uso de la cuenta activa y, con `others`, el de las demás guardadas.
    pub(crate) fn fetch_usage(&mut self, others: bool) {
        if !self.config.show_claude || self.usage_fetching || self.snapshot.is_some() {
            return;
        }
        self.usage_fetching = true;
        self.usage_fetched = Some(Instant::now());
        if others {
            self.usage_fetched_all = Some(Instant::now());
        }
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let _ = proxy.send_event(UserEvent::UsageAll(cuentas::usage_all(others)));
        });
    }

    /// Texto corto del uso de una cuenta para el menú: «42 % · 18 %» (sesión · semana).
    pub(crate) fn usage_detail(&self, key: &str) -> Option<String> {
        if let Some(u) = self.claude_use.get(key) {
            let now = std::time::SystemTime::now();
            let pct = |l: Option<claude_usage::Limit>| l.map_or("—".to_string(), |l| format!("{:.0} %", l.now(now).0));
            return Some(format!("{} · {}", pct(u.session), pct(u.week)));
        }
        self.usage_errors.get(key).map(|e| if e == claude_usage::DEAD_LOGIN { "inicia sesión".into() } else { "sin datos".into() })
    }

    /// Menú de cuentas (clic en el correo del bloque de Claude).
    pub(crate) fn open_account_menu(&mut self) {
        let mut items = Vec::new();
        for (i, a) in self.accounts.iter().enumerate() {
            let current = self.account_key.as_deref() == Some(a.key.as_str());
            let mut label = a.email.clone();
            if let Some(plan) = &a.plan {
                label.push_str(&format!(" · {plan}"));
            }
            let mut item = MenuItem::owned(if current { icons::CHECK } else { icons::USER }, label, Action::SwitchAccount(i));
            if let Some(d) = self.usage_detail(&a.key) {
                item = item.with_detail(d);
            }
            // ✕ para quitarla (no la activa: está en uso).
            if !current {
                item = item.with_remove(Action::RemoveAccount(i));
            }
            items.push(item);
        }
        if !items.is_empty() {
            items.push(MenuItem::separator());
        }
        let saved = self.account_key.as_ref().is_some_and(|k| self.accounts.iter().any(|a| &a.key == k));
        let save_label = if saved { "Actualizar la cuenta actual" } else { "Agregar la cuenta actual" };
        items.push(MenuItem::new(icons::USER_PLUS, save_label, &[], Action::SaveAccount));
        items.push(MenuItem::new(icons::LOG_IN, "Iniciar sesión con otra cuenta", &[], Action::LoginAccount));
        let l = self.layout();
        self.menu = Some(Menu::with_width(self.cursor.0, self.cursor.1, None, self.ws.active, items, &l, 380.0));
        self.editing = None;
        self.window.set_cursor(CursorIcon::Default);
        self.window.request_redraw();
        // Al abrir el menú se consulta el uso de todas las cuentas (si la última consulta de
        // todas fue hace más de 1 min); llega con el menú abierto y se actualiza ahí.
        if self.usage_fetched_all.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
            self.usage_fetching = false;
            self.refresh_usage();
        }
    }

    /// Corre una operación de cuentas en un hilo (el Llavero y los bloqueos pueden tardar).
    pub(crate) fn account_task(&self, f: impl FnOnce() -> Result<String, String> + Send + 'static) {
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let _ = proxy.send_event(UserEvent::AccountDone(f()));
        });
    }

    /// Terminó una operación de cuentas: avisa y vuelve a leer la cuenta y su uso. Un cambio
    /// es en caliente: las sesiones de Claude abiertas siguen (también las que trabajan) y
    /// toman la credencial nueva del Llavero, sin reiniciar nada.
    pub(crate) fn account_done(&mut self, result: Result<String, String>) {
        match result {
            Ok(msg) => notify_macos("Cuentas de Claude", &msg),
            Err(e) => notify_macos("No se pudo", &e),
        }
        self.claude_account = None;
        self.refresh_claude_account();
        self.window.request_redraw();
    }
}
