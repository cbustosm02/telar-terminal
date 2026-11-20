//! Tabs y grupos: abrir, suspender y reanudar, cerrar, eliminar grupos, renombrar y
//! lo que se le manda al tab activo.

use crate::*;

impl State {
    /// Carpeta de inicio de la configuración, si sigue existiendo.
    pub(crate) fn start_dir(&self) -> Option<PathBuf> {
        self.config.start_dir.clone().filter(|d| d.is_dir())
    }

    /// Tab para un grupo nuevo: en la carpeta de inicio o, sin ella, en la del tab activo.
    pub(crate) fn spawn_term(&mut self) -> Option<u64> {
        let cwd = self.start_dir().or_else(|| self.active().and_then(|t| t.cwd()));
        self.spawn_term_in(cwd.as_deref())
    }

    pub(crate) fn spawn_term_in(&mut self, cwd: Option<&Path>) -> Option<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let t = self.spawn_with_id(id, cwd)?;
        self.terms.insert(id, t);
        Some(id)
    }

    pub(crate) fn spawn_with_id(&self, id: u64, cwd: Option<&Path>) -> Option<Terminal> {
        let socket = self.server.as_ref().map(|s| s.path.as_path());
        Terminal::spawn(id, self.layout().grid_size(), self.proxy.clone(), socket, cwd, self.config.scrollback)
            .map_err(|e| log::error!("no se pudo abrir la shell: {e}"))
            .ok()
    }

    /// Suspende un tab: guarda su pantalla y cierra su shell (y Claude, si corre ahí).
    /// Queda la vista previa; al reanudarlo, Claude retoma la misma sesión.
    pub(crate) fn suspend(&mut self, id: u64) -> bool {
        let Some(old) = self.terms.get_mut(&id) else { return false };
        if old.is_suspended() {
            return false;
        }
        let claude = old.agent.state != AgentState::None && old.agent.session_id.is_some();
        // Con Claude, su carpeta es donde se inició la sesión (la necesita --resume).
        let cwd = if claude { old.agent.cwd.as_ref().map(PathBuf::from).or_else(|| old.cwd()) } else { old.cwd() };
        let screen = screen::to_ansi(&old.term.lock());
        // Otra cosa que la shell o Claude en primer plano (una TUI, un servidor): se
        // vuelve a ejecutar al reanudar, como Claude con --resume.
        let command = if claude { None } else { old.foreground_command() };
        let title = old.title.take();
        let name = old.name.take();
        let mut agent = std::mem::take(&mut old.agent);
        let size = old.size();
        let last_used = old.last_used.get();
        let mut t = Terminal::frozen(id, size, self.proxy.clone(), Frozen { cwd, screen, command, lazy: false });
        t.last_used.set(last_used);
        t.title = title;
        t.name = name;
        if claude {
            agent.set_state(AgentState::Idle);
            agent.message = None;
            t.agent = agent;
        }
        // Al reemplazarla se suelta la terminal vieja: su shell recibe SIGHUP.
        self.terms.insert(id, t);
        self.window.request_redraw();
        true
    }

    /// Vuelve a abrir un tab suspendido en su carpeta; si tenía Claude, retoma la sesión.
    pub(crate) fn resume(&mut self, id: u64) -> bool {
        let Some(old) = self.terms.get(&id) else { return false };
        let Some(frozen) = &old.frozen else { return false };
        let cwd = frozen.cwd.clone();
        let command = frozen.command.clone();
        let session = old.agent.session_id.clone().filter(|_| old.agent.state != AgentState::None);
        let (title, name) = (old.title.clone(), old.name.clone());
        let Some(mut t) = self.spawn_with_id(id, cwd.as_deref()) else { return false };
        t.title = title;
        t.name = name;
        let claude = session.is_some_and(|s| resume_claude(&mut t, &s, cwd.as_deref()));
        if let Some(command) = command.filter(|_| !claude) {
            t.write(format!("{command}\r").into_bytes());
        }
        self.terms.insert(id, t);
        self.window.request_redraw();
        true
    }

    /// El tab a la vista quedó en espera al abrir Telar: arranca ahora.
    pub(crate) fn wake_lazy_active(&mut self) {
        let lazy = |t: &Terminal| t.frozen.as_ref().is_some_and(|f| f.lazy);
        if let Some(id) = self.ws.active_term().filter(|id| self.terms.get(id).is_some_and(lazy)) {
            // Si no se pudo abrir la shell, queda como suspendido (no se reintenta en cada vuelta).
            if !self.resume(id) {
                if let Some(f) = self.terms.get_mut(&id).and_then(|t| t.frozen.as_mut()) {
                    f.lazy = false;
                }
            }
        }
    }

    /// Tabs del grupo que se pueden suspender sin cortar trabajo: no están trabajando
    /// ni esperando un permiso.
    pub(crate) fn suspendable(&self, gi: usize) -> Vec<u64> {
        let Some(g) = self.ws.groups.get(gi) else { return Vec::new() };
        g.tabs
            .iter()
            .copied()
            .filter(|id| {
                self.terms.get(id).is_some_and(|t| {
                    !t.is_suspended() && !matches!(t.agent.state, AgentState::Working | AgentState::NeedsYou)
                })
            })
            .collect()
    }

    /// ⌘S: suspende o reanuda el tab activo.
    pub(crate) fn toggle_suspend_tab(&mut self) {
        let Some(id) = self.ws.active_term() else { return };
        if self.terms.get(&id).is_some_and(Terminal::is_suspended) {
            self.resume(id);
        } else {
            self.suspend(id);
        }
    }

    /// ⌘⇧S: suspende el grupo activo (salvo los tabs que trabajan); si ya está todo
    /// suspendido, lo reanuda entero.
    pub(crate) fn toggle_suspend_group(&mut self, gi: usize) {
        let awake = self.suspendable(gi);
        if awake.is_empty() {
            let tabs = self.ws.groups.get(gi).map(|g| g.tabs.clone()).unwrap_or_default();
            for id in tabs {
                self.resume(id);
            }
        } else {
            for id in awake {
                self.suspend(id);
            }
        }
    }

    pub(crate) fn new_group(&mut self) -> Option<u64> {
        let id = self.spawn_term()?;
        self.ws.add_group(id);
        self.reveal_active_group();
        self.window.request_redraw();
        Some(id)
    }

    /// Carpeta de un tab nuevo en el grupo activo: la del grupo; si no tiene, la de inicio
    /// de la configuración; y si tampoco hay, la del tab activo.
    pub(crate) fn new_tab_cwd(&self) -> Option<PathBuf> {
        let dir = self.ws.group().and_then(|g| g.dir.clone()).filter(|d| d.is_dir());
        dir.or_else(|| self.start_dir()).or_else(|| self.active().and_then(|t| t.cwd()))
    }

    pub(crate) fn new_tab(&mut self) -> Option<u64> {
        if self.ws.groups.is_empty() {
            return self.new_group();
        }
        let cwd = self.new_tab_cwd();
        let id = self.spawn_term_in(cwd.as_deref())?;
        self.ws.add_tab(id);
        self.window.request_redraw();
        Some(id)
    }

    /// ⌘⇧T: tab nuevo que arranca Claude Code (en la carpeta del grupo).
    pub(crate) fn new_agent(&mut self) {
        if let Some(id) = self.new_tab() {
            // La shell todavía arranca: lo escrito queda en el buffer del PTY y se ejecuta al estar lista.
            self.write(id, b"claude\r".to_vec());
        }
    }

    /// La primera sesión de Claude de un grupo sin carpeta le da la suya; si el grupo aún
    /// se llama «Grupo N», también su nombre. Así sus tabs y agentes nuevos abren en ese proyecto.
    pub(crate) fn adopt_group_dir(&mut self, tab: u64, dir: &Path) {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        if home.as_deref() == Some(dir) || !dir.is_dir() {
            return;
        }
        let Some(gi) = self.ws.group_of(tab) else { return };
        let g = &mut self.ws.groups[gi];
        if g.dir.is_some() {
            return;
        }
        g.dir = Some(dir.to_path_buf());
        if workspace::is_default_name(&g.name) {
            if let Some(base) = dir.file_name() {
                g.name = base.to_string_lossy().into_owned();
            }
        }
        self.window.request_redraw();
    }

    /// Pide confirmación para eliminar el grupo `gi` con todos sus tabs.
    pub(crate) fn ask_delete_group(&mut self, gi: usize) {
        let Some(g) = self.ws.groups.get(gi) else { return };
        let Some(&anchor) = g.tabs.first() else { return };
        let terms: Vec<&Terminal> = g.tabs.iter().filter_map(|id| self.terms.get(id)).collect();
        let claude = terms.iter().filter(|t| t.agent.session_id.is_some()).count();
        let working = terms.iter().filter(|t| t.agent.state == AgentState::Working).count();
        self.confirm = Some(confirm::delete_group(&g.name, g.tabs.len(), claude, working, anchor));
        self.menu = None;
        self.editing = None;
        self.drag = None;
        self.hover = Hit::None;
        self.window.set_cursor(CursorIcon::Default);
        self.window.request_redraw();
    }

    /// Hace lo que se confirmó en el diálogo y lo cierra.
    pub(crate) fn run_confirm(&mut self) {
        let Some(c) = self.confirm.take() else { return };
        match c.pending {
            confirm::Pending::DeleteGroup { anchor } => {
                if let Some(gi) = self.ws.group_of(anchor) {
                    self.delete_group(gi);
                }
            }
            confirm::Pending::RemoveAccount { key } => {
                self.account_task(move || cuentas::remove(&key).map(|()| "Cuenta quitada".to_string()));
            }
        }
    }

    /// Elimina el grupo `gi` y cierra sus tabs (con lo que corre en ellos).
    pub(crate) fn delete_group(&mut self, gi: usize) {
        for id in self.ws.remove_group(gi) {
            self.terms.remove(&id);
        }
        if self.ws.groups.is_empty() {
            // Era el único: Telar sigue con un grupo nuevo en lugar de cerrarse.
            self.new_group();
        }
        // Los grupos pasados de su límite se indican por índice: se recalculan.
        self.enforce_limits();
        self.after_switch();
        self.save_session();
    }

    /// Cierra una terminal. Devuelve `true` si ya no queda ninguna (hay que salir).
    pub(crate) fn close_term(&mut self, id: u64) -> bool {
        self.terms.remove(&id);
        self.editing = None;
        let empty = self.ws.remove_term(id);
        self.window.request_redraw();
        empty
    }

    pub(crate) fn write(&self, id: u64, bytes: Vec<u8>) {
        if let Some(t) = self.terms.get(&id) {
            t.write(bytes);
        }
    }

    pub(crate) fn active(&self) -> Option<&Terminal> {
        self.ws.active_term().and_then(|id| self.terms.get(&id))
    }

    pub(crate) fn active_mode(&self) -> TermMode {
        self.active().map_or(TermMode::empty(), |t| *t.term.lock().mode())
    }

    /// Envía bytes al tab activo y baja al final del scrollback.
    pub(crate) fn send_input(&self, bytes: Vec<u8>) {
        if let Some(t) = self.active() {
            {
                let mut term = t.term.lock();
                if term.grid().display_offset() != 0 {
                    term.scroll_display(Scroll::Bottom);
                    self.window.request_redraw();
                }
            }
            t.touch();
            t.write(bytes);
        }
    }

    pub(crate) fn resize_terms(&mut self) {
        let size = self.layout().grid_size();
        for t in self.terms.values_mut() {
            t.resize(size);
        }
    }

    /// Solo el tab visible (los demás quedan para `resize_pending`).
    pub(crate) fn resize_visible(&mut self) {
        let size = self.layout().grid_size();
        if let Some(t) = self.ws.active_term().and_then(|id| self.terms.get_mut(&id)) {
            t.resize(size);
        }
    }

    pub(crate) fn apply_font_size(&mut self) {
        self.fonts.set_size(self.font_pt * self.window.scale_factor() as f32);
        self.resize_terms();
        self.window.request_redraw();
    }

    pub(crate) fn copy_selection(&mut self) {
        let text = self.active().and_then(|t| t.term.lock().selection_to_string());
        if let (Some(text), Some(cb)) = (text, self.clipboard.as_mut()) {
            let _ = cb.set_text(text);
        }
    }

    pub(crate) fn paste(&mut self) {
        let text = self.clipboard.as_mut().and_then(|cb| cb.get_text().ok()).unwrap_or_default();
        if text.is_empty() {
            // Sin texto (p. ej. una imagen copiada): con bracketed paste se manda un pegado
            // vacío, como iTerm2 o Ghostty. Claude Code lo interpreta como «leer la imagen
            // del portapapeles» y la adjunta.
            if self.editing.is_none() && self.active_mode().contains(TermMode::BRACKETED_PASTE) {
                self.send_input(input::paste_bytes("", self.active_mode()));
            }
            return;
        }
        if let Some((_, buf)) = self.editing.as_mut() {
            buf.extend(text.chars().filter(|c| !c.is_control()));
            self.window.request_redraw();
            return;
        }
        let bytes = input::paste_bytes(&text, self.active_mode());
        self.send_input(bytes);
    }

    pub(crate) fn start_rename(&mut self, group: usize) {
        if let Some(g) = self.ws.groups.get(group) {
            self.editing = Some((Edit::Group(group), g.name.clone()));
            self.window.request_redraw();
        }
    }

    /// Renombrar un tab: se edita en la barra de tabs (el tab pasa a estar visible).
    pub(crate) fn start_rename_tab(&mut self, id: u64) {
        self.show_tab(id);
        if let Some(t) = self.terms.get(&id) {
            self.editing = Some((Edit::Tab(id), t.label().unwrap_or_default().to_string()));
            self.window.request_redraw();
        }
    }

    /// Lleva a la vista el tab `id` (su grupo y su posición).
    pub(crate) fn show_tab(&mut self, id: u64) {
        let Some(gi) = self.ws.group_of(id) else { return };
        let ti = self.ws.groups[gi].tabs.iter().position(|t| *t == id).unwrap_or(0);
        self.ws.select_group(gi);
        self.ws.select_tab(ti);
        self.after_switch();
    }

    /// ⌘K: borra la pantalla y el scrollback. Si en primer plano hay una shell o Claude,
    /// se le manda Ctrl-L para que vuelva a dibujar su prompt arriba.
    pub(crate) fn clear_terminal(&mut self, id: u64) {
        let Some(t) = self.terms.get(&id).filter(|t| !t.is_suspended()) else { return };
        {
            let mut term = t.term.lock();
            term.scroll_display(Scroll::Bottom);
            // All lleva la pantalla al scrollback; Saved vacía el scrollback.
            term.clear_screen(ClearMode::All);
            term.clear_screen(ClearMode::Saved);
            term.selection = None;
        }
        t.changed();
        let redraw = t.process.as_deref().is_none_or(|p| proc::is_shell(p) || p == "claude");
        if redraw {
            t.write(b"\x0c".to_vec());
        }
        self.window.request_redraw();
    }

    /// Al cambiar de tab o grupo: el tamaño puede haber cambiado (nº de tabs) y hay que repintar.
    pub(crate) fn after_switch(&mut self) {
        self.editing = None;
        self.reveal_active_group();
        self.mark_seen();
        self.resize_terms();
        self.window.request_redraw();
    }
}
