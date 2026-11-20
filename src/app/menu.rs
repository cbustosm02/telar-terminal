//! Menú contextual de tabs y grupos.

use crate::*;

impl State {
    /// Abre el menú contextual de un tab (`tab`) o de un grupo, en la posición del puntero.
    pub(crate) fn open_menu(&mut self, tab: Option<u64>, group: usize) {
        let suspended = |id: &u64| self.terms.get(id).is_some_and(Terminal::is_suspended);
        let group_asleep = self.suspendable(group).is_empty();
        let mut items = Vec::new();
        if let Some(id) = tab {
            if self.ws.active_term() != Some(id) {
                items.push(MenuItem::new(icons::EYE, "Mostrar", &[], Action::Show));
            }
            if suspended(&id) {
                items.push(MenuItem::new(icons::PLAY, "Reanudar", &["↩"], Action::Resume));
            } else {
                items.push(MenuItem::new(icons::MOON, "Suspender", &["⌘", "S"], Action::Suspend));
                items.push(MenuItem::new(icons::ERASER, "Limpiar", &["⌘", "K"], Action::Clear));
            }
            items.push(MenuItem::new(icons::PENCIL, "Renombrar tab", &[], Action::RenameTab));
            items.push(MenuItem::new(icons::X, "Cerrar tab", &["⌘", "W"], Action::CloseTab));
            items.push(MenuItem::separator());
        } else {
            items.push(MenuItem::new(icons::TERMINAL, "Nuevo tab", &["⌘", "T"], Action::NewTab));
            items.push(MenuItem::new(icons::SPARKLES, "Nuevo agente", &["⌘", "⇧", "T"], Action::NewAgent));
            items.push(MenuItem::separator());
        }
        items.push(MenuItem::new(icons::SQUARE_PEN, "Renombrar grupo", &["⌘", "⇧", "R"], Action::RenameGroup));
        if group_asleep {
            items.push(MenuItem::new(icons::PLAY, "Reanudar grupo", &["⌘", "⇧", "S"], Action::ResumeGroup));
        } else {
            items.push(MenuItem::new(icons::MOON, "Suspender grupo", &["⌘", "⇧", "S"], Action::SuspendGroup));
        }
        if self.config.idle.enabled() {
            let exempt = self.ws.groups.get(group).is_some_and(|g| g.idle_exempt);
            let (icon, label) = if exempt { (icons::TIMER, "Permitir suspender solo") } else { (icons::PIN, "Nunca suspender solo") };
            items.push(MenuItem::new(icon, label, &[], Action::ToggleIdleExempt));
        }
        items.push(MenuItem::separator());
        items.push(MenuItem::new(icons::TRASH, "Eliminar grupo", &[], Action::DeleteGroup));
        let l = self.layout();
        self.menu = Some(Menu::new(self.cursor.0, self.cursor.1, tab, group, items, &l));
        self.editing = None;
        self.window.set_cursor(CursorIcon::Default);
        self.window.request_redraw();
    }

    pub(crate) fn run_menu(&mut self, action: Action) {
        let Some(menu) = self.menu.take() else { return };
        let (tab, gi) = (menu.tab, menu.group);
        match (action, tab) {
            (Action::Show, Some(id)) => self.show_tab(id),
            (Action::Suspend, Some(id)) => {
                self.suspend(id);
            }
            (Action::Resume, Some(id)) => {
                self.resume(id);
            }
            (Action::Clear, Some(id)) => self.clear_terminal(id),
            (Action::RenameTab, Some(id)) => self.start_rename_tab(id),
            (Action::CloseTab, Some(id)) => {
                if self.close_term(id) {
                    // Era el último tab: Telar empieza de nuevo con un grupo vacío en lugar de cerrarse.
                    self.new_group();
                }
                self.resize_terms();
            }
            (Action::NewTab | Action::NewAgent, _) => {
                self.ws.select_group(gi);
                if action == Action::NewAgent {
                    self.new_agent();
                } else {
                    self.new_tab();
                }
                self.after_switch();
            }
            (Action::RenameGroup, _) => self.start_rename(gi),
            (Action::SuspendGroup | Action::ResumeGroup, _) => self.toggle_suspend_group(gi),
            (Action::DeleteGroup, _) => self.ask_delete_group(gi),
            (Action::SwitchAccount(i), _) => {
                if let Some(a) = self.accounts.get(i).cloned() {
                    if self.account_key.as_deref() != Some(a.key.as_str()) {
                        let key = a.key;
                        self.account_task(move || cuentas::switch(&key).map(|a| format!("Claude Code ahora usa {}", a.email)));
                    }
                }
            }
            (Action::RemoveAccount(i), _) => {
                if let Some(a) = self.accounts.get(i).cloned() {
                    self.confirm = Some(confirm::remove_account(&a.email, a.plan.as_deref(), a.key));
                }
            }
            (Action::SaveAccount, _) => self.account_task(|| cuentas::save_current().map(|a| format!("Cuenta guardada: {}", a.email))),
            (Action::WebLogin, _) => self.remote_login_start(),
            (Action::WebCancel, _) => self.remote_login_cancel(),
            (Action::WebOpen, _) => {
                let url = self.remote_session.as_ref().map_or_else(|| self.remote_server(), |s| s.server.clone());
                let _ = std::process::Command::new("/usr/bin/open").arg(url).spawn();
            }
            (Action::WebOpenLogin, _) => {
                if let Some(url) = self.remote_login.as_ref().map(|l| l.url.clone()).filter(|u| !u.is_empty()) {
                    let _ = std::process::Command::new("/usr/bin/open").arg(url).spawn();
                }
            }
            (Action::WebToggle, _) => {
                self.config.remote = !self.config.remote;
                self.config.save();
                self.bridge_started = None;
                self.bridge_hold = false;
                if !self.config.remote {
                    self.remote_release_all();
                    self.remote_viewers.clear();
                }
                self.sync_bridge();
            }
            (Action::WebViewOnly, _) => {
                self.config.remote_view_only = !self.config.remote_view_only;
                self.config.save();
            }
            (Action::WebLogout, _) => self.remote_logout(),
            (Action::WebShowTab(id), _) => {
                if self.terms.contains_key(&id) {
                    self.show_tab(id);
                }
            }
            (Action::LoginAccount, _) => {
                // Antes de iniciar sesión con otra, se guarda la actual (si no, se perdería).
                if self.account_key.is_some() {
                    self.account_task(|| cuentas::save_current().map(|a| format!("Se guardó {} antes de iniciar sesión con otra cuenta", a.email)));
                }
                if let Some(id) = self.new_tab() {
                    self.write(id, b"claude auth login\r".to_vec());
                    self.after_switch();
                }
                notify_macos("Cuentas de Claude", "Al terminar de iniciar sesión, elige «Agregar la cuenta actual» en el menú del correo");
            }
            (Action::ToggleIdleExempt, _) => {
                if let Some(g) = self.ws.groups.get_mut(gi) {
                    g.idle_exempt = !g.idle_exempt;
                    self.save_session();
                }
            }
            _ => {}
        }
        self.window.request_redraw();
    }

    /// Teclas con el menú abierto: flechas, Enter y Esc. Cualquier otra lo cierra.
    pub(crate) fn menu_key(&mut self, event: &KeyEvent) {
        let Some(menu) = self.menu.as_mut() else { return };
        match &event.logical_key {
            Key::Named(NamedKey::ArrowDown) => menu.step(1),
            Key::Named(NamedKey::ArrowUp) => menu.step(-1),
            Key::Named(NamedKey::Enter) => {
                if let Some(action) = menu.hover.and_then(|i| menu.items[i].action) {
                    self.run_menu(action);
                }
            }
            _ => self.menu = None,
        }
        self.window.request_redraw();
    }
}
