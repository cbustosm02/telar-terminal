//! Modal de configuración (⌘,): lo que cambia cada opción.

use crate::*;

impl State {
    /// ⌘, o el engranaje: abre o cierra la configuración.
    pub(crate) fn toggle_settings(&mut self) {
        self.menu = None;
        self.editing = None;
        self.settings = match self.settings {
            Some(_) => None,
            None => Some(settings::Settings::open(&self.config.font_family)),
        };
        self.window.set_cursor(CursorIcon::Default);
        self.window.request_redraw();
    }

    /// Cambia la familia de la terminal y la interfaz, y la guarda.
    pub(crate) fn set_font_family(&mut self, family: &str) {
        if family.eq_ignore_ascii_case(&self.fonts.family) {
            return;
        }
        self.fonts = Fonts::new(family, self.font_pt * self.window.scale_factor() as f32);
        self.config.font_family = self.fonts.family.clone();
        self.config.save();
        self.resize_terms();
        self.window.request_redraw();
    }

    /// Cambia el ícono de la app (Dock y Telar.app) y lo guarda.
    pub(crate) fn set_app_icon(&mut self, id: &str) {
        let result = appicon::apply(id);
        if let Some(st) = self.settings.as_mut() {
            st.icon_error = result.as_ref().err().cloned();
        }
        if result.is_ok() {
            self.config.app_icon = id.to_string();
            self.config.save();
        }
        self.window.request_redraw();
    }

    /// «Elegir archivo…»: abre el panel de macOS (fuera del manejador del clic) y la imagen
    /// elegida pasa a ser el ícono propio.
    pub(crate) fn pick_app_icon(&mut self) {
        let proxy = self.proxy.clone();
        appicon::later_on_main(move || {
            let _ = proxy.send_event(UserEvent::Picked(Picked::Icon(appicon::pick_file())));
        });
    }

    /// Abre el panel para elegir la carpeta de inicio (fuera del manejador del clic).
    pub(crate) fn pick_start_dir(&mut self) {
        let proxy = self.proxy.clone();
        let current = self.start_dir().or_else(|| std::env::var_os("HOME").map(PathBuf::from));
        appicon::later_on_main(move || {
            let dir = appicon::pick_folder("Elige la carpeta donde abren los tabs nuevos", current.as_deref());
            let _ = proxy.send_event(UserEvent::Picked(Picked::StartDir(dir)));
        });
    }

    /// Resultado de un panel de macOS.
    pub(crate) fn on_picked(&mut self, picked: Picked) {
        match picked {
            Picked::StartDir(Some(dir)) if dir.is_absolute() => {
                self.config.start_dir = Some(dir);
                self.config.save();
            }
            Picked::Icon(Some(path)) => match appicon::import(&path) {
                Ok(()) => self.set_app_icon(appicon::CUSTOM),
                Err(e) => {
                    if let Some(st) = self.settings.as_mut() {
                        st.icon_error = Some(e);
                    }
                }
            },
            Picked::StartDir(_) | Picked::Icon(None) => {}
        }
        self.window.request_redraw();
    }

    /// − / + del historial por tab: se aplica también a los tabs abiertos (al achicarlo,
    /// lo más viejo se descarta).
    pub(crate) fn set_scrollback(&mut self, delta: i32) {
        self.config.scrollback = config::step_scrollback(self.config.scrollback, delta);
        self.config.save();
        for t in self.terms.values().filter(|t| !t.is_suspended()) {
            t.set_scrollback(self.config.scrollback);
        }
    }

    /// Cambia el tema (interfaz y colores de la terminal) y lo guarda.
    pub(crate) fn set_theme(&mut self, name: &str) {
        if !theme::set(name) {
            return;
        }
        self.palette = theme::t().palette();
        self.config.theme = theme::t().name.to_string();
        self.config.save();
        self.window.request_redraw();
    }

    /// Tema anterior o siguiente de la lista (← → en la configuración).
    pub(crate) fn cycle_theme(&mut self, delta: isize) {
        let list = theme::themes();
        let i = list.iter().position(|t| t.name == theme::t().name).unwrap_or(0) as isize;
        let next = &list[(i + delta).rem_euclid(list.len() as isize) as usize];
        self.set_theme(next.name);
    }

    /// Tamaño desde la configuración: se aplica y queda como el de ⌘0.
    pub(crate) fn set_font_size(&mut self, pt: f32) {
        self.font_pt = pt.clamp(config::FONT_SIZE_MIN, config::FONT_SIZE_MAX);
        self.config.font_size = self.font_pt;
        self.config.save();
        self.apply_font_size();
    }

    pub(crate) fn settings_click(&mut self) {
        let l = self.layout();
        let Some(st) = self.settings.as_mut() else { return };
        match st.hit(self.cursor.0, self.cursor.1, &l, &self.fonts, self.ws.groups.len()) {
            settings::Hit::Outside | settings::Hit::Close => self.settings = None,
            settings::Hit::Section(i) => st.section = i,
            settings::Hit::Font(i) => {
                let page = st.page(&l, &self.fonts);
                st.select(i, page);
                let name = st.visible()[i].name.clone();
                self.set_font_family(&name);
            }
            settings::Hit::Theme(i) => {
                if let Some(t) = theme::themes().get(i) {
                    self.set_theme(t.name);
                }
            }
            settings::Hit::Icon(i) => {
                let id = appicon::CHOICES.get(i).map_or(appicon::CUSTOM, |c| c.id);
                if id == appicon::CUSTOM && !appicon::has_custom() {
                    self.pick_app_icon();
                } else {
                    self.set_app_icon(id);
                }
            }
            settings::Hit::PickIcon => self.pick_app_icon(),
            settings::Hit::PickStartDir => self.pick_start_dir(),
            settings::Hit::ShowClaude => {
                // Oculto, no se consulta nada (ni el uso ni la cuenta); al volver, se consulta ya.
                self.config.show_claude = !self.config.show_claude;
                self.config.save();
                if self.config.show_claude {
                    self.refresh_claude_account();
                }
            }
            settings::Hit::ShowMeters => {
                // Oculto, el muestreo se detiene (salvo que haya límites de memoria).
                self.config.show_meters = !self.config.show_meters;
                self.config.save();
                self.update_stats_needed();
            }
            settings::Hit::LazyRestore => {
                self.config.lazy_restore = !self.config.lazy_restore;
                self.config.save();
            }
            settings::Hit::ScrollbackDown => self.set_scrollback(-1),
            settings::Hit::ScrollbackUp => self.set_scrollback(1),
            settings::Hit::FullDiskAccess => {
                let _ = std::process::Command::new("/usr/bin/open")
                    .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
                    .spawn();
            }
            settings::Hit::ClearStartDir => {
                self.config.start_dir = None;
                self.config.save();
            }
            settings::Hit::LimitDown(i) => self.step_limit(i, -1),
            settings::Hit::LimitUp(i) => self.step_limit(i, 1),
            settings::Hit::AutoSuspend => {
                self.config.auto_suspend = !self.config.auto_suspend;
                self.config.save();
                self.enforce_limits();
            }
            settings::Hit::IdleTabDown => self.set_idle(|r| r.tab_min = idle::step(r.tab_min, -1)),
            settings::Hit::IdleTabUp => self.set_idle(|r| r.tab_min = idle::step(r.tab_min, 1)),
            settings::Hit::IdleGroupDown => self.set_idle(|r| r.group_min = idle::step(r.group_min, -1)),
            settings::Hit::IdleGroupUp => self.set_idle(|r| r.group_min = idle::step(r.group_min, 1)),
            settings::Hit::IdleWaiting => self.set_idle(|r| r.include_waiting = !r.include_waiting),
            settings::Hit::IdleRunning => self.set_idle(|r| r.include_running = !r.include_running),
            settings::Hit::IdleExempt(gi) => {
                if let Some(g) = self.ws.groups.get_mut(gi) {
                    g.idle_exempt = !g.idle_exempt;
                    self.save_session();
                }
            }
            settings::Hit::SizeDown => self.set_font_size(self.font_pt - 1.0),
            settings::Hit::SizeUp => self.set_font_size(self.font_pt + 1.0),
            settings::Hit::Inside => {}
        }
        // Cambiar de sección (p. ej. a Rendimiento) puede necesitar el muestreo.
        self.update_stats_needed();
        self.window.request_redraw();
    }

    /// Teclado con la configuración abierta: escribir filtra, flechas eligen (y aplican),
    /// ⌘+/⌘- cambian el tamaño, Esc o ⌘, cierran.
    pub(crate) fn settings_key(&mut self, event: &KeyEvent) {
        let l = self.layout();
        let cmd = self.mods.super_key();
        let Some(st) = self.settings.as_mut() else { return };
        let page = st.page(&l, &self.fonts);
        let mut apply = false;
        // Las flechas y el filtro son de la sección Apariencia.
        if st.section != settings::APPEARANCE_SECTION {
            let close = event.logical_key == Key::Named(NamedKey::Escape)
                || (cmd && matches!(event.key_without_modifiers(), Key::Character(ref c) if c.as_str() == "," || c.as_str() == "w"));
            if close {
                self.settings = None;
            }
            self.window.request_redraw();
            return;
        }
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => self.settings = None,
            Key::Named(NamedKey::ArrowDown) => {
                st.select(st.selected + 1, page);
                apply = true;
            }
            Key::Named(NamedKey::ArrowUp) => {
                st.select(st.selected.saturating_sub(1), page);
                apply = true;
            }
            Key::Named(NamedKey::PageDown) => {
                st.select(st.selected + page, page);
                apply = true;
            }
            Key::Named(NamedKey::PageUp) => {
                st.select(st.selected.saturating_sub(page), page);
                apply = true;
            }
            Key::Named(NamedKey::Enter) => apply = true,
            Key::Named(NamedKey::ArrowLeft) => {
                self.cycle_theme(-1);
                return;
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.cycle_theme(1);
                return;
            }
            Key::Named(NamedKey::Backspace) => {
                input::delete_back(&mut st.filter, self.mods);
                st.select(0, page);
                st.scroll = 0;
            }
            _ if cmd => match event.key_without_modifiers() {
                Key::Character(c) if c.as_str() == "," || c.as_str() == "w" => self.settings = None,
                Key::Character(c) if c.as_str() == "=" || c.as_str() == "+" => self.set_font_size(self.font_pt + 1.0),
                Key::Character(c) if c.as_str() == "-" => self.set_font_size(self.font_pt - 1.0),
                Key::Character(c) if c.as_str() == "v" => {
                    if let Some(text) = self.clipboard.as_mut().and_then(|cb| cb.get_text().ok()) {
                        if let Some(st) = self.settings.as_mut() {
                            st.filter.extend(text.chars().filter(|c| !c.is_control()));
                            st.select(0, page);
                            st.scroll = 0;
                        }
                    }
                }
                _ => {}
            },
            _ => {
                if let Some(t) = &event.text {
                    st.filter.extend(t.chars().filter(|c| !c.is_control()));
                    st.select(0, page);
                    st.scroll = 0;
                }
            }
        }
        if apply {
            if let Some(name) = self.settings.as_ref().and_then(|st| st.visible().get(st.selected).map(|f| f.name.clone())) {
                self.set_font_family(&name);
            }
        }
        self.window.request_redraw();
    }
}
