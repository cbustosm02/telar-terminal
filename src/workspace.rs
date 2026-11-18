//! Modelo: grupos en la barra lateral; cada grupo tiene sus propios tabs.
//! Solo guarda ids de terminal; las terminales viven en `App::terms`.

pub struct Group {
    pub name: String,
    pub tabs: Vec<u64>,
    pub active: usize,
}

#[derive(Default)]
pub struct Workspace {
    pub groups: Vec<Group>,
    pub active: usize,
    created: usize,
}

impl Workspace {
    pub fn next_group_name(&mut self) -> String {
        self.created += 1;
        format!("Grupo {}", self.created)
    }

    pub fn group(&self) -> Option<&Group> {
        self.groups.get(self.active)
    }

    pub fn group_mut(&mut self) -> Option<&mut Group> {
        self.groups.get_mut(self.active)
    }

    /// Terminal visible: el tab activo del grupo activo.
    pub fn active_term(&self) -> Option<u64> {
        self.group().and_then(|g| g.tabs.get(g.active).copied())
    }

    pub fn add_group(&mut self, first_tab: u64) {
        let name = self.next_group_name();
        self.groups.push(Group { name, tabs: vec![first_tab], active: 0 });
        self.active = self.groups.len() - 1;
    }

    pub fn add_tab(&mut self, id: u64) {
        if let Some(g) = self.group_mut() {
            let at = (g.active + 1).min(g.tabs.len());
            g.tabs.insert(at, id);
            g.active = at;
        }
    }

    /// Quita una terminal de donde esté. Si su grupo queda vacío, lo elimina.
    /// Devuelve `true` si ya no quedan grupos.
    pub fn remove_term(&mut self, id: u64) -> bool {
        let Some(gi) = self.groups.iter().position(|g| g.tabs.contains(&id)) else {
            return self.groups.is_empty();
        };
        let g = &mut self.groups[gi];
        let ti = g.tabs.iter().position(|t| *t == id).unwrap();
        g.tabs.remove(ti);
        if ti < g.active || g.active >= g.tabs.len() {
            g.active = g.active.saturating_sub(1);
        }
        if g.tabs.is_empty() {
            self.groups.remove(gi);
            if gi < self.active || self.active >= self.groups.len() {
                self.active = self.active.saturating_sub(1);
            }
        }
        self.groups.is_empty()
    }

    pub fn select_group(&mut self, i: usize) {
        if i < self.groups.len() {
            self.active = i;
        }
    }

    pub fn cycle_group(&mut self, delta: isize) {
        let n = self.groups.len() as isize;
        if n > 0 {
            self.active = (self.active as isize + delta).rem_euclid(n) as usize;
        }
    }

    pub fn select_tab(&mut self, i: usize) {
        if let Some(g) = self.group_mut() {
            if i < g.tabs.len() {
                g.active = i;
            }
        }
    }

    pub fn cycle_tab(&mut self, delta: isize) {
        if let Some(g) = self.group_mut() {
            let n = g.tabs.len() as isize;
            if n > 0 {
                g.active = (g.active as isize + delta).rem_euclid(n) as usize;
            }
        }
    }
}
