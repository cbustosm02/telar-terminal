//! Telar remoto (ver `remote`): suscripción de `telar bridge`, pantallas de los tabs que
//! se miran desde el celular, toma de tamaño y respuestas de la bandeja.

use crate::*;

impl State {
    /// `telar bridge` se suscribió: desde ahora recibe el estado y las pantallas.
    pub(crate) fn remote_subscribe(&mut self, tx: std::sync::mpsc::Sender<String>) {
        self.remote.subscribe(tx);
        log::info!("telar remoto: bridge suscrito");
    }

    /// Lo pendiente para el celular. Devuelve cuándo volver a mirar (o `None` si no hay
    /// nadie suscrito: sin bridge no se calcula nada).
    pub(crate) fn remote_tick(&mut self, now: Instant) -> Option<Instant> {
        if !self.remote.active() {
            return None;
        }
        if self.remote.state_due(now) {
            let state = remote::state(&self.ws, &self.terms);
            if !self.remote.send_state(state, now) {
                self.remote_release_all();
                return None;
            }
        }
        let due: Vec<u64> = self
            .remote
            .watches
            .iter()
            .filter(|(_, w)| w.dirty && w.sent.is_none_or(|t| now.duration_since(t) >= remote::SCREEN_EVERY))
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            let Some(t) = self.terms.get(&id) else {
                self.remote.watches.remove(&id);
                continue;
            };
            let w = self.remote.watches.get_mut(&id).unwrap();
            w.seq += 1;
            w.dirty = false;
            w.sent = Some(now);
            let line = remote::screen(id, t, w.seq).to_string();
            if !self.remote.broadcast(&line) {
                self.remote_release_all();
                return None;
            }
        }
        self.remote.next_wake(now)
    }

    /// Un tab que alguien mira mostró algo nuevo.
    pub(crate) fn remote_output(&mut self, id: u64) {
        if let Some(w) = self.remote.watches.get_mut(&id) {
            w.dirty = true;
        }
    }

    /// Se fue el bridge: todos los tabs vuelven al tamaño de la ventana.
    pub(crate) fn remote_release_all(&mut self) {
        let ids: Vec<u64> = self.remote.watches.keys().copied().collect();
        for id in ids {
            self.remote_release(id);
        }
    }

    pub(crate) fn remote_release(&mut self, id: u64) {
        self.remote.watches.remove(&id);
        if let Some(t) = self.terms.get_mut(&id) {
            t.set_remote_size(None);
        }
        self.remote.state_dirty = true;
        self.window.request_redraw();
    }

    /// Alguien empieza a mirar el tab (o cambió su tamaño): toma ese tamaño.
    fn remote_attach(&mut self, tab: u64, cols: usize, rows: usize) -> Result<(), String> {
        if !remote::COLS.contains(&cols) || !remote::ROWS.contains(&rows) {
            return Err(format!("tamaño fuera de rango: {cols}×{rows}"));
        }
        // Un tab que esperaba a mostrarse (reanudación perezosa) arranca ya.
        self.wake_lazy(tab);
        if self.terms.get(&tab).is_none_or(Terminal::is_suspended) {
            return Err(if self.terms.contains_key(&tab) { "el tab está suspendido".into() } else { "el tab ya no existe".into() });
        }
        let t = self.terms.get_mut(&tab).unwrap();
        if t.remote_size != Some((cols, rows)) {
            t.set_remote_size(Some((cols, rows)));
            self.remote.state_dirty = true;
            self.window.request_redraw();
        }
        let w = self.remote.watches.entry(tab).or_insert(remote::Watch { seq: 0, dirty: true, sent: None });
        w.dirty = true;
        Ok(())
    }

    /// `remote-sync`: el conjunto completo de tabs que se miran y su tamaño. Lo que no
    /// está, se suelta. Es idempotente: si se perdió un pedido, el siguiente lo corrige.
    fn remote_sync(&mut self, body: &Value) -> Value {
        let wanted: HashMap<u64, (usize, usize)> = body["tabs"]
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((k.parse().ok()?, (v[0].as_u64()? as usize, v[1].as_u64()? as usize))))
                    .collect()
            })
            .unwrap_or_default();
        let stale: Vec<u64> = self.remote.watches.keys().filter(|id| !wanted.contains_key(id)).copied().collect();
        for id in stale {
            self.remote_release(id);
        }
        let mut errors = serde_json::Map::new();
        for (tab, (cols, rows)) in wanted {
            if let Err(e) = self.remote_attach(tab, cols, rows) {
                errors.insert(tab.to_string(), json!(e));
            }
        }
        json!({ "ok": true, "errors": errors })
    }

    /// Pedidos `remote-*` del bridge.
    pub(crate) fn ipc_remote(&mut self, cmd: &str, body: &Value) -> Value {
        if cmd == "remote-new" {
            return self.remote_new(body);
        }
        if cmd == "remote-sync" {
            return self.remote_sync(body);
        }
        if cmd == "remote-delete-group" {
            let name = body["group"].as_str().unwrap_or_default();
            let Some(gi) = self.ws.groups.iter().position(|g| g.name.eq_ignore_ascii_case(name)) else {
                return json!({ "ok": false, "error": format!("no existe el grupo «{name}»") });
            };
            self.delete_group(gi);
            self.remote.state_dirty = true;
            return json!({ "ok": true });
        }
        let tab = match self.resolve_tab(&body["tab"]) {
            Ok(id) => id,
            Err(e) => return json!({ "ok": false, "error": e }),
        };
        match cmd {
            "remote-attach" => {
                let cols = body["cols"].as_u64().unwrap_or(0) as usize;
                let rows = body["rows"].as_u64().unwrap_or(0) as usize;
                match self.remote_attach(tab, cols, rows) {
                    Ok(()) => json!({ "ok": true, "id": tab }),
                    Err(e) => json!({ "ok": false, "error": e }),
                }
            }
            "remote-release" => {
                self.remote_release(tab);
                json!({ "ok": true, "id": tab })
            }
            "remote-answer" => {
                let Some(digit) = body["digit"].as_str().filter(|d| d.len() == 1 && d.chars().all(|c| c.is_ascii_digit())) else {
                    return json!({ "ok": false, "error": "falta digit" });
                };
                // Solo si la pregunta sigue siendo la misma que vio el celular.
                let current = inbox::entries(&self.ws, &self.terms)
                    .into_iter()
                    .find(|e| e.tab == tab && e.state == AgentState::NeedsYou)
                    .map(|e| remote::question_id(tab, e.detail.as_deref(), &e.options));
                if current.as_deref() != body["question_id"].as_str() {
                    return json!({ "ok": false, "code": "stale-question", "error": "la pregunta ya cambió" });
                }
                let t = &self.terms[&tab];
                t.touch();
                t.write(digit.as_bytes().to_vec());
                json!({ "ok": true, "id": tab })
            }
            "remote-paste" => {
                let text = body["text"].as_str().unwrap_or_default();
                let t = &self.terms[&tab];
                if t.is_suspended() {
                    return json!({ "ok": false, "error": "el tab está suspendido" });
                }
                let mode = *t.term.lock().mode();
                t.touch();
                t.write(input::paste_bytes(text, mode));
                json!({ "ok": true, "id": tab })
            }
            "remote-close" => {
                if self.close_term(tab) {
                    // Era el último tab: Telar sigue con un grupo nuevo en lugar de cerrarse.
                    self.new_group();
                }
                self.resize_terms();
                self.save_session();
                self.remote.state_dirty = true;
                json!({ "ok": true, "id": tab })
            }
            // Una imagen que llegó desde Telar web (ya guardada por el bridge): se pega su
            // ruta; por SSH, se sube primero al servidor (ver `sshpaste`).
            "remote-paste-file" => {
                let Some(path) = body["path"].as_str().map(PathBuf::from).filter(|p| p.is_file()) else {
                    return json!({ "ok": false, "error": "no llegó el archivo" });
                };
                // Solo lo que dejó el bridge (no cualquier archivo del Mac, que por SSH se
                // subiría al servidor).
                let dir = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches/Telar/pegado"));
                let inside = dir.and_then(|d| d.canonicalize().ok()).zip(path.canonicalize().ok()).is_some_and(|(d, p)| p.starts_with(d));
                if !inside {
                    return json!({ "ok": false, "error": "ese archivo no vino de Telar web" });
                }
                if self.terms.get(&tab).is_some_and(Terminal::is_remote) {
                    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
                    return match self.ssh_upload_to(tab, move || std::fs::read(&path).map(|b| (b, ext)).map_err(|e| e.to_string())) {
                        Ok(()) => json!({ "ok": true, "id": tab, "via": "ssh" }),
                        Err(e) => json!({ "ok": false, "error": e }),
                    };
                }
                let t = &self.terms[&tab];
                let mode = *t.term.lock().mode();
                t.touch();
                t.write(input::paste_bytes(&format!("{} ", input::shell_escape(&path.to_string_lossy())), mode));
                json!({ "ok": true, "id": tab })
            }
            _ => json!({ "ok": false, "error": format!("pedido desconocido: {cmd}") }),
        }
    }

    /// Lanza o cierra `telar bridge` según la configuración y las credenciales.
    pub(crate) fn sync_bridge(&mut self) {
        let running = self.bridge.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)));
        let want = self.config.remote && self.snapshot.is_none();
        if !want {
            if let Some(mut c) = self.bridge.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
            self.bridge_status = ("off".into(), String::new());
            return;
        }
        if running {
            return;
        }
        if let Some(mut c) = self.bridge.take() {
            let _ = c.wait();
        }
        // Si se cayó, no se relanza en bucle: como mucho una vez por minuto. Tras un rechazo
        // definitivo del servidor, solo a mano.
        if self.bridge_hold || self.bridge_started.is_some_and(|t| t.elapsed() < Duration::from_secs(60)) {
            return;
        }
        // También cuenta sin sesión: así no se consulta el Llavero cada 5 s.
        self.bridge_started = Some(Instant::now());
        if bridge::Session::load().is_none() {
            self.bridge_status = ("off".into(), "sin sesión".into());
            return;
        }
        let (Ok(exe), Some(server)) = (std::env::current_exe(), self.server.as_ref()) else { return };
        // Su bitácora, para diagnosticar una conexión que falla.
        let log = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Library/Logs/Telar")).and_then(|dir| {
            std::fs::create_dir_all(&dir).ok()?;
            let path = dir.join("bridge.log");
            // Rota al pasar de 1 MB (queda la anterior como bridge.log.1).
            if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1024 * 1024) {
                let _ = std::fs::rename(&path, dir.join("bridge.log.1"));
            }
            std::fs::OpenOptions::new().create(true).append(true).open(path).ok()
        });
        match std::process::Command::new(exe)
            .arg("bridge")
            .env("TELAR_SOCKET", &server.path)
            .env("RUST_LOG", "telar=info")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(log.map_or_else(std::process::Stdio::null, std::process::Stdio::from))
            .spawn()
        {
            Ok(child) => {
                log::info!("telar remoto: bridge lanzado (pid {})", child.id());
                self.bridge = Some(child);
                self.bridge_status = ("connecting".into(), String::new());
            }
            Err(e) => self.bridge_status = ("error".into(), format!("no se pudo lanzar el bridge: {e}")),
        }
    }

    /// Al cerrar Telar: el bridge termina solo al cerrarse el socket, pero por si acaso.
    pub(crate) fn stop_bridge(&mut self) {
        if let Some(mut c) = self.bridge.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// `remote-status` (del bridge), `remote-control` y `remote-info` (de la CLI).
    pub(crate) fn ipc_remote_admin(&mut self, cmd: &str, body: &Value) -> Value {
        match cmd {
            "remote-status" => {
                let status = body["status"].as_str().unwrap_or("error").to_string();
                let detail = body["detail"].as_str().unwrap_or_default().to_string();
                // La sesión se cerró desde la web: se olvida aquí también.
                if status == "revoked" {
                    self.remote_session = None;
                    self.config.remote = false;
                    self.config.save();
                    let _ = bridge::Session::forget();
                    self.stop_bridge();
                }
                // Otro rechazo definitivo (versión, otra conexión de este Mac): no se relanza
                // solo, pero la sesión se conserva (se reintenta al conectar a mano).
                if status == "fatal" {
                    self.bridge_hold = true;
                    self.stop_bridge();
                }
                self.bridge_status = (status, detail);
                self.window.request_redraw();
                json!({ "ok": true })
            }
            // La primera acción de un dispositivo en esta conexión: se avisa en el Mac (por si
            // no fuiste tú, el aviso lo genera el bridge, no el relevo).
            "remote-notice" => {
                let device = body["device"].as_str().unwrap_or("un navegador").chars().take(60).collect::<String>();
                let what = body["action"].as_str().unwrap_or("actuar");
                let tab = body["tab"].as_u64().filter(|id| self.terms.contains_key(id));
                let place = tab.and_then(|id| self.terms.get(&id)).and_then(Terminal::label).map(|l| format!(" en «{l}»")).unwrap_or_default();
                notify::send("Telar web", &format!("{device} empezó a {what}{place}"), tab);
                json!({ "ok": true })
            }
            "remote-viewers" => {
                self.remote_viewers = remote::viewers_from(&body["list"]);
                // Con alguien mirando, el estado se revisa cada segundo; sin nadie, solo por eventos.
                self.remote.poll = !self.remote_viewers.is_empty();
                self.remote.state_dirty = true;
                self.window.request_redraw();
                json!({ "ok": true })
            }
            "remote-login" => {
                if let Some(server) = body["server"].as_str().filter(|s| !s.is_empty()) {
                    self.config.remote_server = Some(server.trim_end_matches('/').to_string());
                    self.config.save();
                }
                self.remote_login_start();
                self.remote_info()
            }
            "remote-logout" => {
                self.remote_logout();
                json!({ "ok": true })
            }
            "remote-control" => {
                if let Some(v) = body["view_only"].as_bool() {
                    self.config.remote_view_only = v;
                    self.config.save();
                }
                if let Some(on) = body["enabled"].as_bool() {
                    self.config.remote = on;
                    self.config.save();
                    // Conectar a mano vuelve a intentar aunque el servidor lo haya rechazado.
                    self.bridge_hold = false;
                    // Un cambio de credenciales (setup) se aplica relanzando.
                    if body["restart"] == json!(true) || !on {
                        self.bridge_started = None;
                        self.stop_bridge();
                    }
                    self.sync_bridge();
                    self.window.request_redraw();
                }
                self.remote_info()
            }
            _ => self.remote_info(),
        }
    }

    pub(crate) fn remote_info(&self) -> Value {
        json!({
            "ok": true,
            "enabled": self.config.remote,
            "view_only": self.config.remote_view_only,
            "who": self.remote_session.as_ref().map(|s| s.who.clone()),
            "server": self.remote_server(),
            "login": self.remote_login.as_ref().map(|l| json!({ "user_code": l.user_code, "url": l.url })),
            "viewers": self.remote_viewers.iter().map(|v| json!({ "device": v.device, "tab": v.tab })).collect::<Vec<_>>(),
            "status": self.bridge_status.0,
            "detail": self.bridge_status.1,
            "watching": self.remote.watches.keys().collect::<Vec<_>>(),
        })
    }

    pub(crate) fn remote_server(&self) -> String {
        self.config.remote_server.clone().unwrap_or_else(|| bridge::DEFAULT_SERVER.to_string())
    }

    /// Al abrir: quién está vinculado (una lectura del Llavero).
    pub(crate) fn load_remote_session(&mut self) {
        self.remote_session = bridge::Session::load().map(|s| remote::SessionInfo { who: s.who(), server: s.server.clone() });
    }

    /// «Iniciar sesión»: pide un código al servidor, abre la página para autorizarlo y
    /// pregunta hasta que lo autorices (en un hilo, sin frenar la app).
    pub(crate) fn remote_login_start(&mut self) {
        if let Some(l) = self.remote_login.take() {
            l.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let generation = self.remote_login_generation.wrapping_add(1);
        self.remote_login_generation = generation;
        self.remote_login = Some(remote::LoginUi { generation, user_code: None, url: String::new(), cancel: cancel.clone() });
        self.bridge_status = ("login".into(), "pidiendo el código…".into());
        let server = self.remote_server();
        let proxy = self.proxy.clone();
        let send = move |ev| {
            let _ = proxy.send_event(UserEvent::RemoteLogin(ev));
        };
        std::thread::spawn(move || {
            let start = match bridge::login_start(&server) {
                Ok(s) => s,
                Err(e) => return send(remote::LoginEvent::Failed(generation, e)),
            };
            // TELAR_REMOTE_NO_BROWSER: para pruebas (la autorización la hace otro).
            if std::env::var_os("TELAR_REMOTE_NO_BROWSER").is_none() {
                let _ = std::process::Command::new("/usr/bin/open").arg(&start.verification_url).status();
            }
            send(remote::LoginEvent::Started { generation, user_code: start.user_code.clone(), url: start.verification_url.clone() });
            let mut interval = start.interval;
            loop {
                std::thread::sleep(interval);
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                if Instant::now() >= start.expires {
                    return send(remote::LoginEvent::Failed(generation, "el código venció: vuelve a iniciar sesión".into()));
                }
                match bridge::login_poll(&server, &start.device_code) {
                    bridge::Poll::Pending => {}
                    bridge::Poll::SlowDown => interval += Duration::from_secs(2),
                    bridge::Poll::Failed(e) => return send(remote::LoginEvent::Failed(generation, e)),
                    bridge::Poll::Done(session) => {
                        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                            return;
                        }
                        let info = remote::SessionInfo { who: session.who(), server: session.server.clone() };
                        return match session.save() {
                            Ok(()) => send(remote::LoginEvent::Done(generation, info)),
                            Err(e) => send(remote::LoginEvent::Failed(generation, e)),
                        };
                    }
                }
            }
        });
        self.window.request_redraw();
    }

    pub(crate) fn remote_login_cancel(&mut self) {
        if let Some(l) = self.remote_login.take() {
            l.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.bridge_status = ("off".into(), String::new());
        self.window.request_redraw();
    }

    /// «Cerrar sesión»: avisa al servidor y borra la sesión (en un hilo: es una petición).
    pub(crate) fn remote_logout(&mut self) {
        self.config.remote = false;
        self.config.save();
        self.stop_bridge();
        self.remote_release_all();
        self.remote_viewers.clear();
        self.remote_session = None;
        self.bridge_status = ("off".into(), String::new());
        // Se borra ya del Llavero; el aviso al servidor va en un hilo (es una petición).
        let session = bridge::Session::load();
        if let Err(e) = bridge::Session::forget() {
            self.bridge_status = ("error".into(), e);
        }
        if let Some(s) = session {
            let proxy = self.proxy.clone();
            std::thread::spawn(move || {
                let result = s.revoke().err();
                let _ = proxy.send_event(UserEvent::RemoteLogin(remote::LoginEvent::LoggedOut(result)));
            });
        }
        self.window.request_redraw();
    }

    pub(crate) fn on_remote_login(&mut self, ev: remote::LoginEvent) {
        // Avisos de un intento anterior (cancelado o reemplazado): no pisan al actual.
        let current = self.remote_login.as_ref().map(|l| l.generation);
        let stale = match &ev {
            remote::LoginEvent::Started { generation, .. } | remote::LoginEvent::Done(generation, _) | remote::LoginEvent::Failed(generation, _) => current != Some(*generation),
            remote::LoginEvent::LoggedOut(_) => false,
        };
        if stale {
            return;
        }
        match ev {
            remote::LoginEvent::Started { user_code, url, .. } => {
                if let Some(l) = self.remote_login.as_mut() {
                    l.user_code = user_code;
                    l.url = url;
                }
                self.bridge_status = ("login".into(), "autorízalo en el navegador".into());
            }
            remote::LoginEvent::Done(_, info) => {
                self.remote_login = None;
                log::info!("telar remoto: sesión iniciada como {}", info.who);
                self.remote_session = Some(info);
                self.config.remote = true;
                self.config.save();
                self.bridge_started = None;
                self.bridge_hold = false;
                self.stop_bridge();
                self.sync_bridge();
            }
            remote::LoginEvent::Failed(_, e) => {
                self.remote_login = None;
                self.bridge_status = ("error".into(), e);
            }
            remote::LoginEvent::LoggedOut(None) => {}
            remote::LoginEvent::LoggedOut(Some(e)) => {
                log::warn!("telar remoto: no se pudo revocar el dispositivo: {e}");
                self.bridge_status = ("error".into(), "Sesión cerrada aquí, pero el servidor no respondió: quita este Mac en Telar web (Mis Macs)".into());
            }
        }
        self.window.request_redraw();
    }

    /// Menú de la zona de Telar web.
    pub(crate) fn open_web_menu(&mut self) {
        let mut items = Vec::new();
        if let Some(l) = &self.remote_login {
            let label = l.user_code.as_ref().map_or("Esperando el navegador…".to_string(), |c| format!("Código {c}: compáralo en el navegador"));
            items.push(MenuItem::owned(icons::LOG_IN, label, Action::WebOpenLogin));
            items.push(MenuItem::new(icons::EXTERNAL_LINK, "Abrir de nuevo la página", &[], Action::WebOpenLogin));
            items.push(MenuItem::new(icons::X, "Cancelar", &[], Action::WebCancel));
        } else if let Some(session) = self.remote_session.clone() {
            let status = match self.bridge_status.0.as_str() {
                "connected" => "conectado",
                "connecting" => "conectando",
                "error" | "fatal" => "con error",
                _ if !self.config.remote => "desconectado",
                _ => "",
            };
            items.push(MenuItem::owned(icons::USER, session.who.clone(), Action::WebOpen).with_detail(status.to_string()));
            if matches!(self.bridge_status.0.as_str(), "error" | "fatal") && !self.bridge_status.1.is_empty() {
                items.push(MenuItem::owned(icons::CIRCLE_ALERT, self.bridge_status.1.clone(), Action::WebOpen));
            }
            items.push(MenuItem::separator());
            // Quién mira, y qué tab (clic: ir a ese tab).
            for v in &self.remote_viewers {
                let tab = v.tab.filter(|id| self.terms.contains_key(id));
                let detail = match tab {
                    Some(id) => self.terms.get(&id).and_then(Terminal::label).map_or_else(|| format!("mira el tab {id}"), |l| format!("mira «{l}»")),
                    None => "sin tab abierto".into(),
                };
                let action = tab.map_or(Action::WebOpen, Action::WebShowTab);
                items.push(MenuItem::owned(icons::SMARTPHONE, v.device.clone(), action).with_detail(detail));
            }
            if !self.remote_viewers.is_empty() {
                items.push(MenuItem::separator());
            }
            items.push(MenuItem::new(icons::EXTERNAL_LINK, "Abrir Telar web", &[], Action::WebOpen));
            let (icon, label) = if self.config.remote { (icons::UNPLUG, "Desconectar este Mac") } else { (icons::PLUG, "Conectar este Mac") };
            items.push(MenuItem::new(icon, label, &[], Action::WebToggle));
            // Desde la web solo se mira (no se escribe, ni se crea, cierra o suspende nada).
            let icon = if self.config.remote_view_only { icons::CHECK } else { icons::EYE };
            items.push(MenuItem::new(icon, "Solo ver desde la web", &[], Action::WebViewOnly).with_detail(if self.config.remote_view_only { "activo".into() } else { String::new() }));
            items.push(MenuItem::new(icons::LOG_OUT, "Cerrar sesión", &[], Action::WebLogout));
        } else {
            items.push(MenuItem::new(icons::LOG_IN, "Iniciar sesión en Telar web", &[], Action::WebLogin));
            if self.bridge_status.0 == "error" && !self.bridge_status.1.is_empty() {
                items.push(MenuItem::owned(icons::CIRCLE_ALERT, self.bridge_status.1.clone(), Action::WebLogin));
            }
            let host = bridge::host(&self.remote_server());
            items.push(MenuItem::owned(icons::EXTERNAL_LINK, format!("Abrir {host}"), Action::WebOpen));
        }
        let l = self.layout();
        self.menu = Some(Menu::with_width(self.cursor.0, self.cursor.1, None, self.ws.active, items, &l, 340.0));
        self.editing = None;
        self.window.set_cursor(CursorIcon::Default);
        self.window.request_redraw();
    }

    /// Tab o grupo nuevo desde Telar web: `{"group": "<nombre>"}` (tab en ese grupo; si no
    /// existe, se crea) o `{"new_group": true, "name"?}`; `kind` es `shell` o `claude`.
    /// Desde la web solo se abre una shell o un agente: nada de comandos arbitrarios.
    fn remote_new(&mut self, body: &Value) -> Value {
        let claude = match body["kind"].as_str().unwrap_or("shell") {
            "shell" => false,
            "claude" => true,
            other => return json!({ "ok": false, "error": format!("tipo desconocido: {other} (usa shell o claude)") }),
        };
        // Nombre de grupo desde la web: sin caracteres de control y de 80 como máximo.
        let name = body["name"]
            .as_str()
            .or(body["group"].as_str())
            .map(|n| n.chars().filter(|c| !c.is_control()).take(80).collect::<String>())
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty());
        let existing = (!body["new_group"].as_bool().unwrap_or(false))
            .then(|| name.as_ref().and_then(|n| self.ws.groups.iter().position(|g| g.name.eq_ignore_ascii_case(n))))
            .flatten();
        // Carpeta: la del grupo; si no tiene, la de inicio.
        let cwd = existing.and_then(|gi| self.ws.groups[gi].dir.clone()).filter(|d| d.is_dir()).or_else(|| self.start_dir());
        let Some(id) = self.spawn_term_in(cwd.as_deref()) else {
            return json!({ "ok": false, "error": "no se pudo abrir la shell" });
        };
        // Sin quitarle el foco a quien esté en el Mac.
        let gi = match existing {
            Some(gi) => {
                self.ws.insert_tab(gi, id, false);
                gi
            }
            None => {
                let name = name.unwrap_or_else(|| self.ws.next_group_name());
                self.ws.push_group(name, id, false, cwd)
            }
        };
        if claude {
            // La shell todavía arranca: lo escrito queda en el buffer del PTY y se ejecuta al estar lista.
            self.write(id, b"claude\r".to_vec());
        }
        self.remote.state_dirty = true;
        self.save_session();
        self.window.request_redraw();
        json!({ "ok": true, "id": id, "group": self.ws.groups[gi].name })
    }
}
