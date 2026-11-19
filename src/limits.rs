//! Límites de memoria de Telar completo y de cada grupo.
//!
//! macOS no deja fijar un tope duro de memoria a otros procesos sin privilegios (no hay
//! cgroups y `RLIMIT_RSS` no se aplica). Así que el límite se hace cumplir suspendiendo
//! tabs: al pasarlo, se suspende el tab inactivo que más memoria usa, de a uno por
//! muestra, hasta volver por debajo. Nunca se suspende el tab que estás mirando ni uno
//! donde Claude trabaja o espera tu respuesta. Sin suspensión automática, el límite solo
//! avisa y sirve de referencia para los indicadores.

use std::collections::HashMap;

use crate::stats::{Proc, Sample};
use crate::terminal::{AgentState, Terminal};
use crate::workspace::Workspace;

/// Pasos del límite, en MB (0: sin límite).
pub const PRESETS_MB: &[u64] = &[0, 512, 1024, 2048, 3072, 4096, 6144, 8192, 12288, 16384, 24576, 32768];

/// Por debajo de esto no vale la pena suspender un tab (una shell sola usa ~3 MB).
const MIN_TO_SUSPEND: u64 = 32 * 1024 * 1024;

const MB: u64 = 1024 * 1024;

/// Siguiente (`delta` > 0) o anterior paso de la lista desde `current`. Si `current` no
/// es un paso exacto, va al paso inmediato de arriba o de abajo.
pub fn step(current: u64, delta: i32) -> u64 {
    match PRESETS_MB.iter().position(|&p| p == current) {
        Some(i) => PRESETS_MB[(i as i32 + delta).clamp(0, PRESETS_MB.len() as i32 - 1) as usize],
        None if delta > 0 => PRESETS_MB.iter().copied().find(|&p| p > current).unwrap_or(PRESETS_MB[PRESETS_MB.len() - 1]),
        None => PRESETS_MB.iter().copied().rev().find(|&p| p < current).unwrap_or(0),
    }
}

/// «Sin límite», «512 MB», «4 GB».
pub fn label(mb: u64) -> String {
    match mb {
        0 => "Sin límite".into(),
        m if m < 1024 => format!("{m} MB"),
        m if m % 1024 == 0 => format!("{} GB", m / 1024),
        m => format!("{:.1} GB", m as f64 / 1024.0).replace('.', ","),
    }
}

/// Procesos de un tab: desde su proceso en primer plano se sube hasta la shell y se
/// toma todo lo que cuelga de ella. Solo los que se pueden leer.
pub fn tab_procs<'a>(sample: &'a Sample, t: &Terminal) -> Vec<&'a Proc> {
    let root = t.foreground_pid().and_then(|pid| sample.tab_root(pid));
    root.map(|r| sample.subtree(r)).unwrap_or_default().into_iter().filter(|p| p.readable).collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct TabUse {
    pub id: u64,
    pub group: usize,
    pub mem: u64,
    /// Se puede suspender: no es el visible, no está suspendido y Claude no trabaja
    /// ni espera respuesta.
    pub eligible: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    pub tabs: Vec<TabUse>,
    /// Memoria de cada grupo (suma de sus tabs).
    pub groups: Vec<u64>,
    /// Telar completo (incluye la interfaz).
    pub total: u64,
}

pub fn usage(sample: &Sample, ws: &Workspace, terms: &HashMap<u64, Terminal>, visible: Option<u64>) -> Usage {
    let mut u = Usage { total: sample.mem, ..Default::default() };
    for (gi, g) in ws.groups.iter().enumerate() {
        let mut sum = 0;
        for id in &g.tabs {
            let Some(t) = terms.get(id) else { continue };
            let mem: u64 = tab_procs(sample, t).iter().map(|p| p.mem).sum();
            sum += mem;
            let busy = matches!(t.agent.state, AgentState::Working | AgentState::NeedsYou);
            u.tabs.push(TabUse { id: *id, group: gi, mem, eligible: !t.is_suspended() && !busy && Some(*id) != visible });
        }
        u.groups.push(sum);
    }
    u
}

/// Qué está pasado de su límite: Telar completo y cada grupo.
pub fn over(u: &Usage, group_limits_mb: &[u64], total_limit_mb: u64) -> (bool, Vec<bool>) {
    let total = total_limit_mb > 0 && u.total > total_limit_mb * MB;
    let groups = u.groups.iter().zip(group_limits_mb).map(|(used, lim)| *lim > 0 && *used > lim * MB).collect();
    (total, groups)
}

/// El tab a suspender ahora (uno por vez): entre los de los grupos pasados (o de todos,
/// si lo está Telar completo), el inactivo que más memoria usa.
pub fn pick(u: &Usage, over_total: bool, over_groups: &[bool]) -> Option<u64> {
    u.tabs
        .iter()
        .filter(|t| t.eligible && t.mem >= MIN_TO_SUSPEND)
        .filter(|t| over_total || over_groups.get(t.group).copied().unwrap_or(false))
        .max_by_key(|t| t.mem)
        .map(|t| t.id)
}

/// Nivel del medidor de RAM (0–1), umbrales (ámbar, rojo) y relleno de la barra. Con
/// límite, el 100 % es el límite; sin él, la memoria del Mac (y la barra llena es la mitad).
pub fn ram_meter(mem: u64, machine: u64, limit_mb: u64) -> (f32, f32, f32, f32) {
    if limit_mb > 0 {
        let level = mem as f32 / (limit_mb * MB) as f32;
        (level, 0.75, 0.9, level)
    } else {
        let level = if machine > 0 { mem as f32 / machine as f32 } else { 0.0 };
        (level, 0.25, 0.5, level * 2.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasos_del_limite() {
        assert_eq!(step(0, 1), 512);
        assert_eq!(step(512, 1), 1024);
        assert_eq!(step(4096, -1), 3072);
        assert_eq!(step(0, -1), 0);
        assert_eq!(step(32768, 1), 32768);
        assert_eq!(step(5000, 1), 6144, "desde un valor suelto, el paso de arriba");
        assert_eq!(step(5000, -1), 4096, "y el anterior");
    }

    #[test]
    fn etiquetas() {
        assert_eq!(label(0), "Sin límite");
        assert_eq!(label(512), "512 MB");
        assert_eq!(label(4096), "4 GB");
        assert_eq!(label(1536), "1,5 GB");
    }

    fn tab(id: u64, group: usize, mb: u64, eligible: bool) -> TabUse {
        TabUse { id, group, mem: mb * MB, eligible }
    }

    #[test]
    fn que_esta_pasado() {
        let u = Usage { tabs: vec![], groups: vec![900 * MB, 3000 * MB], total: 4000 * MB };
        assert_eq!(over(&u, &[1024, 2048], 0), (false, vec![false, true]));
        assert_eq!(over(&u, &[0, 0], 3072), (true, vec![false, false]));
    }

    #[test]
    fn a_quien_suspender() {
        let u = Usage {
            tabs: vec![tab(1, 0, 800, true), tab(2, 1, 1200, false), tab(3, 1, 600, true), tab(4, 1, 10, true)],
            groups: vec![800 * MB, 1810 * MB],
            total: 2700 * MB,
        };
        // Grupo 1 pasado: el 2 trabaja (no elegible), el 4 usa poco: se suspende el 3.
        assert_eq!(pick(&u, false, &[false, true]), Some(3));
        // Telar completo pasado: el que más usa entre todos los elegibles.
        assert_eq!(pick(&u, true, &[false, false]), Some(1));
        assert_eq!(pick(&u, false, &[false, false]), None);
    }

    #[test]
    fn medidor_de_ram() {
        let gb = 1024 * MB;
        assert_eq!(ram_meter(gb, 16 * gb, 2048), (0.5, 0.75, 0.9, 0.5));
        let (level, warn, _, fill) = ram_meter(4 * gb, 16 * gb, 0);
        assert_eq!((level, warn, fill), (0.25, 0.25, 0.5));
    }
}
