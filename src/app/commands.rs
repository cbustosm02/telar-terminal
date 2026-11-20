//! Pedidos de la CLI `telar` (por el socket): tabs, read, send, new, group, suspend…

use crate::*;

impl State {
    pub(crate) fn on_ipc(&mut self, body: &Value) -> Value {
        // Pedidos que vienen de Telar web (el bridge los marca): en modo «solo ver», nada
        // que cambie algo en el Mac.
        if body["remote"] == json!(true) && self.config.remote_view_only {
            let acts = matches!(
                body["cmd"].as_str(),
                Some("send" | "suspend" | "resume" | "remote-new" | "remote-close" | "remote-delete-group" | "remote-paste-file" | "remote-paste" | "remote-answer")
            );
            if acts {
                return json!({ "ok": false, "code": "view-only", "error": "Este Mac solo deja mirar desde Telar web (modo «Solo ver»)" });
            }
        }
        match body["cmd"].as_str() {
            Some("hook") => {
                if let Some(tab) = body["tab"].as_u64() {
                    self.on_hook(tab, &body["payload"]);
                }
                self.remote.state_dirty = true;
                json!({ "ok": true })
            }
            Some("tabs") => self.tabs_json(),
            Some("browser") => self.ipc_browser(body),
            Some(cmd @ ("remote-attach" | "remote-release" | "remote-answer" | "remote-new" | "remote-close" | "remote-delete-group" | "remote-paste-file" | "remote-paste" | "remote-sync")) => {
                self.ipc_remote(cmd, body)
            }
            Some(cmd @ ("remote-status" | "remote-control" | "remote-info" | "remote-viewers" | "remote-login" | "remote-logout" | "remote-notice")) => {
                self.ipc_remote_admin(cmd, body)
            }
            Some("status") => match self.resolve_tab(&body["tab"]) {
                Ok(id) => json!({ "ok": true, "tab": self.tab_json(id) }),
                Err(e) => json!({ "ok": false, "error": e }),
            },
            Some("read") => match self.resolve_tab(&body["tab"]) {
                Ok(id) => {
                    let lines = body["lines"].as_u64().unwrap_or(50).clamp(1, 100_000) as usize;
                    json!({ "ok": true, "id": id, "text": self.terms[&id].text(lines) })
                }
                Err(e) => json!({ "ok": false, "error": e }),
            },
            Some("send") => match self.resolve_tab(&body["tab"]) {
                Ok(id) if self.terms[&id].is_suspended() => {
                    json!({ "ok": false, "error": format!("el tab {id} está suspendido; reanúdalo con `telar resume {id}`") })
                }
                Ok(id) => {
                    let text = body["text"].as_str().unwrap_or_default();
                    let t = &self.terms[&id];
                    // Texto de varias líneas: como pegado, para que los saltos no se tomen como Enter.
                    let bytes = if text.contains('\n') && body["raw"] != json!(true) {
                        input::paste_bytes(text, *t.term.lock().mode())
                    } else {
                        text.as_bytes().to_vec()
                    };
                    t.touch();
                    t.write(bytes);
                    json!({ "ok": true, "id": id })
                }
                Err(e) => json!({ "ok": false, "error": e }),
            },
            Some("new") => self.ipc_new(body),
            Some("group") => self.ipc_group(body),
            Some("clear") => match self.resolve_tab(&body["tab"]) {
                Ok(id) => {
                    self.clear_terminal(id);
                    json!({ "ok": true, "id": id })
                }
                Err(e) => json!({ "ok": false, "error": e }),
            },
            Some("awake") => {
                if let Some(s) = body["mode"].as_str() {
                    let Some(mode) = awake::Mode::parse(s) else {
                        return json!({ "ok": false, "error": format!("modo desconocido «{s}»: usa on, off o auto") });
                    };
                    self.awake.mode = mode;
                    self.update_awake();
                    self.window.request_redraw();
                }
                json!({ "ok": true, "mode": self.awake.mode.name(), "active": self.awake.active() })
            }
            Some(cmd @ ("suspend" | "resume")) => self.ipc_suspend(cmd == "suspend", body),
            other => json!({ "ok": false, "error": format!("comando desconocido: {other:?}") }),
        }
    }

    /// `<tab>`: id, `grupo/n` (n desde 1) o parte del título (sin distinguir mayúsculas).
    pub(crate) fn resolve_tab(&self, spec: &Value) -> Result<u64, String> {
        let exists = |id: u64| self.terms.contains_key(&id).then_some(id).ok_or(format!("no existe el tab {id}"));
        if let Some(id) = spec.as_u64() {
            return exists(id);
        }
        let s = spec.as_str().ok_or("falta <tab>")?.trim();
        if let Ok(id) = s.parse::<u64>() {
            return exists(id);
        }
        let lower = s.to_lowercase();
        if let Some((group, n)) = lower.rsplit_once('/') {
            if let Ok(n) = n.parse::<usize>() {
                let g = self.ws.groups.iter().find(|g| g.name.to_lowercase() == group).ok_or(format!("no existe el grupo «{group}»"))?;
                return g.tabs.get(n.wrapping_sub(1)).copied().ok_or(format!("el grupo «{}» no tiene tab {n}", g.name));
            }
        }
        let matches: Vec<u64> = self
            .terms
            .iter()
            .filter(|(_, t)| [&t.name, &t.title].iter().any(|v| v.as_deref().is_some_and(|v| v.to_lowercase().contains(&lower))))
            .map(|(id, _)| *id)
            .collect();
        match matches.as_slice() {
            [id] => Ok(*id),
            [] => Err(format!("ningún tab coincide con «{s}»")),
            many => Err(format!("«{s}» coincide con varios tabs: {many:?}; usa el id")),
        }
    }

    /// `telar new`: tab nuevo en el grupo indicado (se crea si no existe), en el
    /// grupo del tab que lo pidió, o en el activo. No roba el foco salvo `focus`.
    pub(crate) fn ipc_new(&mut self, body: &Value) -> Value {
        let cwd = body["cwd"].as_str().map(PathBuf::from).filter(|d| d.is_absolute()).or_else(|| self.start_dir());
        let focus = body["focus"].as_bool().unwrap_or(false);
        let Some(id) = self.spawn_term_in(cwd.as_deref()) else {
            return json!({ "ok": false, "error": "no se pudo abrir la shell" });
        };
        let gi = match body["group"].as_str() {
            Some(name) => match self.ws.groups.iter().position(|g| g.name.eq_ignore_ascii_case(name)) {
                Some(gi) => {
                    self.ws.insert_tab(gi, id, focus);
                    gi
                }
                // Grupo nuevo: su carpeta es la del tab con que se crea.
                None => self.ws.push_group(name.to_string(), id, focus, cwd.clone()),
            },
            None => {
                let gi = body["near"].as_u64().and_then(|near| self.ws.group_of(near)).unwrap_or(self.ws.active);
                self.ws.insert_tab(gi, id, focus);
                gi
            }
        };
        if let Some(command) = body["command"].as_str().filter(|c| !c.trim().is_empty()) {
            // La shell todavía arranca: lo escrito queda en el buffer del PTY y se ejecuta al estar lista.
            self.write(id, format!("{command}\r").into_bytes());
        }
        if focus {
            self.after_switch();
        }
        self.window.request_redraw();
        json!({ "ok": true, "id": id, "group": self.ws.groups[gi].name })
    }

    /// `telar group`: muestra o cambia el nombre y la carpeta de un grupo
    /// (el indicado, el del tab que lo pidió o el activo).
    pub(crate) fn ipc_group(&mut self, body: &Value) -> Value {
        let gi = match body["group"].as_str() {
            Some(name) => self.ws.groups.iter().position(|g| g.name.eq_ignore_ascii_case(name)),
            None => body["near"].as_u64().and_then(|near| self.ws.group_of(near)).or(Some(self.ws.active)),
        };
        let Some(g) = gi.and_then(|gi| self.ws.groups.get_mut(gi)) else {
            return json!({ "ok": false, "error": "no existe ese grupo" });
        };
        if let Some(dir) = body["dir"].as_str() {
            if dir.is_empty() {
                g.dir = None;
            } else if Path::new(dir).is_absolute() && Path::new(dir).is_dir() {
                g.dir = Some(PathBuf::from(dir));
            } else {
                return json!({ "ok": false, "error": format!("no es una carpeta: {dir}") });
            }
        }
        if let Some(name) = body["name"].as_str().map(str::trim).filter(|n| !n.is_empty()) {
            g.name = name.to_string();
        }
        let v = json!({ "ok": true, "name": g.name, "dir": g.dir, "tabs": g.tabs.len() });
        self.window.request_redraw();
        v
    }

    /// `telar suspend` / `telar resume`: un tab o un grupo entero.
    pub(crate) fn ipc_suspend(&mut self, suspend: bool, body: &Value) -> Value {
        let force = body["force"].as_bool().unwrap_or(false);
        let ids: Vec<u64> = match body["group"].as_str() {
            Some(name) => {
                let Some(gi) = self.ws.groups.iter().position(|g| g.name.eq_ignore_ascii_case(name)) else {
                    return json!({ "ok": false, "error": format!("no existe el grupo «{name}»") });
                };
                if suspend && !force { self.suspendable(gi) } else { self.ws.groups[gi].tabs.clone() }
            }
            None => match self.resolve_tab(&body["tab"]) {
                Ok(id) => {
                    let busy = self.terms[&id].agent.state;
                    if suspend && !force && matches!(busy, AgentState::Working | AgentState::NeedsYou) {
                        return json!({ "ok": false, "code": "busy", "error": format!("Claude está {} en el tab {id}; usa --force para suspenderlo igual", if busy == AgentState::Working { "trabajando" } else { "esperando tu respuesta" }) });
                    }
                    vec![id]
                }
                Err(e) => return json!({ "ok": false, "error": e }),
            },
        };
        let changed: Vec<u64> = ids.into_iter().filter(|id| if suspend { self.suspend(*id) } else { self.resume(*id) }).collect();
        json!({ "ok": true, "changed": changed })
    }

    pub(crate) fn tabs_json(&self) -> Value {
        let active = self.ws.active_term();
        let groups: Vec<Value> = self
            .ws
            .groups
            .iter()
            .map(|g| {
                let tabs: Vec<Value> = g
                    .tabs
                    .iter()
                    .map(|id| {
                        let mut v = self.tab_json(*id);
                        v["active"] = json!(Some(*id) == active);
                        v
                    })
                    .collect();
                json!({ "name": g.name, "dir": g.dir, "tabs": tabs })
            })
            .collect();
        json!({ "ok": true, "groups": groups })
    }

    pub(crate) fn tab_json(&self, id: u64) -> Value {
        let Some(t) = self.terms.get(&id) else { return Value::Null };
        let group = self.ws.group_of(id).map(|gi| self.ws.groups[gi].name.clone());
        json!({
            "id": id,
            "group": group,
            "title": t.title,
            "name": t.name,
            "process": t.process,
            "state": t.agent.state.name(),
            // Comandos de shell que Claude dejó corriendo (servidores, watchers…).
            "claude_shells": t.claude_shells,
            "suspended": t.is_suspended(),
            "state_secs": t.agent.since.elapsed().as_secs(),
            "idle_secs": t.last_used.get().elapsed().as_secs(),
            "changes": t.agent.changes,
            "session_id": t.agent.session_id,
            "cwd": t.cwd().or_else(|| t.agent.cwd.as_ref().map(PathBuf::from)),
            "message": t.agent.message,
        })
    }
}
