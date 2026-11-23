//! Teclado: atajos con ⌘, edición de nombres y lo que va a la terminal.

use crate::*;

impl State {
    /// Devuelve `true` si hay que cerrar la app.
    pub(crate) fn on_key(&mut self, event: &KeyEvent) -> bool {
        let m = self.mods;
        // Al escribir, el cursor se ve (y vuelve a titilar desde aquí).
        self.caret_since = Instant::now();
        self.next_caret = self.caret_since + CARET_BLINK;

        if self.confirm.is_some() {
            match event.logical_key {
                Key::Named(NamedKey::Enter) => self.run_confirm(),
                Key::Named(NamedKey::Escape) => self.confirm = None,
                _ => {}
            }
            self.window.request_redraw();
            return false;
        }
        if self.inbox.is_some() {
            self.inbox_key(event);
            return false;
        }
        if self.menu.is_some() {
            self.menu_key(event);
            return false;
        }
        if self.settings.is_some() {
            self.settings_key(event);
            return false;
        }
        if self.search.is_some() {
            self.search_key(event);
            return false;
        }
        if self.stats_detail.is_some() {
            let close = event.logical_key == Key::Named(NamedKey::Escape)
                || (self.mods.super_key() && matches!(event.key_without_modifiers(), Key::Character(ref c) if c.as_str() == "w"));
            if close {
                self.toggle_stats();
            }
            return false;
        }

        // Renombrando un grupo o un tab: el teclado edita el nombre.
        if let Some((target, buf)) = self.editing.as_mut() {
            match &event.logical_key {
                Key::Named(NamedKey::Enter) => {
                    let name = buf.trim().to_string();
                    match *target {
                        Edit::Group(gi) => {
                            if let Some(g) = self.ws.groups.get_mut(gi).filter(|_| !name.is_empty()) {
                                g.name = name;
                            }
                        }
                        // Vacío: vuelve el título automático.
                        Edit::Tab(id) => {
                            if let Some(t) = self.terms.get_mut(&id) {
                                t.name = (!name.is_empty()).then_some(name);
                            }
                        }
                    }
                    self.editing = None;
                }
                Key::Named(NamedKey::Escape) => self.editing = None,
                Key::Named(NamedKey::Backspace) => input::delete_back(buf, m),
                _ if m.super_key() => {
                    if matches!(event.key_without_modifiers(), Key::Character(ref s) if s.as_str() == "v") {
                        self.paste();
                    }
                }
                _ => {
                    if let Some(t) = &event.text {
                        buf.extend(t.chars().filter(|c| !c.is_control()));
                    }
                }
            }
            self.window.request_redraw();
            return false;
        }

        if m.super_key() {
            return self.shortcut(event);
        }
        if m.control_key() && event.logical_key == Key::Named(NamedKey::Tab) {
            self.ws.cycle_tab(if m.shift_key() { -1 } else { 1 });
            self.after_switch();
            return false;
        }

        if let Some(id) = self.ws.active_term().filter(|id| self.terms.get(id).is_some_and(Terminal::is_suspended)) {
            if event.logical_key == Key::Named(NamedKey::Enter) {
                self.resume(id);
            }
            return false;
        }
        if let Some(bytes) = input::key_to_bytes(event, m, self.active_mode()) {
            self.send_input(bytes);
        }
        false
    }

    /// Atajos con ⌘. Devuelve `true` si hay que cerrar la app.
    pub(crate) fn shortcut(&mut self, event: &KeyEvent) -> bool {
        let m = self.mods;
        if let Key::Named(named) = &event.logical_key {
            // ⌘⌫ borra la línea hasta el cursor (Ctrl+U); ⌘← y ⌘→ van al inicio y al final
            // de la línea (Ctrl+A y Ctrl+E), como en Terminal.app e iTerm. Funcionan en zsh,
            // bash y en el prompt de Claude Code.
            let line_key = match named {
                NamedKey::Backspace => Some(0x15),
                NamedKey::ArrowLeft => Some(0x01),
                NamedKey::ArrowRight => Some(0x05),
                _ => None,
            };
            if let Some(byte) = line_key.filter(|_| !m.shift_key()) {
                if self.active().is_some_and(|t| !t.is_suspended()) {
                    self.send_input(vec![byte]);
                }
                return false;
            }
            match (named, m.shift_key()) {
                (NamedKey::ArrowRight, true) => self.ws.cycle_tab(1),
                (NamedKey::ArrowLeft, true) => self.ws.cycle_tab(-1),
                (NamedKey::ArrowDown, true) => self.ws.cycle_group(1),
                (NamedKey::ArrowUp, true) => self.ws.cycle_group(-1),
                _ => return false,
            }
            self.after_switch();
            return false;
        }
        let Key::Character(key) = event.key_without_modifiers() else { return false };
        match key.to_lowercase().as_str() {
            "t" if m.shift_key() => self.new_agent(),
            "t" => {
                self.new_tab();
            }
            "n" => {
                self.new_group();
            }
            "w" => {
                if let Some(id) = self.ws.active_term() {
                    return self.close_term(id);
                }
            }
            "q" => return true,
            "j" if m.shift_key() => self.toggle_inbox(),
            "j" => self.jump_to_attention(),
            "c" if m.shift_key() => self.cycle_awake(),
            "c" => self.copy_selection(),
            "v" => self.paste(),
            "r" if m.shift_key() => self.start_rename(self.ws.active),
            "s" if m.shift_key() => self.toggle_suspend_group(self.ws.active),
            "k" => {
                if let Some(id) = self.ws.active_term() {
                    self.clear_terminal(id);
                }
            }
            "s" => self.toggle_suspend_tab(),
            "=" | "+" => {
                self.font_pt = (self.font_pt + 1.0).min(36.0);
                self.apply_font_size();
            }
            "-" => {
                self.font_pt = (self.font_pt - 1.0).max(8.0);
                self.apply_font_size();
            }
            "0" => {
                self.font_pt = self.config.font_size;
                self.apply_font_size();
            }
            "," => self.toggle_settings(),
            "p" => self.open_search(),
            d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() => {
                let n = (d.as_bytes()[0] - b'0') as usize;
                if n > 0 {
                    self.ws.select_tab(n - 1);
                    self.after_switch();
                }
            }
            _ => {}
        }
        false
    }

    pub(crate) fn on_ime(&mut self, ime: Ime) {
        match ime {
            Ime::Preedit(text, _) => {
                self.preedit = text;
                self.window.request_redraw();
            }
            Ime::Commit(text) => {
                self.preedit.clear();
                if self.confirm.is_some() {
                    // Con la confirmación abierta, nada llega a la terminal.
                } else if let Some((_, buf)) = self.editing.as_mut() {
                    buf.push_str(&text);
                    self.window.request_redraw();
                } else {
                    self.send_input(text.into_bytes());
                }
            }
            Ime::Enabled | Ime::Disabled => {}
        }
    }

    /// Hay un campo de texto con el foco: renombrando, buscando o filtrando fuentes.
    pub(crate) fn text_field_focused(&self) -> bool {
        self.editing.is_some() || self.search.is_some() || self.settings.as_ref().is_some_and(|s| s.section == settings::APPEARANCE_SECTION)
    }

    /// El cursor está en su mitad visible: medio segundo sí, medio no, contando desde la
    /// última tecla (así se ve mientras escribes). Sin el foco de la ventana, fijo.
    pub(crate) fn caret_visible(&self, now: Instant) -> bool {
        !self.focused || (now.duration_since(self.caret_since).as_millis() / CARET_BLINK.as_millis()).is_multiple_of(2)
    }

    /// Cuándo cambia el cursor la próxima vez.
    pub(crate) fn next_caret_toggle(&self, now: Instant) -> Instant {
        let n = now.duration_since(self.caret_since).as_millis() / CARET_BLINK.as_millis() + 1;
        self.caret_since + CARET_BLINK * n as u32
    }
}

/// Medio ciclo del cursor de los campos de texto (como en macOS).
pub(crate) const CARET_BLINK: Duration = Duration::from_millis(530);
