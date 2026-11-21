//! Sesión guardada: qué se guarda de cada tab y cómo se restaura al abrir Telar.

use crate::*;

impl State {
    /// Lo que se guarda de la sesión. Con `screens` (al cerrar Telar), también la pantalla
    /// de los tabs abiertos: con la reanudación perezosa, es su vista previa al volver.
    pub(crate) fn saved_state(&self, screens: bool) -> Saved {
        let groups = self
            .ws
            .groups
            .iter()
            .map(|g| SavedGroup {
                name: g.name.clone(),
                dir: g.dir.clone(),
                ram_limit_mb: g.ram_limit_mb,
                idle_exempt: g.idle_exempt,
                active: g.active,
                tabs: g
                    .tabs
                    .iter()
                    .filter_map(|id| self.terms.get(id))
                    .map(|t| {
                        let claude = (t.agent.state != AgentState::None).then(|| t.agent.session_id.clone()).flatten();
                        let lazy = t.frozen.as_ref().is_none_or(|f| f.lazy);
                        let screen = match &t.frozen {
                            Some(f) => Some(f.screen.clone()),
                            None if screens => Some(screen::to_ansi(&t.term.lock())),
                            None => None,
                        };
                        // Con Claude abierto, su carpeta es donde se inició la sesión (la necesita --resume).
                        let cwd = if t.is_suspended() {
                            t.cwd()
                        } else if claude.is_some() {
                            t.agent.cwd.as_ref().map(PathBuf::from).or_else(|| t.cwd())
                        } else {
                            t.cwd()
                        };
                        // Suspendido: lo que corría al suspenderlo. Abierto con una conexión remota
                        // (ssh, mosh) en primer plano: esa conexión, para volver a abrirla al
                        // reiniciar Telar (otros programas no se relanzan solos).
                        let command = match &t.frozen {
                            Some(f) => f.command.clone(),
                            // `process` se refresca cada 5 s: un tab recién abierto todavía no
                            // lo tiene, y se mira igual.
                            None if t.is_remote() || t.process.is_none() => t.foreground_command().filter(|c| proc::is_remote_command(c)),
                            None => None,
                        };
                        SavedTab { title: t.title.clone(), name: t.name.clone(), cwd, claude, screen, command, lazy }
                    })
                    .collect(),
            })
            .collect();
        Saved { active: self.ws.active, groups, awake: self.awake.mode.name().to_string(), window: self.window_frame() }
    }

    /// Dónde está la ventana ahora, en puntos (ver `WindowFrame`).
    fn window_frame(&self) -> Option<session::WindowFrame> {
        if self.snapshot.is_some() {
            return None;
        }
        let scale = self.window.scale_factor();
        let pos = self.window.outer_position().ok()?.to_logical::<f64>(scale);
        let size = self.window.inner_size().to_logical::<f64>(scale);
        Some(session::WindowFrame {
            x: pos.x.round(),
            y: pos.y.round(),
            w: size.width.round(),
            h: size.height.round(),
            maximized: self.window.is_maximized(),
            fullscreen: self.window.fullscreen().is_some(),
        })
    }

    pub(crate) fn save_session(&mut self) {
        self.save_session_with(false);
    }

    /// Al cerrar Telar: con la última pantalla de cada tab abierto.
    pub(crate) fn save_session_final(&mut self) {
        self.save_session_with(true);
    }

    pub(crate) fn save_session_with(&mut self, screens: bool) {
        if !self.persist {
            return;
        }
        let saved = self.saved_state(screens);
        if self.last_saved.as_ref() != Some(&saved) {
            session::save(&saved);
            self.last_saved = Some(saved);
        }
    }

    /// Vuelve a abrir los grupos y tabs guardados; los que tenían Claude retoman su sesión.
    pub(crate) fn restore(&mut self, saved: Saved) {
        self.awake.mode = awake::Mode::parse(&saved.awake).unwrap_or_default();
        let mut groups = Vec::new();
        let n_groups = saved.groups.len();
        for (gi, g) in saved.groups.into_iter().enumerate() {
            let mut tabs = Vec::new();
            let n_tabs = g.tabs.len();
            for (ti, mut tab) in g.tabs.into_iter().enumerate() {
                let id = self.next_id;
                // Con la reanudación perezosa, solo arranca el tab que queda a la vista; los
                // demás que estaban abiertos esperan con su última pantalla.
                let visible = gi == saved.active.min(n_groups - 1) && ti == g.active.min(n_tabs - 1);
                let lazy = tab.lazy && self.config.lazy_restore && !visible;
                if tab.lazy && !lazy {
                    tab.screen = None;
                }
                if lazy && tab.screen.is_none() {
                    tab.screen = Some(String::new());
                }
                // Suspendido (o en espera de mostrarse): no arranca nada, solo su vista previa.
                let mut t = match tab.screen {
                    Some(screen) => {
                        self.next_id += 1;
                        let frozen = Frozen { cwd: tab.cwd.clone(), screen, command: tab.command.clone(), lazy };
                        let mut t = Terminal::frozen(id, self.layout().grid_size(), self.proxy.clone(), frozen);
                        if let Some(session) = tab.claude.as_ref().filter(|s| claude_session_exists(s)) {
                            t.agent.session_id = Some(session.clone());
                            t.agent.cwd = tab.cwd.as_ref().map(|c| c.display().to_string());
                            t.agent.set_state(AgentState::Idle);
                        }
                        t
                    }
                    None => {
                        let Some(id) = self.spawn_term_in(tab.cwd.as_deref()) else { continue };
                        let mut t = self.terms.remove(&id).unwrap();
                        let claude = tab.claude.as_ref().is_some_and(|s| resume_claude(&mut t, s, tab.cwd.as_deref()));
                        // Era un Claude sin conversación: el título que puso Claude ya no corresponde.
                        if tab.claude.is_some() && !claude {
                            tab.title = None;
                        }
                        // Una conexión remota que estaba abierta (o lo que corría al suspenderlo):
                        // la shell todavía arranca, lo escrito se ejecuta al estar lista.
                        if let Some(command) = tab.command.as_ref().filter(|_| !claude) {
                            t.write(format!("{command}\r").into_bytes());
                        }
                        t
                    }
                };
                t.title = tab.title;
                t.name = tab.name;
                self.terms.insert(id, t);
                tabs.push(id);
            }
            if !tabs.is_empty() {
                let active = g.active.min(tabs.len() - 1);
                groups.push(Group { name: g.name, tabs, active, dir: g.dir, ram_limit_mb: g.ram_limit_mb, idle_exempt: g.idle_exempt });
            }
        }
        self.ws.restore(groups, saved.active);
    }
}
