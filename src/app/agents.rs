//! Sesiones de Claude Code en los tabs: estados según sus hooks, atención y caffeinate.

use crate::*;

impl State {
    /// Hook de un Claude que corre en un servidor (por SSH), llegado por la terminal del
    /// tab. Solo se acepta con ssh en primer plano (si no, cualquier archivo que se muestre
    /// podría inventar estados), y la sesión y la carpeta no se usan para nada local: son
    /// de la otra máquina.
    pub(crate) fn on_inband_hook(&mut self, tab: u64, mut payload: Value) {
        // Lo que corre en primer plano se refresca cada pocos segundos: aquí, en el momento
        // (un `ssh` recién abierto todavía no figura).
        let Some(t) = self.terms.get_mut(&tab) else { return };
        t.refresh_process();
        if !t.is_remote() {
            return;
        }
        // `telar-remote.sh test` (claude-mods): solo confirma que el camino funciona.
        if payload["notification_type"] == "telar_test" {
            let msg = payload["message"].as_str().unwrap_or("Prueba de telar-remote").chars().take(200).collect::<String>();
            notify::send("Telar · Claude por SSH", &format!("Llegó: {msg}"), Some(tab));
            if let Some(t) = self.terms.get_mut(&tab) {
                t.attention = self.ws.active_term() != Some(tab);
            }
            self.window.request_redraw();
            return;
        }
        if let Some(obj) = payload.as_object_mut() {
            obj.remove("session_id");
            obj.remove("cwd");
            obj.retain(|k, _| matches!(k.as_str(), "hook_event_name" | "notification_type" | "message"));
        }
        self.on_hook(tab, &payload);
        self.remote.state_dirty = true;
        self.update_awake();
    }

    /// El tab activo ya se vio: se limpia la campana y «terminó» pasa a «en espera».
    pub(crate) fn mark_seen(&mut self) {
        if let Some(t) = self.ws.active_term().and_then(|id| self.terms.get_mut(&id)) {
            t.attention = false;
            if t.agent.state == AgentState::Done {
                t.agent.set_state(AgentState::Idle);
            }
        }
    }

    /// Qué tan urgente es un tab: 2 = te necesita, 1 = terminó o sonó la campana.
    pub(crate) fn urgency(&self, id: u64) -> u8 {
        match self.terms.get(&id) {
            Some(t) if t.agent.state == AgentState::NeedsYou => 2,
            Some(t) if t.agent.state == AgentState::Done || t.attention => 1,
            _ => 0,
        }
    }

    /// ⌘J: salta al siguiente tab que te necesita; si no hay, al siguiente que terminó.
    pub(crate) fn jump_to_attention(&mut self) {
        let all: Vec<(usize, usize, u64)> = self
            .ws
            .groups
            .iter()
            .enumerate()
            .flat_map(|(gi, g)| g.tabs.iter().enumerate().map(move |(ti, id)| (gi, ti, *id)))
            .collect();
        let current = self.ws.active_term();
        let start = all.iter().position(|(_, _, id)| Some(*id) == current).map_or(0, |p| p + 1);
        let ordered = all.iter().cycle().skip(start).take(all.len());
        let target = ordered
            .clone()
            .find(|(_, _, id)| self.urgency(*id) == 2)
            .or_else(|| ordered.clone().find(|(_, _, id)| self.urgency(*id) == 1));
        if let Some(&(gi, ti, _)) = target {
            self.ws.select_group(gi);
            self.ws.select_tab(ti);
            self.after_switch();
        }
    }

    /// ⌘⇧C o clic abajo en la barra lateral: apagado → siempre → con Claude.
    pub(crate) fn cycle_awake(&mut self) {
        self.awake.mode = self.awake.mode.next();
        self.update_awake();
        self.window.request_redraw();
    }

    pub(crate) fn update_awake(&mut self) {
        let working = self.terms.values().any(|t| t.agent.state == AgentState::Working);
        let before = self.awake.active();
        self.awake.update(working);
        if before != self.awake.active() {
            self.window.request_redraw();
        }
    }

    /// Claude dejó de trabajar sin avisar: lo interrumpiste con Esc (no dispara `Stop`) o
    /// el turno se cortó sin `StopFailure`. Dos señales:
    /// - su título: mientras trabaja empieza con un indicador que gira; en reposo, con «✳».
    ///   Si lleva `TITLE_AT_REST` en reposo, ya no está trabajando.
    /// - su pantalla: mientras trabaja se redibuja varias veces por segundo (el indicador y
    ///   el tiempo corren, también durante una herramienta larga); si lleva `STUCK_AFTER`
    ///   sin mostrar nada, tampoco. (Sola no basta: una barra de estado que se refresca
    ///   sigue cambiando la pantalla después de interrumpirlo.)
    pub(crate) fn unstick_working(&mut self) -> bool {
        let visible = self.ws.active_term().filter(|_| self.focused);
        let mut changed = false;
        for (id, t) in self.terms.iter_mut() {
            let resting_title = t.title.as_deref().is_some_and(claude_title_at_rest) && t.title_at.elapsed() > TITLE_AT_REST;
            let silent = t.output_at.get().elapsed() > STUCK_AFTER && t.agent.since.elapsed() > STUCK_AFTER;
            let stuck = t.agent.state == AgentState::Working
                && !t.is_suspended()
                && t.agent.since.elapsed() > TITLE_AT_REST
                && (resting_title || silent);
            if stuck {
                log::info!("el tab {id} figuraba trabajando sin mostrar nada: pasa a terminado");
                t.agent.set_state(if visible == Some(*id) { AgentState::Idle } else { AgentState::Done });
                changed = true;
            }
        }
        changed
    }

    /// Evento de un hook de Claude Code en el tab `tab`.
    pub(crate) fn on_hook(&mut self, tab: u64, p: &Value) {
        // Un tab suspendido no tiene Claude: lo que llegue es del proceso que se cerró (SessionEnd).
        if self.terms.get(&tab).is_none_or(Terminal::is_suspended) {
            return;
        }
        if p["hook_event_name"] == "SessionStart" {
            if let Some(cwd) = p["cwd"].as_str() {
                self.adopt_group_dir(tab, Path::new(cwd));
            }
        }
        let visible = self.ws.active_term() == Some(tab) && self.focused;
        let Some(t) = self.terms.get_mut(&tab) else { return };
        let agent = &mut t.agent;
        if let Some(sid) = p["session_id"].as_str() {
            agent.session_id = Some(sid.to_string());
        }
        if let Some(cwd) = p["cwd"].as_str() {
            agent.cwd = Some(cwd.to_string());
        }
        let message = p["message"].as_str().unwrap_or_default();
        let new = match p["hook_event_name"].as_str().unwrap_or_default() {
            "SessionStart" => Some(AgentState::Idle),
            "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "PostToolUseFailure" => Some(AgentState::Working),
            "Notification" => notification_needs_you(p["notification_type"].as_str(), message).then_some(AgentState::NeedsYou),
            // `StopFailure`: el turno murió por un error de la API o por el límite de uso.
            "Stop" | "StopFailure" => Some(if visible { AgentState::Idle } else { AgentState::Done }),
            "SessionEnd" => Some(AgentState::None),
            _ => None,
        };
        let Some(new) = new else { return };
        // Al terminar una respuesta cambió el consumo: se consulta el uso de la cuenta activa
        // (con al menos 20 s entre consultas).
        if new == AgentState::Idle || new == AgentState::Done {
            if self.usage_fetched.is_none_or(|t| t.elapsed() > Duration::from_secs(20)) {
                self.fetch_usage(false);
            }
        }
        let Some(t) = self.terms.get_mut(&tab) else { return };
        let agent = &mut t.agent;
        if new == AgentState::NeedsYou {
            agent.message = Some(message.to_string());
        } else if new != AgentState::Working {
            agent.message = None;
        }
        if !agent.set_state(new) {
            return;
        }
        if matches!(new, AgentState::NeedsYou | AgentState::Done) && !self.focused && self.snapshot.is_none() {
            let what = if new == AgentState::NeedsYou {
                if message.is_empty() { "Claude te necesita".to_string() } else { message.to_string() }
            } else {
                "Claude terminó".to_string()
            };
            let place = self.place_of(tab);
            notify::send(&place, &what, Some(tab));
            self.window.request_user_attention(Some(UserAttentionType::Informational));
        }
        self.window.request_redraw();
    }

    /// «Grupo › título del tab», para las notificaciones.
    pub(crate) fn place_of(&self, tab: u64) -> String {
        let group = self.ws.groups.iter().find(|g| g.tabs.contains(&tab)).map_or("", |g| g.name.as_str());
        let title = self.terms.get(&tab).and_then(|t| t.title.clone()).unwrap_or_default();
        if title.is_empty() { group.to_string() } else { format!("{group} › {title}") }
    }

    /// ⌘⇧J o el botón de la bandeja: abre o cierra el panel de agentes. Al abrirlo queda
    /// elegida la primera sesión (la más urgente).
    pub(crate) fn toggle_inbox(&mut self) {
        self.menu = None;
        self.settings = None;
        self.stats_detail = None;
        self.editing = None;
        self.inbox = match self.inbox {
            Some(_) => None,
            None => Some(inbox::Inbox::default()),
        };
        self.window.set_cursor(CursorIcon::Default);
        self.window.request_redraw();
    }

    /// Responde un permiso pendiente con la opción `digit` (como apretar esa tecla en el tab).
    fn answer(&mut self, tab: u64, digit: char) {
        if let Some(t) = self.terms.get(&tab).filter(|t| !t.is_suspended() && t.agent.state == AgentState::NeedsYou) {
            t.touch();
            t.write(digit.to_string().into_bytes());
        }
        self.window.request_redraw();
    }

    /// Va al tab de la sesión y cierra la bandeja.
    fn inbox_go(&mut self, tab: u64) {
        self.inbox = None;
        self.show_tab(tab);
    }

    pub(crate) fn inbox_click(&mut self) {
        let l = self.layout();
        let entries = inbox::entries(&self.ws, &self.terms);
        let Some(b) = self.inbox.as_mut() else { return };
        match b.hit(self.cursor.0, self.cursor.1, &l, &mut self.fonts, &entries) {
            inbox::Hit::Outside | inbox::Hit::Close => self.inbox = None,
            inbox::Hit::Row(i) | inbox::Hit::Go(i) => {
                if let Some(e) = entries.get(i) {
                    self.inbox_go(e.tab);
                }
            }
            inbox::Hit::Option(i, digit) => {
                b.selected = i;
                if let Some(e) = entries.get(i) {
                    self.answer(e.tab, digit);
                }
            }
            inbox::Hit::Inside => {}
        }
        self.window.set_cursor(CursorIcon::Default);
        self.window.request_redraw();
    }

    /// Teclado con la bandeja abierta: ↑/↓ eligen, Enter va al tab, 1–9 responden el
    /// permiso de la sesión elegida, Esc (o ⌘⇧J, ⌘W) cierra.
    pub(crate) fn inbox_key(&mut self, event: &KeyEvent) {
        let l = self.layout();
        let entries = inbox::entries(&self.ws, &self.terms);
        let cmd = self.mods.super_key();
        let Some(b) = self.inbox.as_mut() else { return };
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => self.inbox = None,
            Key::Named(NamedKey::ArrowDown) => b.selected += 1,
            Key::Named(NamedKey::ArrowUp) => b.selected = b.selected.saturating_sub(1),
            Key::Named(NamedKey::Enter) => {
                if let Some(e) = entries.get(b.selected) {
                    let tab = e.tab;
                    self.inbox_go(tab);
                }
            }
            _ if cmd => {
                if matches!(event.key_without_modifiers(), Key::Character(ref c) if c.as_str() == "w" || c.as_str() == "j") {
                    self.inbox = None;
                }
            }
            _ => {
                let digit = event.text.as_ref().and_then(|t| t.chars().next()).filter(|c| c.is_ascii_digit());
                if let (Some(d), Some(e)) = (digit, entries.get(b.selected)) {
                    if e.options.iter().any(|(o, _)| *o == d) {
                        let tab = e.tab;
                        self.answer(tab, d);
                    }
                }
            }
        }
        if let Some(b) = self.inbox.as_mut() {
            b.clamp(entries.len(), &l);
        }
        self.window.request_redraw();
    }
}

/// Si un aviso de Claude Code (hook `Notification`) significa que la sesión espera algo de
/// ti. Solo los pedidos de permiso o de respuesta: los demás (`idle_prompt` a los 60 s,
/// `push_notification` con el resumen al terminar, `agent_completed` de un agente en
/// segundo plano, `auth_success`…) son informativos y no cambian el estado.
/// Tiempo con el título en reposo tras el cual Claude ya no está trabajando.
const TITLE_AT_REST: Duration = Duration::from_secs(4);

/// Título de Claude Code en reposo («✳ Tarea»); trabajando empieza con un indicador que gira.
pub(crate) fn claude_title_at_rest(title: &str) -> bool {
    title.trim_start().starts_with('✳')
}

pub(crate) fn notification_needs_you(kind: Option<&str>, message: &str) -> bool {
    match kind {
        Some(k) => matches!(k, "permission_prompt" | "worker_permission_prompt" | "elicitation_dialog" | "agent_needs_input"),
        // Versiones viejas sin tipo: todo menos el aviso de inactividad.
        None => !message.contains("waiting for your input"),
    }
}

#[cfg(test)]
mod tests {
    use super::notification_needs_you as needs;

    #[test]
    fn titulo_en_reposo() {
        use super::claude_title_at_rest as rest;
        assert!(rest("✳ BCM"));
        assert!(!rest("◑ Terceros"));
        assert!(!rest("⠐ Terceros"));
        assert!(!rest("zsh"));
    }

    #[test]
    fn avisos_que_piden_algo() {
        assert!(needs(Some("permission_prompt"), "Claude needs your permission to use Bash"));
        assert!(needs(Some("elicitation_dialog"), ""));
        // El resumen que manda Claude al terminar no es un pedido.
        assert!(!needs(Some("push_notification"), "Hice las dos cosas"));
        assert!(!needs(Some("idle_prompt"), "Claude is waiting for your input"));
        assert!(!needs(Some("agent_completed"), "x finished"));
        assert!(!needs(Some("auth_success"), ""));
        assert!(!needs(Some("tipo_nuevo"), ""));
        assert!(needs(None, "Claude needs your permission to use Bash"));
        assert!(!needs(None, "Claude is waiting for your input"));
    }
}
