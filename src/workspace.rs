//! Modelo: grupos en la barra lateral; cada grupo tiene sus propios tabs.
//! Solo guarda ids de terminal; las terminales viven en `App::terms`.

use std::path::PathBuf;

pub struct Group {
    pub name: String,
    pub tabs: Vec<u64>,
    pub active: usize,
    /// Carpeta del proyecto del grupo: ahí se abren sus tabs nuevos.
    pub dir: Option<PathBuf>,
    /// Límite de memoria del grupo en MB (0: sin límite). Ver `limits`.
    pub ram_limit_mb: u64,
    /// Nunca suspenderlo por inactividad. Ver `idle`.
    pub idle_exempt: bool,
}

/// Nombre puesto por Telar («Grupo 3»), no por el usuario.
pub fn is_default_name(name: &str) -> bool {
    name.strip_prefix("Grupo ").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
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
        self.push_group(name, first_tab, true, None);
    }

    /// Agrega un grupo con su primer tab. Devuelve su índice.
    pub fn push_group(&mut self, name: String, first_tab: u64, focus: bool, dir: Option<PathBuf>) -> usize {
        self.groups.push(Group { name, tabs: vec![first_tab], active: 0, dir, ram_limit_mb: 0, idle_exempt: false });
        if focus || self.groups.len() == 1 {
            self.active = self.groups.len() - 1;
        }
        self.groups.len() - 1
    }

    /// Agrega un tab al final del grupo `gi`; con `focus` lo deja visible.
    pub fn insert_tab(&mut self, gi: usize, id: u64, focus: bool) {
        let Some(g) = self.groups.get_mut(gi) else { return };
        g.tabs.push(id);
        if focus {
            g.active = g.tabs.len() - 1;
            self.active = gi;
        }
    }

    pub fn group_of(&self, id: u64) -> Option<usize> {
        self.groups.iter().position(|g| g.tabs.contains(&id))
    }

    /// Restaura grupos guardados (los ids ya son de terminales vivas).
    pub fn restore(&mut self, groups: Vec<Group>, active: usize) {
        self.created = groups.len();
        self.groups = groups;
        self.active = active.min(self.groups.len().saturating_sub(1));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Workspace con grupos de los tabs indicados; queda activo el último grupo.
    fn ws(groups: &[&[u64]]) -> Workspace {
        let mut w = Workspace::default();
        for tabs in groups {
            w.add_group(tabs[0]);
            for id in &tabs[1..] {
                w.add_tab(*id);
            }
        }
        w
    }

    #[test]
    fn grupos_nuevos_con_nombre_por_defecto() {
        let w = ws(&[&[1], &[2]]);
        assert_eq!(w.groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["Grupo 1", "Grupo 2"]);
        assert_eq!(w.active, 1);
        assert_eq!(w.active_term(), Some(2));
    }

    #[test]
    fn nombre_por_defecto() {
        assert!(is_default_name("Grupo 1"));
        assert!(is_default_name("Grupo 42"));
        assert!(!is_default_name("Grupo"));
        assert!(!is_default_name("Grupo "));
        assert!(!is_default_name("Grupo uno"));
        assert!(!is_default_name("telar"));
    }

    #[test]
    fn add_tab_inserta_a_la_derecha_del_activo() {
        let mut w = ws(&[&[1, 2, 3]]);
        w.select_tab(0);
        w.add_tab(9);
        assert_eq!(w.groups[0].tabs, [1, 9, 2, 3]);
        assert_eq!(w.active_term(), Some(9));
    }

    #[test]
    fn quitar_tab_ajusta_el_activo() {
        let mut w = ws(&[&[1, 2, 3]]);
        w.select_tab(2);
        assert!(!w.remove_term(1));
        assert_eq!(w.groups[0].tabs, [2, 3]);
        assert_eq!(w.active_term(), Some(3), "el activo sigue siendo el mismo tab");
        w.remove_term(3);
        assert_eq!(w.active_term(), Some(2));
    }

    #[test]
    fn quitar_el_ultimo_tab_elimina_el_grupo() {
        let mut w = ws(&[&[1], &[2], &[3]]);
        w.select_group(2);
        assert!(!w.remove_term(2));
        assert_eq!(w.groups.len(), 2);
        assert_eq!(w.active_term(), Some(3), "el grupo activo no cambia aunque se borre uno anterior");
        w.remove_term(1);
        assert!(w.remove_term(3), "sin grupos devuelve true");
    }

    #[test]
    fn ciclos_dan_la_vuelta() {
        let mut w = ws(&[&[1, 2], &[3]]);
        w.select_group(0);
        w.select_tab(1);
        w.cycle_tab(1);
        assert_eq!(w.active_term(), Some(1));
        w.cycle_tab(-1);
        assert_eq!(w.active_term(), Some(2));
        w.cycle_group(1);
        assert_eq!(w.active, 1);
        w.cycle_group(1);
        assert_eq!(w.active, 0);
    }

    #[test]
    fn seleccion_fuera_de_rango_no_hace_nada() {
        let mut w = ws(&[&[1, 2]]);
        w.select_tab(7);
        w.select_group(7);
        assert_eq!((w.active, w.groups[0].active), (0, 1));
    }

    #[test]
    fn insert_tab_sin_foco_no_cambia_la_vista() {
        let mut w = ws(&[&[1], &[2]]);
        w.insert_tab(0, 5, false);
        assert_eq!(w.groups[0].tabs, [1, 5]);
        assert_eq!(w.active_term(), Some(2));
        w.insert_tab(0, 6, true);
        assert_eq!(w.active_term(), Some(6));
        assert_eq!(w.group_of(6), Some(0));
        assert_eq!(w.group_of(99), None);
    }

    #[test]
    fn push_group_con_carpeta() {
        let mut w = ws(&[&[1]]);
        let gi = w.push_group("api".into(), 2, false, Some("/tmp".into()));
        assert_eq!(gi, 1);
        assert_eq!(w.active, 0, "sin foco no cambia el grupo activo");
        assert_eq!(w.groups[1].dir.as_deref(), Some(std::path::Path::new("/tmp")));
    }

    #[test]
    fn restore_acota_el_activo_y_sigue_numerando() {
        let mut w = Workspace::default();
        let g = |name: &str, id| Group { name: name.into(), tabs: vec![id], active: 0, dir: None, ram_limit_mb: 0, idle_exempt: false };
        w.restore(vec![g("a", 1), g("b", 2)], 9);
        assert_eq!(w.active, 1);
        w.add_group(3);
        assert_eq!(w.groups[2].name, "Grupo 3");
    }
}
