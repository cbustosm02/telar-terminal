//! Suspensión por inactividad: un tab que no usas hace un rato (o un grupo entero) se
//! suspende solo y libera su memoria. Se configura en Configuración › Inactividad.
//!
//! Un tab cuenta como usado cuando lo ves, cuando escribes en él o cuando su programa
//! muestra algo (Claude trabajando, un servidor con logs…). Un grupo, cuando se usó
//! cualquiera de sus tabs o es el grupo abierto.
//!
//! Nunca se suspende el tab visible ni uno donde Claude trabaja. Por defecto tampoco uno
//! donde Claude espera tu respuesta ni uno con un comando corriendo (un servidor sin
//! logs, ssh, vim…), porque suspender cierra ese proceso. Los grupos marcados como
//! excluidos no se suspenden nunca.

use std::time::Duration;

/// Pasos del tiempo de inactividad, en minutos (0: nunca).
pub const STEPS_MIN: &[u64] = &[0, 5, 10, 15, 30, 45, 60, 90, 120, 180, 240, 360, 480, 720, 1440];

/// Siguiente o anterior paso de la lista.
pub fn step(current: u64, delta: i32) -> u64 {
    crate::limits::step_in(STEPS_MIN, current, delta)
}

/// «Nunca», «15 min», «2 h», «1 h 30 min».
pub fn label(min: u64) -> String {
    match (min / 60, min % 60) {
        (0, 0) => "Nunca".into(),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// «ahora», «hace 12 min», «hace 3 h», «hace 2 d».
pub fn ago(d: Duration) -> String {
    let m = d.as_secs() / 60;
    match m {
        0 => "ahora".into(),
        m if m < 60 => format!("hace {m} min"),
        m if m < 60 * 24 => format!("hace {} h", m / 60),
        m => format!("hace {} d", m / (60 * 24)),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    /// Minutos sin uso para suspender un tab (0: nunca).
    pub tab_min: u64,
    /// Minutos sin uso para suspender un grupo entero (0: nunca).
    pub group_min: u64,
    /// También tabs donde Claude espera tu respuesta (un permiso o una pregunta).
    pub include_waiting: bool,
    /// También tabs con un comando corriendo (se cierra).
    pub include_running: bool,
}

impl Rules {
    pub fn enabled(&self) -> bool {
        self.tab_min > 0 || self.group_min > 0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    pub id: u64,
    pub group: usize,
    /// Tiempo desde su último uso.
    pub idle: Duration,
    pub visible: bool,
    pub suspended: bool,
    /// Claude trabajando.
    pub working: bool,
    /// Claude esperando tu respuesta.
    pub waiting: bool,
    /// Un comando corriendo en primer plano (no la shell ni Claude).
    pub running: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// Tiempo desde el último uso de cualquiera de sus tabs (cero si está abierto).
    pub idle: Duration,
    pub open: bool,
    /// Excluido de la suspensión por inactividad.
    pub exempt: bool,
}

/// Tiempo sin uso de cada grupo: el del tab usado más recientemente (cero si es el
/// grupo abierto o no tiene tabs).
pub fn group_idle(tabs: &[Tab], n_groups: usize, open: usize) -> Vec<Duration> {
    (0..n_groups)
        .map(|gi| if gi == open { Duration::ZERO } else { tabs.iter().filter(|t| t.group == gi).map(|t| t.idle).min().unwrap_or_default() })
        .collect()
}

/// Tabs a suspender ahora.
pub fn pick(rules: &Rules, tabs: &[Tab], groups: &[Group]) -> Vec<u64> {
    let mins = |m: u64| Duration::from_secs(m * 60);
    tabs.iter()
        .filter(|t| {
            let Some(g) = groups.get(t.group) else { return false };
            let eligible = !t.suspended
                && !t.visible
                && !t.working
                && !g.exempt
                && (rules.include_waiting || !t.waiting)
                && (rules.include_running || !t.running);
            let tab_idle = rules.tab_min > 0 && t.idle >= mins(rules.tab_min);
            let group_idle = rules.group_min > 0 && !g.open && g.idle >= mins(rules.group_min);
            eligible && (tab_idle || group_idle)
        })
        .map(|t| t.id)
        .collect()
}

/// Shells conocidas: con una de ellas en primer plano el tab no tiene un comando corriendo.
pub fn is_shell(process: &str) -> bool {
    matches!(process.trim_start_matches('-'), "zsh" | "bash" | "fish" | "sh" | "dash" | "ksh" | "tcsh" | "csh" | "nu" | "login")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    fn tab(id: u64, group: usize, idle_min: u32) -> Tab {
        Tab { id, group, idle: MIN * idle_min, visible: false, suspended: false, working: false, waiting: false, running: false }
    }

    fn group(idle_min: u32) -> Group {
        Group { idle: MIN * idle_min, open: false, exempt: false }
    }

    #[test]
    fn pasos_y_etiquetas() {
        assert_eq!(step(0, 1), 5);
        assert_eq!(step(60, 1), 90);
        assert_eq!(step(5, -1), 0);
        assert_eq!(step(1440, 1), 1440);
        assert_eq!(step(50, 1), 60, "desde un valor suelto, el paso de arriba");
        assert_eq!(label(0), "Nunca");
        assert_eq!(label(45), "45 min");
        assert_eq!(label(120), "2 h");
        assert_eq!(label(90), "1 h 30 min");
        assert_eq!(ago(Duration::from_secs(30)), "ahora");
        assert_eq!(ago(MIN * 12), "hace 12 min");
        assert_eq!(ago(MIN * 185), "hace 3 h");
        assert_eq!(ago(MIN * 60 * 50), "hace 2 d");
    }

    #[test]
    fn tabs_inactivos() {
        let rules = Rules { tab_min: 30, ..Default::default() };
        let mut tabs = vec![tab(1, 0, 40), tab(2, 0, 10), tab(3, 0, 90), tab(4, 0, 90), tab(5, 0, 90), tab(6, 0, 90)];
        tabs[2].visible = true;
        tabs[3].working = true;
        tabs[4].waiting = true;
        tabs[5].running = true;
        let groups = vec![group(10)];
        assert_eq!(pick(&rules, &tabs, &groups), vec![1], "solo el inactivo sin nada que lo proteja");
        let all = Rules { include_waiting: true, include_running: true, ..rules };
        assert_eq!(pick(&all, &tabs, &groups), vec![1, 5, 6], "nunca el visible ni donde Claude trabaja");
        assert!(pick(&Rules::default(), &tabs, &groups).is_empty(), "desactivado");
        tabs[0].suspended = true;
        assert!(pick(&rules, &tabs, &groups).is_empty(), "ya suspendido");
    }

    #[test]
    fn grupos_inactivos() {
        let rules = Rules { group_min: 60, ..Default::default() };
        let tabs = vec![tab(1, 0, 5), tab(2, 1, 70), tab(3, 1, 120), tab(4, 2, 200)];
        let idle = group_idle(&tabs, 3, 0);
        assert_eq!(idle, vec![Duration::ZERO, MIN * 70, MIN * 200]);
        let mut groups: Vec<Group> = idle.iter().map(|d| Group { idle: *d, open: false, exempt: false }).collect();
        groups[0].open = true;
        assert_eq!(pick(&rules, &tabs, &groups), vec![2, 3, 4], "el grupo entero, aunque un tab se usó hace poco");
        groups[2].exempt = true;
        assert_eq!(pick(&rules, &tabs, &groups), vec![2, 3], "excluido");
        // Con regla de tab y de grupo: el 1 está en el grupo abierto y se usó hace poco.
        let both = Rules { tab_min: 100, group_min: 0, ..rules };
        assert_eq!(pick(&both, &tabs, &groups), vec![3]);
    }

    #[test]
    fn shells() {
        assert!(is_shell("zsh") && is_shell("-zsh") && is_shell("bash"));
        assert!(!is_shell("node") && !is_shell("claude") && !is_shell("ssh"));
    }
}
