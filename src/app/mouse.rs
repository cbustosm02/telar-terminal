//! Mouse: puntero, clics, arrastre para reordenar, enlaces y desplazamiento.

use crate::*;

impl State {
    /// Enlaces de la pantalla del tab visible. Se buscan de nuevo solo si cambió lo que
    /// muestra: el dibujo y el puntero los piden muchas veces por segundo.
    pub(crate) fn visible_links(&self) -> std::rc::Rc<Vec<links::Link>> {
        let Some((id, t)) = self.ws.active_term().and_then(|id| Some((id, self.terms.get(&id)?))) else {
            return Default::default();
        };
        let term = t.term.lock();
        let size = t.size();
        let key = (id, t.generation, t.version.get(), term.grid().display_offset(), size.cols, size.rows);
        let mut cache = self.links_cache.borrow_mut();
        if let Some((_, links)) = cache.as_ref().filter(|(k, _)| *k == key) {
            return links.clone();
        }
        let links = std::rc::Rc::new(links::scan(&*term));
        *cache = Some((key, links.clone()));
        links
    }

    pub(crate) fn on_cursor_moved(&mut self, pos: PhysicalPosition<f64>) {
        self.cursor = (pos.x as f32, pos.y as f32);
        if self.drag.is_some() {
            self.drag_move();
            return;
        }
        let l = self.layout();
        if let Some(c) = self.confirm.as_mut() {
            let hover = Some(c.hit(self.cursor.0, self.cursor.1, &l, &mut self.fonts));
            if hover != c.hover {
                c.hover = hover;
                let pointer = matches!(hover, Some(confirm::Hit::Cancel | confirm::Hit::Confirm));
                self.window.set_cursor(if pointer { CursorIcon::Pointer } else { CursorIcon::Default });
                self.window.request_redraw();
            }
            return;
        }
        if self.inbox.is_some() {
            let entries = inbox::entries(&self.ws, &self.terms);
            let hit = self.inbox.as_ref().map(|b| b.hit(self.cursor.0, self.cursor.1, &l, &mut self.fonts, &entries));
            if let Some(b) = self.inbox.as_mut() {
                if hit != b.hover {
                    b.hover = hit;
                    let pointer = matches!(hit, Some(inbox::Hit::Close | inbox::Hit::Row(_) | inbox::Hit::Go(_) | inbox::Hit::Option(..)));
                    self.window.set_cursor(if pointer { CursorIcon::Pointer } else { CursorIcon::Default });
                    self.window.request_redraw();
                }
            }
            return;
        }
        if let Some(d) = self.stats_detail.as_mut() {
            let hover = Some(d.hit(self.cursor.0, self.cursor.1, &l));
            if hover != d.hover {
                d.hover = hover;
                self.window.set_cursor(if hover == Some(statsview::Hit::Close) { CursorIcon::Pointer } else { CursorIcon::Default });
                self.window.request_redraw();
            }
            return;
        }
        if let Some(st) = self.settings.as_mut() {
            let hit = st.hit(self.cursor.0, self.cursor.1, &l, &self.fonts, self.ws.groups.len());
            let hover = Some(hit);
            if hover != st.hover {
                st.hover = hover;
                let pointer = matches!(
                    hit,
                    settings::Hit::Close
                        | settings::Hit::Section(_)
                        | settings::Hit::Font(_)
                        | settings::Hit::SizeDown
                        | settings::Hit::SizeUp
                        | settings::Hit::PickStartDir
                        | settings::Hit::ShowClaude
                        | settings::Hit::ShowMeters
                        | settings::Hit::LazyRestore
                        | settings::Hit::ScrollbackDown
                        | settings::Hit::ScrollbackUp
                ) || (hit == settings::Hit::ClearStartDir && self.config.start_dir.is_some());
                self.window.set_cursor(if pointer { CursorIcon::Pointer } else { CursorIcon::Default });
                self.window.request_redraw();
            }
            return;
        }
        if let Some(menu) = self.menu.as_mut() {
            let s = self.window.scale_factor() as f32;
            let hover = menu.item_at(self.cursor.0, self.cursor.1, s);
            let on_x = menu.remove_at(self.cursor.0, self.cursor.1, s).is_some();
            if hover != menu.hover || on_x != menu.hover_remove {
                menu.hover = hover;
                menu.hover_remove = on_x;
                self.window.set_cursor(if hover.is_some() { CursorIcon::Pointer } else { CursorIcon::Default });
                self.window.request_redraw();
            }
            return;
        }
        let n_tabs = self.ws.group().map_or(0, |g| g.tabs.len());
        let hit = l.hit(self.cursor.0, self.cursor.1, self.ws.groups.len(), n_tabs);
        // Sobre un enlace que se abriría con un clic: mano y resaltado.
        let link_cell = if hit == Hit::Terminal && !self.selecting && self.mouse_button.is_none() {
            let (col, row, _) = l.cell_at(self.cursor.0, self.cursor.1);
            self.clickable_link(col, row).map(|_| (col, row))
        } else {
            None
        };
        if link_cell.is_some() != self.link_cell.is_some() {
            self.window.set_cursor(if link_cell.is_some() { CursorIcon::Pointer } else { CursorIcon::Text });
        }
        if link_cell != self.link_cell {
            self.link_cell = link_cell;
            self.window.request_redraw();
        }
        if hit != self.hover {
            self.hover = hit;
            let icon = match hit {
                Hit::Terminal if self.link_cell.is_some() => CursorIcon::Pointer,
                Hit::Terminal => CursorIcon::Text,
                Hit::Group(_)
                | Hit::CloseGroup(_)
                | Hit::ClaudeAccount
                | Hit::NewGroup
                | Hit::Tab(_)
                | Hit::CloseTab(_)
                | Hit::NewTab
                | Hit::Awake
                | Hit::Update
                | Hit::Settings
                | Hit::Agents
                | Hit::Stats => {
                    CursorIcon::Pointer
                }
                _ => CursorIcon::Default,
            };
            self.window.set_cursor(icon);
            self.window.request_redraw();
        }

        if self.selecting || self.mouse_button.is_some() {
            let (col, row, side) = l.cell_at(self.cursor.0, self.cursor.1);
            let mode = self.active_mode();
            if let Some(button) = self.mouse_button {
                if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) {
                    self.mouse_report(button + 32, true, col, row, mode);
                }
                return;
            }
            if let Some(t) = self.active() {
                let mut term = t.term.lock();
                let point = ui::grid_point(&term, col, row);
                if let Some(sel) = term.selection.as_mut() {
                    sel.update(point, side);
                }
                drop(term);
                self.window.request_redraw();
            }
        }
    }

    pub(crate) fn on_mouse(&mut self, state: ElementState, button: MouseButton) {
        let l = self.layout();
        let n_tabs = self.ws.group().map_or(0, |g| g.tabs.len());
        let hit = l.hit(self.cursor.0, self.cursor.1, self.ws.groups.len(), n_tabs);
        let pressed = state == ElementState::Pressed;

        // Confirmación abierta: solo sus botones; un clic fuera la cancela.
        if let Some(c) = &self.confirm {
            if pressed && button == MouseButton::Left {
                match c.hit(self.cursor.0, self.cursor.1, &l, &mut self.fonts) {
                    confirm::Hit::Confirm => self.run_confirm(),
                    confirm::Hit::Cancel | confirm::Hit::Outside => self.confirm = None,
                    confirm::Hit::Inside => {}
                }
                self.on_cursor_moved(PhysicalPosition::new(self.cursor.0 as f64, self.cursor.1 as f64));
                self.window.request_redraw();
            }
            return;
        }

        // Bandeja abierta: sus filas y botones; un clic fuera la cierra.
        if self.inbox.is_some() {
            if pressed && button == MouseButton::Left {
                self.inbox_click();
            }
            return;
        }

        // Detalle de uso abierto: un clic fuera o en la ✕ lo cierra.
        if let Some(d) = &self.stats_detail {
            if pressed && button == MouseButton::Left && d.hit(self.cursor.0, self.cursor.1, &l) != statsview::Hit::Inside {
                self.toggle_stats();
            }
            return;
        }

        // Configuración abierta: los clics son del modal.
        if self.settings.is_some() {
            if pressed && button == MouseButton::Left {
                self.settings_click();
            }
            return;
        }

        // Menú abierto: un clic elige una opción o lo cierra; no llega a lo de abajo.
        if let Some(menu) = &self.menu {
            if pressed {
                // La ✕ de un ítem tiene su propia acción.
                let item = menu.item_at(self.cursor.0, self.cursor.1, l.s());
                let action = menu.remove_at(self.cursor.0, self.cursor.1, l.s()).or_else(|| item.and_then(|i| menu.items[i].action));
                match action {
                    Some(action) if button == MouseButton::Left => self.run_menu(action),
                    _ => self.menu = None,
                }
                self.on_cursor_moved(PhysicalPosition::new(self.cursor.0 as f64, self.cursor.1 as f64));
                self.window.request_redraw();
            }
            return;
        }

        if !pressed {
            if button == MouseButton::Left {
                self.drag_end();
            }
            if let Some(b) = self.mouse_button.take() {
                let (col, row, _) = l.cell_at(self.cursor.0, self.cursor.1);
                self.mouse_report(b, false, col, row, self.active_mode());
            }
            self.selecting = false;
            if let Some((link, from)) = self.link_press.take().filter(|_| button == MouseButton::Left) {
                self.release_on_link(&l, link, from);
            }
            return;
        }

        // macOS mueve la ventana al arrastrar en la barra de título, donde están los tabs:
        // solo se permite desde sus zonas vacías, así arrastrar un tab lo reordena.
        if button == MouseButton::Left {
            appicon::set_window_movable(&self.window, hit == Hit::Drag);
        }

        // Conteo de clics para doble/triple clic.
        let now = Instant::now();
        let count = if self.clicks.1 == hit && now - self.clicks.0 < DOUBLE_CLICK { self.clicks.2 % 3 + 1 } else { 1 };
        self.clicks = (now, hit, count);

        // Buscador: un clic fuera de él y de sus resultados lo cierra.
        if self.search.is_some() && !matches!(hit, Hit::Search | Hit::Group(_)) {
            self.close_search();
        }
        // Con resultados a la vista, las filas de la lista son resultados.
        if self.searching() {
            if let Hit::Group(i) = hit {
                match button {
                    MouseButton::Left => self.go_to_result(i),
                    MouseButton::Right => {
                        if let Some(f) = self.search_results().get(i).copied() {
                            let id = f.tab.and_then(|ti| self.ws.groups[f.group].tabs.get(ti).copied());
                            self.open_menu(id, f.group);
                        }
                    }
                    _ => {}
                }
                self.window.request_redraw();
                return;
            }
        }

        if hit != Hit::Terminal && button == MouseButton::Left {
            let edited = match (hit, self.editing.as_ref().map(|(e, _)| *e)) {
                (Hit::Group(i), Some(Edit::Group(g))) => i == g,
                (Hit::Tab(i), Some(Edit::Tab(id))) => self.ws.group().and_then(|g| g.tabs.get(i)) == Some(&id),
                _ => false,
            };
            if !edited {
                self.editing = None;
            }
        }

        match (hit, button) {
            (Hit::Group(i), MouseButton::Left) => {
                if count >= 2 {
                    self.start_rename(i);
                } else {
                    if self.ws.active != i {
                        self.ws.select_group(i);
                        self.after_switch();
                    }
                    self.drag = Some(Dragging { item: Dragged::Group(i), from: self.cursor, moved: false, drop_group: None });
                }
            }
            (Hit::NewGroup, MouseButton::Left) => {
                self.new_group();
            }
            (Hit::Tab(i), MouseButton::Left) => {
                let id = self.ws.group().and_then(|g| g.tabs.get(i).copied());
                if count >= 2 {
                    if let Some(id) = id {
                        self.start_rename_tab(id);
                    }
                } else {
                    self.ws.select_tab(i);
                    self.after_switch();
                    if let Some(id) = id {
                        self.drag = Some(Dragging { item: Dragged::Tab(id), from: self.cursor, moved: false, drop_group: None });
                    }
                }
            }
            (Hit::Tab(i) | Hit::CloseTab(i), MouseButton::Right) => {
                if let Some(id) = self.ws.group().and_then(|g| g.tabs.get(i).copied()) {
                    self.open_menu(Some(id), self.ws.active);
                }
            }
            (Hit::Group(i) | Hit::CloseGroup(i), MouseButton::Right) => self.open_menu(None, i),
            (Hit::CloseGroup(i), MouseButton::Left) => self.ask_delete_group(i),
            (Hit::ClaudeAccount, MouseButton::Left) => self.open_account_menu(),
            (Hit::Search, MouseButton::Left) => self.open_search(),
            (Hit::Tab(i), MouseButton::Middle) | (Hit::CloseTab(i), MouseButton::Left) => {
                if let Some(id) = self.ws.group().and_then(|g| g.tabs.get(i).copied()) {
                    self.close_term(id);
                    self.resize_terms();
                }
            }
            (Hit::NewTab, MouseButton::Left) => {
                self.new_tab();
            }
            (Hit::Awake, MouseButton::Left) => self.cycle_awake(),
            (Hit::Update, MouseButton::Left) => self.update_action(),
            (Hit::Settings, MouseButton::Left) => self.toggle_settings(),
            (Hit::Agents, MouseButton::Left) => self.toggle_inbox(),
            (Hit::Stats, MouseButton::Left) => self.toggle_stats(),
            (Hit::Drag, MouseButton::Left) => {
                if count == 2 {
                    let max = self.window.is_maximized();
                    self.window.set_maximized(!max);
                } else {
                    let _ = self.window.drag_window();
                }
            }
            (Hit::Terminal, _) => self.on_terminal_press(button, count),
            _ => {}
        }
        self.window.request_redraw();
    }

    /// Arrastrando: el grupo o tab se mueve en vivo al lugar bajo el puntero. Un tab
    /// llevado a la barra lateral marca el grupo donde va a quedar.
    pub(crate) fn drag_move(&mut self) {
        let Some(mut d) = self.drag.take() else { return };
        let l = self.layout();
        let (x, y) = self.cursor;
        if !d.moved {
            let (dx, dy) = (x - d.from.0, y - d.from.1);
            let min = 5.0 * l.s();
            if dx * dx + dy * dy < min * min {
                self.drag = Some(d);
                return;
            }
            d.moved = true;
            self.editing = None;
            self.window.set_cursor(CursorIcon::Grabbing);
        }
        match d.item {
            Dragged::Group(from) => {
                let n = self.ws.groups.len();
                let to = l.row_at(y).clamp(0, n as isize - 1) as usize;
                if self.ws.move_group(from, to) {
                    d.item = Dragged::Group(to);
                    self.reveal_row(to);
                }
            }
            Dragged::Tab(id) => {
                let src = self.ws.group_of(id);
                if x < l.sidebar_w() {
                    let row = l.row_at(y);
                    let row_ok = row >= 0 && l.row_visible(row as usize) && (row as usize) < self.ws.groups.len();
                    d.drop_group = (row_ok && Some(row as usize) != src).then_some(row as usize);
                } else {
                    d.drop_group = None;
                    if let Some(g) = src.filter(|g| *g == self.ws.active) {
                        let tabs = &self.ws.groups[g].tabs;
                        if let Some(from) = tabs.iter().position(|t| *t == id) {
                            let to = l.tab_at(x, tabs.len());
                            self.ws.move_tab(g, from, to);
                        }
                    }
                }
            }
        }
        self.drag = Some(d);
        self.window.request_redraw();
    }

    /// Al soltar: un tab sobre otro grupo pasa a ese grupo; lo demás ya quedó en su lugar.
    pub(crate) fn drag_end(&mut self) {
        let Some(d) = self.drag.take() else { return };
        if !d.moved {
            return;
        }
        if let (Dragged::Tab(id), Some(gi)) = (d.item, d.drop_group) {
            if self.ws.move_tab_to_group(id, gi) {
                self.after_switch();
            }
        }
        // Los grupos pasados de su límite se indican por índice: se recalculan.
        self.enforce_limits();
        self.save_session();
        self.hover = Hit::None;
        self.on_cursor_moved(PhysicalPosition::new(self.cursor.0 as f64, self.cursor.1 as f64));
        self.window.request_redraw();
    }

    pub(crate) fn on_terminal_press(&mut self, button: MouseButton, count: u8) {
        let l = self.layout();
        let (col, row, side) = l.cell_at(self.cursor.0, self.cursor.1);
        let mode = self.active_mode();
        let code = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            _ => return,
        };
        // Clic en un enlace: con ⌘ se abre ya; sin ⌘, al soltar, si el puntero casi no se
        // movió (no se arrastró para seleccionar). Vale también si la app pidió el mouse
        // (Claude Code lo hace): el clic le llega igual y además se abre el enlace.
        self.link_press = None;
        if button == MouseButton::Left && count == 1 {
            if let Some(link) = self.clickable_link(col, row) {
                if self.mods.super_key() {
                    links::open(&link.url);
                    return;
                }
                self.link_press = Some((link, self.cursor));
            }
        }
        // La app pidió el mouse (vim, htop…): se le reporta. Shift fuerza selección local.
        if mode.intersects(TermMode::MOUSE_MODE) && !self.mods.shift_key() {
            self.mouse_button = Some(code);
            self.mouse_report(code, true, col, row, mode);
            return;
        }
        if button != MouseButton::Left {
            return;
        }
        let Some(t) = self.active() else { return };
        let mut term = t.term.lock();
        let point = ui::grid_point(&term, col, row);
        let ty = match count {
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        term.selection = Some(Selection::new(ty, point, side));
        drop(term);
        self.selecting = true;
    }

    /// Reporte de mouse al estilo xterm (SGR si está activo, si no X10).
    pub(crate) fn mouse_report(&self, button: u8, pressed: bool, col: usize, row: usize, mode: TermMode) {
        let m = self.mods;
        let b = button + 4 * m.shift_key() as u8 + 8 * m.alt_key() as u8 + 16 * m.control_key() as u8;
        let bytes = if mode.contains(TermMode::SGR_MOUSE) {
            format!("\x1b[<{};{};{}{}", b, col + 1, row + 1, if pressed { 'M' } else { 'm' }).into_bytes()
        } else {
            let b = if pressed { b } else { 3 + (b & !3) };
            let clamp = |v: usize| (32 + 1 + v).min(255) as u8;
            vec![0x1b, b'[', b'M', 32 + b, clamp(col), clamp(row)]
        };
        if let Some(id) = self.ws.active_term() {
            self.write(id, bytes);
        }
    }

    /// Enlace en la celda (columna, fila) del tab visible que se abre con un clic.
    pub(crate) fn clickable_link(&self, col: usize, row: usize) -> Option<links::Link> {
        self.active().filter(|t| !t.is_suspended())?;
        self.visible_links().iter().find(|k| k.contains(col, row) && links::openable(&k.url)).cloned()
    }

    /// Se soltó el clic que empezó en `link` (con el puntero en `from`): si el puntero casi
    /// no se movió y sigue sobre el enlace, se abre. Un temblor dentro de la misma letra
    /// no cuenta como selección.
    pub(crate) fn release_on_link(&mut self, l: &Layout, link: links::Link, from: (f32, f32)) {
        let (col, row, _) = l.cell_at(self.cursor.0, self.cursor.1);
        let moved = (self.cursor.0 - from.0).hypot(self.cursor.1 - from.1);
        if moved > 4.0 * l.s() || !link.contains(col, row) {
            return;
        }
        if let Some(t) = self.active() {
            t.term.lock().selection = None;
        }
        links::open(&link.url);
        self.window.request_redraw();
    }

    pub(crate) fn on_scroll(&mut self, delta: MouseScrollDelta) {
        if self.confirm.is_some() {
            return;
        }
        let l = self.layout();
        // Sobre la barra lateral: desplaza la lista de grupos (o de resultados).
        if self.cursor.0 < l.sidebar_w() && self.settings.is_none() && self.stats_detail.is_none() && self.inbox.is_none() {
            let row = 32.0 * l.s();
            let px = match delta {
                MouseScrollDelta::LineDelta(_, y) => -y * row,
                MouseScrollDelta::PixelDelta(p) => -p.y as f32,
            };
            self.list_scroll_px += px;
            let rows = (self.list_scroll_px / row).trunc();
            if rows != 0.0 {
                self.list_scroll_px -= rows * row;
                let max = self.list_len().saturating_sub(l.list_rows());
                self.list_scroll = (self.list_scroll as isize + rows as isize).clamp(0, max as isize) as usize;
                self.window.request_redraw();
            }
            return;
        }
        if self.inbox.is_some() {
            let rows = match delta {
                MouseScrollDelta::LineDelta(_, y) => -y.round() as isize,
                MouseScrollDelta::PixelDelta(p) => (-p.y as f32 / (40.0 * l.s())).round() as isize,
            };
            let n = inbox::entries(&self.ws, &self.terms).len();
            if let Some(b) = self.inbox.as_mut() {
                b.scroll_by(rows, n, &l);
            }
            self.window.request_redraw();
            return;
        }
        if self.stats_detail.is_some() {
            let rows = match delta {
                MouseScrollDelta::LineDelta(_, y) => -y.round() as isize,
                MouseScrollDelta::PixelDelta(p) => (-p.y as f32 / (28.0 * l.s())).round() as isize,
            };
            let total = self.stats.as_ref().map_or(0, |sm| statsview::rows(sm, &self.ws, &self.terms).len());
            if let Some(d) = self.stats_detail.as_mut() {
                d.scroll_by(rows, total, &l);
            }
            self.window.request_redraw();
            return;
        }
        if let Some(st) = self.settings.as_mut() {
            let rows = match delta {
                MouseScrollDelta::LineDelta(_, y) => -y.round() as isize,
                MouseScrollDelta::PixelDelta(p) => (-p.y as f32 / (26.0 * l.s())).round() as isize,
            };
            if st.section == settings::IDLE_SECTION {
                let n = self.ws.groups.len();
                let max = n.saturating_sub(st.idle_capacity(&l, n));
                st.idle_scroll = (st.idle_scroll as isize + rows).clamp(0, max as isize) as usize;
                self.window.request_redraw();
                return;
            }
            if st.section == settings::PERF_SECTION {
                let n = self.ws.groups.len();
                let max = n.saturating_sub(st.perf_capacity(&l, n));
                st.perf_scroll = (st.perf_scroll as isize + rows).clamp(0, max as isize) as usize;
                self.window.request_redraw();
                return;
            }
            let page = st.page(&l, &self.fonts);
            st.scroll_by(rows, page);
            self.window.request_redraw();
            return;
        }
        if l.hit(self.cursor.0, self.cursor.1, self.ws.groups.len(), 0) != Hit::Terminal {
            return;
        }
        let px = match delta {
            MouseScrollDelta::LineDelta(_, y) => y * l.cell_h,
            MouseScrollDelta::PixelDelta(p) => p.y as f32,
        };
        self.scroll_px += px;
        let lines = (self.scroll_px / l.cell_h).trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_px -= lines as f32 * l.cell_h;

        let mode = self.active_mode();
        let (col, row, _) = l.cell_at(self.cursor.0, self.cursor.1);
        if mode.intersects(TermMode::MOUSE_MODE) {
            let button = if lines > 0 { 64 } else { 65 };
            for _ in 0..lines.unsigned_abs() {
                self.mouse_report(button, true, col, row, mode);
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let seq: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            let bytes = seq.repeat(lines.unsigned_abs() as usize);
            if let Some(id) = self.ws.active_term() {
                self.write(id, bytes);
            }
        } else if let Some(t) = self.active() {
            t.term.lock().scroll_display(Scroll::Delta(lines));
            self.window.request_redraw();
        }
    }
}
