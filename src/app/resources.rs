//! Recursos: muestreo, límites de memoria e inactividad.

use crate::*;

impl State {
    /// Nueva muestra de uso. Solo se repinta si cambia lo que se ve.
    pub(crate) fn on_stats(&mut self, sample: stats::Sample) {
        let shown = format!("{:.0}|{}|{:.0}", sample.cpu, stats::human_bytes(sample.mem), sample.gpu);
        let changed = shown != self.meters_shown;
        self.meters_shown = shown;
        self.stats = Some(sample);
        let over_changed = self.enforce_limits();
        if changed || over_changed || self.stats_detail.is_some() || self.settings.is_some() {
            self.window.request_redraw();
        }
    }

    /// Revisa los límites de memoria con la última muestra. Con suspensión automática,
    /// suspende un tab (el inactivo que más usa) si algo está pasado; uno cada 4 s, para
    /// darle tiempo a la memoria a liberarse antes de decidir otro. Devuelve si cambió
    /// qué está pasado.
    pub(crate) fn enforce_limits(&mut self) -> bool {
        let Some(sample) = &self.stats else { return false };
        let u = limits::usage(sample, &self.ws, &self.terms, self.ws.active_term());
        let group_limits: Vec<u64> = self.ws.groups.iter().map(|g| g.ram_limit_mb).collect();
        let (over_total, over_groups) = limits::over(&u, &group_limits, self.config.ram_limit_mb);
        let changed = over_total != self.over_total || over_groups != self.over_groups;
        let pick = limits::pick(&u, over_total, &over_groups);
        self.over_total = over_total;
        self.over_groups = over_groups;
        self.usage = Some(u);
        let due = self.last_auto_suspend.elapsed() > Duration::from_secs(4);
        if let Some(id) = pick.filter(|_| self.config.auto_suspend && due) {
            let name = self.terms.get(&id).and_then(|t| t.label().map(String::from)).unwrap_or_else(|| "un tab".into());
            let place = self.place_of(id);
            if self.suspend(id) {
                self.last_auto_suspend = Instant::now();
                notify::send(&place, &format!("Se suspendió «{name}» para respetar el límite de memoria"), Some(id));
            }
        }
        changed
    }

    /// Estado de inactividad de cada tab y grupo. Antes, marca como usados el tab visible
    /// y los tabs donde Claude trabaja: un agente trabajando mucho rato no es inactividad,
    /// aunque no toques nada.
    pub(crate) fn idle_state(&self) -> (Vec<idle::Tab>, Vec<idle::Group>) {
        let visible = self.ws.active_term();
        let mut tabs = Vec::new();
        for (gi, g) in self.ws.groups.iter().enumerate() {
            for id in &g.tabs {
                let Some(t) = self.terms.get(id) else { continue };
                let state = t.agent.state;
                if Some(*id) == visible || state == AgentState::Working {
                    t.touch();
                }
                tabs.push(idle::Tab {
                    id: *id,
                    group: gi,
                    idle: t.last_used.get().elapsed(),
                    visible: Some(*id) == visible,
                    suspended: t.is_suspended(),
                    working: state == AgentState::Working,
                    waiting: state == AgentState::NeedsYou,
                    running: !t.is_suspended()
                        && state == AgentState::None
                        && !t.process.as_deref().is_some_and(proc::is_shell),
                });
            }
        }
        let group_idle = idle::group_idle(&tabs, self.ws.groups.len(), self.ws.active);
        let groups = self
            .ws
            .groups
            .iter()
            .zip(group_idle)
            .enumerate()
            .map(|(gi, (g, idle))| idle::Group { idle, open: gi == self.ws.active, exempt: g.idle_exempt })
            .collect();
        (tabs, groups)
    }

    /// Suspende lo que lleva sin usarse más de lo configurado (Configuración › Inactividad).
    pub(crate) fn enforce_idle(&mut self) {
        let (tabs, groups) = self.idle_state();
        let ids = idle::pick(&self.config.idle, &tabs, &groups);
        let mut any = false;
        for id in ids {
            any |= self.suspend(id);
        }
        if any {
            log::info!("suspensión por inactividad");
            self.save_session();
        }
    }

    /// Datos de la sección Inactividad del modal.
    pub(crate) fn idle_view(&self) -> settings::IdleView {
        let (tabs, groups) = self.idle_state();
        settings::IdleView {
            rules: self.config.idle,
            groups: self
                .ws
                .groups
                .iter()
                .zip(groups)
                .enumerate()
                .map(|(gi, (g, ig))| settings::IdleGroup {
                    name: g.name.clone(),
                    exempt: g.idle_exempt,
                    open: ig.open,
                    idle: ig.idle,
                    suspended: tabs.iter().filter(|t| t.group == gi && t.suspended).count(),
                    tabs: g.tabs.len(),
                })
                .collect(),
        }
    }

    /// Cambia algo de Inactividad, lo guarda y aplica.
    pub(crate) fn set_idle(&mut self, f: impl FnOnce(&mut idle::Rules)) {
        f(&mut self.config.idle);
        self.config.save();
        self.enforce_idle();
    }

    /// − / + de un límite: 0 es Telar completo (configuración); 1.. el grupo i-1 (sesión).
    pub(crate) fn step_limit(&mut self, i: usize, delta: i32) {
        if i == 0 {
            self.config.ram_limit_mb = limits::step(self.config.ram_limit_mb, delta);
            self.config.save();
        } else if let Some(g) = self.ws.groups.get_mut(i - 1) {
            g.ram_limit_mb = limits::step(g.ram_limit_mb, delta);
            self.save_session();
        }
        self.update_stats_needed();
        self.enforce_limits();
        self.window.request_redraw();
    }

    /// Datos de la sección Rendimiento del modal.
    pub(crate) fn perf_view(&self) -> settings::Perf {
        let used = |gi: usize| self.usage.as_ref().and_then(|u| u.groups.get(gi).copied()).unwrap_or(0);
        settings::Perf {
            total_limit_mb: self.config.ram_limit_mb,
            total_used: self.stats.as_ref().map_or(0, |s| s.mem),
            over_total: self.over_total,
            auto_suspend: self.config.auto_suspend,
            groups: self
                .ws
                .groups
                .iter()
                .enumerate()
                .map(|(gi, g)| (g.name.clone(), g.ram_limit_mb, used(gi), self.over_groups.get(gi).copied().unwrap_or(false)))
                .collect(),
        }
    }

    /// Clic en los medidores: abre o cierra el detalle de uso.
    pub(crate) fn toggle_stats(&mut self) {
        self.menu = None;
        self.settings = None;
        self.stats_detail = match self.stats_detail {
            Some(_) => None,
            None => Some(statsview::Detail::default()),
        };
        self.window.set_cursor(CursorIcon::Default);
        self.window.request_redraw();
    }

    /// El muestreo de CPU, RAM y GPU hace falta si se ven los medidores (o su detalle, o la
    /// sección Rendimiento con el uso por grupo) o hay un límite de memoria que vigilar; si
    /// no, se detiene.
    pub(crate) fn update_stats_needed(&mut self) {
        let limits = self.config.ram_limit_mb > 0 || self.ws.groups.iter().any(|g| g.ram_limit_mb > 0);
        let perf_open = self.settings.as_ref().is_some_and(|st| st.section == settings::PERF_SECTION);
        let needed = self.config.show_meters || limits || self.stats_detail.is_some() || perf_open;
        let was = self.stats_on.swap(needed, std::sync::atomic::Ordering::Relaxed);
        if was && !needed {
            // Sin medición, nada queda marcado como pasado de su límite.
            self.stats = None;
            self.over_total = false;
            self.over_groups.clear();
        }
    }
}
