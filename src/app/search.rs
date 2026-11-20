//! Buscador de la barra lateral (⌘P).

use crate::*;

impl State {
    /// Hay texto en el buscador: la lista muestra resultados.
    pub(crate) fn searching(&self) -> bool {
        self.search.as_ref().is_some_and(|(q, _)| !q.trim().is_empty())
    }

    pub(crate) fn search_results(&self) -> Vec<ui::Found> {
        self.search.as_ref().map(|(q, _)| ui::search(&self.ws, &self.terms, q)).unwrap_or_default()
    }

    /// Filas de la lista: grupos + «Nuevo grupo», o los resultados de la búsqueda.
    pub(crate) fn list_len(&self) -> usize {
        if self.searching() { self.search_results().len() } else { self.ws.groups.len() + 1 }
    }

    /// Desplaza la lista para que se vea la fila `i`.
    pub(crate) fn reveal_row(&mut self, i: usize) {
        let rows = self.layout().list_rows();
        if i < self.list_scroll {
            self.list_scroll = i;
        } else if i >= self.list_scroll + rows {
            self.list_scroll = i + 1 - rows;
        }
    }

    pub(crate) fn reveal_active_group(&mut self) {
        if !self.searching() {
            self.reveal_row(self.ws.active);
        }
    }

    /// ⌘P o clic en el buscador.
    pub(crate) fn open_search(&mut self) {
        if self.search.is_none() {
            self.search = Some((String::new(), 0));
        }
        self.editing = None;
        self.menu = None;
        self.window.request_redraw();
    }

    pub(crate) fn close_search(&mut self) {
        self.search = None;
        self.list_scroll = 0;
        self.reveal_active_group();
        self.window.request_redraw();
    }

    /// Va al resultado `i` de la búsqueda (grupo, y tab si lo hay) y cierra el buscador.
    pub(crate) fn go_to_result(&mut self, i: usize) {
        let Some(f) = self.search_results().get(i).copied() else { return };
        self.ws.select_group(f.group);
        if let Some(ti) = f.tab {
            self.ws.select_tab(ti);
        }
        self.close_search();
        self.after_switch();
    }

    /// Teclado con el buscador abierto.
    pub(crate) fn search_key(&mut self, event: &KeyEvent) {
        let n = self.search_results().len();
        let cmd = self.mods.super_key();
        let Some((query, selected)) = self.search.as_mut() else { return };
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => return self.close_search(),
            Key::Named(NamedKey::Enter) => {
                let i = *selected;
                return self.go_to_result(i);
            }
            Key::Named(NamedKey::ArrowDown) if n > 0 => *selected = (*selected + 1).min(n - 1),
            Key::Named(NamedKey::ArrowUp) => *selected = selected.saturating_sub(1),
            Key::Named(NamedKey::Backspace) => {
                query.pop();
                *selected = 0;
            }
            _ if cmd => match event.key_without_modifiers() {
                Key::Character(c) if c.as_str() == "p" => return self.close_search(),
                Key::Character(c) if c.as_str() == "v" => {
                    if let Some(text) = self.clipboard.as_mut().and_then(|cb| cb.get_text().ok()) {
                        if let Some((query, selected)) = self.search.as_mut() {
                            query.extend(text.chars().filter(|c| !c.is_control()));
                            *selected = 0;
                        }
                    }
                }
                _ => {}
            },
            _ => {
                if let Some(t) = &event.text {
                    query.extend(t.chars().filter(|c| !c.is_control()));
                    *selected = 0;
                }
            }
        }
        // La lista muestra los resultados desde arriba y sigue al elegido.
        let sel = self.search.as_ref().map_or(0, |(_, s)| *s);
        if sel == 0 {
            self.list_scroll = 0;
        }
        self.reveal_row(sel);
        self.window.request_redraw();
    }
}
