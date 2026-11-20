//! Modo captura (`TELAR_SNAPSHOT`): el escenario y lo que se abre encima para la foto.

use crate::*;

impl State {
    /// Escenario del modo snapshot: varios grupos y tabs, y el texto de prueba.
    pub(crate) fn snapshot_setup(&mut self) {
        self.ws.groups[0].name = "telar".into();
        self.new_tab();
        self.new_group();
        self.ws.groups[1].name = "pirani-portal".into();
        self.new_group();
        self.ws.select_group(0);
        self.ws.select_tab(1);
        if let Ok(text) = std::env::var("TELAR_SNAPSHOT_INPUT") {
            let text = text.replace("\\n", "\r");
            self.send_input(text.into_bytes());
        }
    }

    /// Modo snapshot: lo que se abre encima para la captura (también con una sesión cargada).
    /// TELAR_SNAPSHOT_HOVER=x,y: el puntero en ese punto (en puntos).
    pub(crate) fn snapshot_hover(&mut self) {
        if let Some((x, y)) = std::env::var("TELAR_SNAPSHOT_HOVER").ok().and_then(|v| {
            let (x, y) = v.split_once(',')?;
            Some((x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?))
        }) {
            let s = self.window.scale_factor();
            self.on_cursor_moved(PhysicalPosition::new(x * s, y * s));
        }
    }

    pub(crate) fn snapshot_overlays(&mut self) {
        // TELAR_SNAPSHOT_SEARCH=texto: captura con el buscador abierto.
        if let Some(q) = std::env::var("TELAR_SNAPSHOT_SEARCH").ok().filter(|q| !q.is_empty()) {
            self.search = Some((q, 0));
        }
        // TELAR_SNAPSHOT_STATS=1: captura con el detalle de uso abierto.
        if std::env::var_os("TELAR_SNAPSHOT_STATS").is_some() {
            self.toggle_stats();
        }
        // TELAR_SNAPSHOT_USAGE='<json>': uso de Claude de prueba (como `telar usage`), con
        // una cuenta ficticia.
        if let Some(v) = std::env::var("TELAR_SNAPSHOT_USAGE").ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            self.account_key = Some("persona@ejemplo.com|".into());
            self.claude_use.insert("persona@ejemplo.com|".into(), claude_usage::parse(&v));
            self.claude_account = Some(claude_usage::Account { email: Some("persona@ejemplo.com".into()), plan: Some("Max".into()) });
        }
        // TELAR_SNAPSHOT_INBOX=1: la bandeja de agentes, con estados de prueba: el tab
        // visible pide un permiso (su pantalla, de TELAR_SNAPSHOT_INPUT) y los demás trabajan
        // o terminaron.
        if std::env::var_os("TELAR_SNAPSHOT_INBOX").is_some() {
            let active = self.ws.active_term();
            let states = [AgentState::Working, AgentState::Done, AgentState::Idle];
            let mut n = 0;
            for (id, t) in self.terms.iter_mut() {
                let state = if Some(*id) == active {
                    t.agent.message = Some("Claude needs your permission to use Bash".into());
                    AgentState::NeedsYou
                } else {
                    n += 1;
                    states[(n - 1) % states.len()]
                };
                t.agent.set_state(state);
                t.title = Some(["Revisar el login", "Pruebas del portal", "Refactor de cuentas"][n % 3].into());
            }
            self.toggle_inbox();
        }
        // TELAR_SNAPSHOT_UPDATE=X.Y.Z: captura con esa versión disponible.
        if let Ok(version) = std::env::var("TELAR_SNAPSHOT_UPDATE") {
            self.update = update::Status::Available(update::Release { version, url: String::new(), sha_url: String::new(), size: 0 });
        }
        // TELAR_SNAPSHOT_DELETE=n: captura con la confirmación de eliminar el grupo n.
        if let Some(gi) = std::env::var("TELAR_SNAPSHOT_DELETE").ok().and_then(|v| v.parse().ok()) {
            self.ask_delete_group(gi);
        }
        self.snapshot_hover();
        // TELAR_SNAPSHOT_WEB=login|conectado|mirando: la zona de Telar web en ese estado.
        if let Ok(mode) = std::env::var("TELAR_SNAPSHOT_WEB") {
            let who = remote::SessionInfo { who: "cesar@ejemplo.dev".into(), server: "https://telar.ejemplo.dev".into() };
            match mode.as_str() {
                "login" => {
                    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    self.remote_login = Some(remote::LoginUi { user_code: Some("K7QX-M2PD".into()), url: String::new(), cancel });
                    self.bridge_status = ("login".into(), String::new());
                }
                _ => {
                    self.remote_session = Some(who);
                    self.config.remote = true;
                    self.bridge_status = ("connected".into(), String::new());
                    if mode == "mirando" {
                        let tab = self.ws.active_term();
                        self.remote_viewers = vec![remote::Viewer { device: "iPhone · Safari".into(), tab }];
                        if let Some(t) = tab.and_then(|id| self.terms.get_mut(&id)) {
                            t.set_remote_size(Some((48, 16)));
                        }
                    }
                }
            }
            if std::env::var_os("TELAR_SNAPSHOT_WEB_MENU").is_some() {
                self.open_web_menu();
            }
        }
        // TELAR_SNAPSHOT_SETTINGS=1: captura con la configuración abierta.
        if let Ok(section) = std::env::var("TELAR_SNAPSHOT_SETTINGS") {
            self.toggle_settings();
            if let Some(st) = self.settings.as_mut() {
                match section.as_str() {
                    "apariencia" => st.section = settings::APPEARANCE_SECTION,
                    "icono" => st.section = settings::ICON_SECTION,
                    "rendimiento" => st.section = settings::PERF_SECTION,
                    "inactividad" => st.section = settings::IDLE_SECTION,
                    _ => {}
                }
            }
        }
        // TELAR_SNAPSHOT_DRAG=group|tab|tab-to-group: arrastre simulado. «group» lleva el
        // primer grupo a la cuarta fila; «tab», el primer tab al final; «tab-to-group»
        // deja el tab visible sobre la tercera fila de la barra lateral, sin soltarlo.
        if let Ok(kind) = std::env::var("TELAR_SNAPSHOT_DRAG") {
            let l = self.layout();
            let s = l.s();
            let row_y = |i: f32| (112.0 + 32.0 * i + 15.0) * s;
            let (item, start, end) = match kind.as_str() {
                "group" => (Some(Dragged::Group(0)), (60.0 * s, row_y(0.0)), (60.0 * s, row_y(3.0))),
                "tab" => {
                    let r = l.tab_rect(0);
                    let n = self.ws.group().map_or(1, |g| g.tabs.len());
                    let last = l.tab_rect(n - 1);
                    let first = self.ws.group().and_then(|g| g.tabs.first().copied());
                    (first.map(Dragged::Tab), (r.x + 10.0, r.y + 10.0), (last.x + last.w - 10.0, last.y + 10.0))
                }
                _ => {
                    let r = l.tab_rect(0);
                    (self.ws.active_term().map(Dragged::Tab), (r.x + 10.0, r.y + 10.0), (60.0 * s, row_y(2.0)))
                }
            };
            if let Some(item) = item {
                if let Dragged::Group(i) = item {
                    self.ws.select_group(i);
                }
                self.cursor = start;
                self.drag = Some(Dragging { item, from: start, moved: false, drop_group: None });
                self.cursor = end;
                self.drag_move();
                if kind != "tab-to-group" {
                    self.drag_end();
                }
            }
        }
        // TELAR_SNAPSHOT_MENU=tab|group: captura con el menú contextual abierto.
        match std::env::var("TELAR_SNAPSHOT_MENU").as_deref() {
            Ok("tab") => {
                let s = self.window.scale_factor() as f32;
                self.cursor = (500.0 * s, 24.0 * s);
                let id = self.ws.active_term();
                self.open_menu(id, self.ws.active);
                if let Some(m) = self.menu.as_mut() {
                    m.hover = Some(1);
                }
            }
            Ok("group") => {
                let s = self.window.scale_factor() as f32;
                self.cursor = (60.0 * s, 125.0 * s);
                self.open_menu(None, 1);
            }
            // Menú de cuentas de Claude, con cuentas ficticias.
            Ok("cuentas") => {
                let fake = |email: &str, org: &str, plan: &str| cuentas::Saved {
                    key: cuentas::key_of(email, Some(org)),
                    email: email.into(),
                    org: None,
                    plan: Some(plan.into()),
                };
                self.accounts = vec![fake("persona@ejemplo.com", "a", "Max"), fake("trabajo@empresa.co", "b", "Team")];
                self.account_key = Some(self.accounts[0].key.clone());
                let s = self.window.scale_factor() as f32;
                let r = self.layout().claude_account_rect();
                self.cursor = (r.x + 40.0 * s, r.y + r.h / 2.0);
                self.open_account_menu();
            }
            _ => {}
        }
    }
}
