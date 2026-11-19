//! Telar: terminal personal con grupos de tabs en una barra lateral.

mod appicon;
mod awake;
mod cli;
mod config;
mod confirm;
mod font;
mod fontlist;
mod hooks;
mod idle;
mod input;
mod kit;
mod limits;
mod ipc;
mod proc;
mod renderer;
mod screen;
mod session;
mod stats;
mod statsview;
mod settings;
mod terminal;
mod theme;
mod ui;
mod update;
mod workspace;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::Event as TermEvent;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::color::COUNT;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{ClearMode, Handler, Rgb};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::macos::WindowAttributesExtMacOS;
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use serde_json::{Value, json};
use winit::window::{CursorIcon, UserAttentionType, Window, WindowId};

use crate::font::Fonts;
use crate::renderer::{Quad, Renderer};
use crate::terminal::{AgentState, Frozen, Terminal};
use crate::theme::Rgba;
use crate::kit::icons;
use crate::ui::{Action, Edit, Hit, Layout, Menu, MenuItem, View};
use crate::session::{Saved, SavedGroup, SavedTab};
use crate::workspace::{Group, Workspace};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

const SPINNER_TICK: Duration = Duration::from_millis(80);
/// Cada cuánto se refrescan procesos/carpetas y se guarda la sesión (si cambió).
const SAVE_TICK: Duration = Duration::from_secs(5);

pub enum UserEvent {
    /// Evento de la terminal `id`, de la generación indicada (ver `terminal::Listener`).
    Term(u64, u64, TermEvent),
    /// Avance de la búsqueda o instalación de una actualización.
    Update(update::Status),
    /// Uso de recursos (cada 2 s).
    Stats(Box<stats::Sample>),
    Ipc(ipc::Request),
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => {}
        Some("hook") => return ipc::hook_main(),
        Some("install" | "install-hooks") => return finish(hooks::install()),
        Some("uninstall" | "uninstall-hooks") => return finish(hooks::uninstall()),
        Some("-h" | "--help" | "help") => return println!("{}", cli::USAGE),
        Some("-V" | "--version" | "version") => return println!("telar {}", update::VERSION),
        Some(cmd) => return finish(cli::run(cmd, &args[1..])),
    }

    env_logger::init();
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App { proxy: event_loop.create_proxy(), state: None };
    event_loop.run_app(&mut app).expect("event loop");
    // Al salir se suelta el estado y, con él, el socket.
    drop(app);
}

fn finish(result: Result<String, String>) {
    match result {
        Ok(msg) if msg.is_empty() => {}
        Ok(msg) => println!("{msg}"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    state: Option<State>,
}

struct State {
    window: Arc<Window>,
    renderer: Renderer,
    fonts: Fonts,
    font_pt: f32,
    palette: [Rgba; COUNT],
    proxy: EventLoopProxy<UserEvent>,
    terms: HashMap<u64, Terminal>,
    next_id: u64,
    ws: Workspace,
    mods: ModifiersState,
    cursor: (f32, f32),
    hover: Hit,
    selecting: bool,
    mouse_button: Option<u8>,
    scroll_px: f32,
    clicks: (Instant, Hit, u8),
    editing: Option<(Edit, String)>,
    /// Menú contextual abierto (clic derecho en un tab o grupo).
    menu: Option<Menu>,
    preedit: String,
    focused: bool,
    clipboard: Option<arboard::Clipboard>,
    quads: Vec<Quad>,
    fg_quads: Vec<Quad>,
    snapshot: Option<Snapshot>,
    server: Option<ipc::Server>,
    started: Instant,
    next_tick: Instant,
    /// Guardar la sesión en disco (no en modo snapshot, para no pisar la real).
    persist: bool,
    last_saved: Option<Saved>,
    next_save: Instant,
    /// `caffeinate`: mantener el Mac despierto (⌘⇧C).
    awake: awake::Awake,
    config: config::Config,
    /// Modal de configuración abierto (⌘,).
    settings: Option<settings::Settings>,
    /// Última muestra de uso de recursos y el panel de detalle, si está abierto.
    stats: Option<stats::Sample>,
    stats_detail: Option<statsview::Detail>,
    /// Diálogo de confirmación abierto (eliminar un grupo).
    confirm: Option<confirm::Confirm>,
    /// Lo que muestran los medidores: solo se repinta si cambia.
    meters_shown: String,
    /// Primera fila visible de la lista de la barra lateral, y píxeles acumulados del
    /// trackpad hasta completar una fila.
    list_scroll: usize,
    list_scroll_px: f32,
    /// Buscador de la barra lateral abierto (⌘P): texto y resultado elegido.
    search: Option<(String, usize)>,
    /// Límites de memoria: uso por grupo, qué está pasado y la última suspensión automática.
    usage: Option<limits::Usage>,
    over_total: bool,
    over_groups: Vec<bool>,
    last_auto_suspend: Instant,
    /// Grupo o tab que se está arrastrando para reordenarlo.
    drag: Option<Dragging>,
    update: update::Status,
    /// La búsqueda automática al abrir no muestra errores (p. ej. sin conexión).
    update_quiet: bool,
    /// Pedido de salir desde fuera del teclado (p. ej. «Reiniciar» tras actualizar).
    quit: bool,
}

/// Arrastre para reordenar: empieza al apretar sobre un grupo o un tab y cuenta como
/// arrastre recién al moverse unos píxeles (antes es un clic normal).
struct Dragging {
    item: Dragged,
    from: (f32, f32),
    moved: bool,
    /// Tab arrastrado sobre otro grupo de la barra lateral: al soltarlo, pasa ahí.
    drop_group: Option<usize>,
}

#[derive(Clone, Copy)]
enum Dragged {
    /// Índice actual del grupo (cambia mientras se arrastra).
    Group(usize),
    Tab(u64),
}

/// Modo de prueba: `TELAR_SNAPSHOT=archivo.bmp` abre la app, escribe
/// `TELAR_SNAPSHOT_INPUT` (si hay) en la shell, guarda un frame y sale.
struct Snapshot {
    path: std::path::PathBuf,
    at: Instant,
    capture_now: bool,
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Telar")
            .with_inner_size(LogicalSize::new(1200.0, 780.0))
            .with_min_inner_size(LogicalSize::new(520.0, 300.0))
            .with_titlebar_transparent(true)
            .with_fullsize_content_view(true)
            .with_title_hidden(true)
            // Modo captura: la ventana de prueba no le quita el foco a lo que estés usando.
            .with_active(std::env::var_os("TELAR_SNAPSHOT").is_none());
        let window = Arc::new(event_loop.create_window(attrs).expect("ventana"));
        window.set_ime_allowed(true);
        appicon::set_window_movable(&window, false);
        let renderer = pollster::block_on(Renderer::new(window.clone()));
        let scale = window.scale_factor() as f32;
        let config = config::Config::load();
        if !theme::set(&config.theme) {
            theme::set(theme::DEFAULT);
        }
        let mut state = State {
            fonts: Fonts::new(&config.font_family, config.font_size * scale),
            font_pt: config.font_size,
            config,
            settings: None,
            stats: None,
            stats_detail: None,
            confirm: None,
            meters_shown: String::new(),
            list_scroll: 0,
            list_scroll_px: 0.0,
            search: None,
            usage: None,
            over_total: false,
            over_groups: Vec::new(),
            last_auto_suspend: Instant::now(),
            drag: None,
            palette: theme::t().palette(),
            window,
            renderer,
            proxy: self.proxy.clone(),
            terms: HashMap::new(),
            next_id: 1,
            ws: Workspace::default(),
            mods: ModifiersState::empty(),
            cursor: (0.0, 0.0),
            hover: Hit::None,
            selecting: false,
            mouse_button: None,
            scroll_px: 0.0,
            clicks: (Instant::now(), Hit::None, 0),
            editing: None,
            menu: None,
            preedit: String::new(),
            focused: true,
            clipboard: arboard::Clipboard::new().ok(),
            quads: Vec::new(),
            fg_quads: Vec::new(),
            snapshot: std::env::var_os("TELAR_SNAPSHOT").map(|p| Snapshot {
                path: p.into(),
                at: Instant::now()
                    + Duration::from_millis(
                        std::env::var("TELAR_SNAPSHOT_DELAY_MS").ok().and_then(|d| d.parse().ok()).unwrap_or(2500),
                    ),
                capture_now: false,
            }),
            server: match ipc::serve(self.proxy.clone()) {
                Ok(server) => Some(server),
                Err(e) => {
                    log::error!("no se pudo abrir el socket de Telar: {e}");
                    None
                }
            },
            started: Instant::now(),
            next_tick: Instant::now(),
            persist: std::env::var_os("TELAR_SNAPSHOT").is_none() || std::env::var_os("TELAR_SESSION_FILE").is_some(),
            last_saved: None,
            next_save: Instant::now() + SAVE_TICK,
            awake: awake::Awake::default(),
            update: update::Status::Idle,
            update_quiet: true,
            quit: false,
        };
        if let Some(server) = &state.server {
            // Para que el modo snapshot (y quien depure) sepa dónde está el socket.
            log::info!("socket: {}", server.path.display());
            if state.snapshot.is_some() {
                eprintln!("TELAR_SOCKET={}", server.path.display());
            }
        }
        let restored = match state.persist.then(session::load).flatten() {
            Some(saved) => {
                state.restore(saved);
                !state.ws.groups.is_empty()
            }
            None => false,
        };
        if state.ws.groups.is_empty() {
            state.new_group();
        }
        if state.snapshot.is_some() && !restored {
            state.snapshot_setup();
        }
        if state.snapshot.is_some() {
            state.snapshot_overlays();
        }
        state.update_awake();
        stats::spawn(state.proxy.clone());
        if state.snapshot.is_none() {
            // Ícono del paquete (si la app se actualizó desde una versión sin él) y el elegido.
            update::ensure_bundle();
            let _ = appicon::apply(&state.config.app_icon, false);
        }
        if state.snapshot.is_none() {
            update::cleanup_leftovers();
            update::check(state.proxy.clone());
        }
        self.state = Some(state);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let Some(st) = self.state.as_mut() else { return };
        let (id, event) = match event {
            // De un proceso anterior del mismo tab (suspendido o reanudado): se ignora.
            UserEvent::Term(id, generation, _) if st.terms.get(&id).is_none_or(|t| t.generation != generation) => return,
            UserEvent::Term(id, _, event) => (id, event),
            UserEvent::Stats(sample) => {
                st.on_stats(*sample);
                return;
            }
            UserEvent::Update(status) => {
                // La búsqueda automática no molesta con errores; la pedida con clic sí los muestra.
                st.update = match status {
                    update::Status::Error(_) if st.update_quiet => update::Status::Idle,
                    s => s,
                };
                if !matches!(st.update, update::Status::Checking) {
                    st.update_quiet = false;
                }
                st.window.request_redraw();
                return;
            }
            UserEvent::Ipc(req) => {
                let response = st.on_ipc(&req.body);
                let _ = req.reply.send(response);
                // Un hook pudo cambiar si hay algún Claude trabajando.
                st.update_awake();
                return;
            }
        };
        let visible = st.ws.active_term() == Some(id);
        // Lo que muestra una shell (un servidor con logs, un build) cuenta como uso del tab.
        // Lo que muestra Claude no: su interfaz puede redibujarse sola estando quieta. Con
        // Claude cuenta que trabaje (ver `enforce_idle`), que lo veas o que escribas.
        if let (TermEvent::Wakeup, Some(t)) = (&event, st.terms.get(&id)) {
            if t.agent.state == AgentState::None {
                t.touch();
            }
        }
        match event {
            TermEvent::Wakeup if visible => st.window.request_redraw(),
            TermEvent::Bell if !visible => {
                if let Some(t) = st.terms.get_mut(&id) {
                    t.attention = true;
                }
                st.window.request_redraw();
            }
            TermEvent::Title(title) => {
                if let Some(t) = st.terms.get_mut(&id) {
                    t.title = Some(title);
                }
                st.window.request_redraw();
            }
            TermEvent::ResetTitle => {
                if let Some(t) = st.terms.get_mut(&id) {
                    t.title = None;
                }
                st.window.request_redraw();
            }
            TermEvent::ChildExit(_) | TermEvent::Exit => {
                if st.close_term(id) {
                    event_loop.exit();
                }
            }
            TermEvent::PtyWrite(text) => st.write(id, text.into_bytes()),
            TermEvent::ClipboardStore(_, text) => {
                if let Some(cb) = st.clipboard.as_mut() {
                    let _ = cb.set_text(text);
                }
            }
            TermEvent::ClipboardLoad(_, format) => {
                let text = st.clipboard.as_mut().and_then(|cb| cb.get_text().ok()).unwrap_or_default();
                st.write(id, format(&text).into_bytes());
            }
            TermEvent::ColorRequest(index, format) => {
                let color = st.terms.get(&id).and_then(|t| t.term.lock().colors()[index]).unwrap_or_else(|| {
                    let c = st.palette[index];
                    Rgb { r: (c[0] * 255.0) as u8, g: (c[1] * 255.0) as u8, b: (c[2] * 255.0) as u8 }
                });
                st.write(id, format(color).into_bytes());
            }
            TermEvent::TextAreaSizeRequest(format) => {
                if let Some(size) = st.terms.get(&id).map(|t| t.size().window_size()) {
                    st.write(id, format(size).into_bytes());
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(st) = self.state.as_mut() else { return };
        let now = Instant::now();
        if let Some(snap) = st.snapshot.as_mut() {
            if now >= snap.at {
                snap.capture_now = true;
                st.draw();
                event_loop.exit();
                return;
            }
        }
        // Mientras algún agente trabaja, el indicador pulsa: se redibuja a ~12 fps.
        // Si nadie trabaja, la app vuelve a dormir hasta el próximo evento.
        if now >= st.next_save {
            st.periodic();
            st.next_save = now + SAVE_TICK;
        }
        let mut wake = Some(st.snapshot.as_ref().map_or(st.next_save, |s| s.at.min(st.next_save)));
        if st.terms.values().any(|t| t.agent.state == AgentState::Working) {
            if now >= st.next_tick {
                st.window.request_redraw();
                st.next_tick = now + SPINNER_TICK;
            }
            wake = wake.map(|w| w.min(st.next_tick));
        }
        event_loop.set_control_flow(match wake {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // Se guarda antes de cerrar las terminales: así las sesiones de Claude
        // quedan registradas como abiertas y se retoman al volver a abrir.
        if let Some(st) = self.state.as_mut() {
            st.save_session();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(st) = self.state.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                st.renderer.resize(size.width, size.height);
                st.resize_terms();
                st.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => st.apply_font_size(),
            WindowEvent::Focused(focused) => {
                st.focused = focused;
                // Al cambiar de app, el menú se cierra (salvo en modo captura, sin foco).
                if !focused && st.snapshot.is_none() {
                    st.menu = None;
                    st.drag_end();
                }
                if focused {
                    st.mark_seen();
                }
                st.window.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => st.mods = m.state(),
            // En modo captura se ignora el teclado: lo que se escriba es de otra ventana.
            WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_) if st.snapshot.is_some() => {}
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed && st.on_key(&event) {
                    event_loop.exit();
                }
            }
            WindowEvent::Ime(ime) => st.on_ime(ime),
            WindowEvent::CursorMoved { position, .. } => st.on_cursor_moved(position),
            WindowEvent::CursorLeft { .. } => {
                st.hover = Hit::None;
                st.window.request_redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                st.on_mouse(state, button);
                if st.quit {
                    event_loop.exit();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => st.on_scroll(delta),
            WindowEvent::RedrawRequested => st.draw(),
            _ => {}
        }
    }
}

impl State {
    fn layout(&self) -> Layout {
        let size = self.window.inner_size();
        let n_tabs = self.ws.group().map_or(0, |g| g.tabs.len());
        let mut l = Layout::new(size.width as f32, size.height as f32, self.window.scale_factor() as f32, &self.fonts, n_tabs);
        l.list_scroll = self.list_scroll;
        l.new_row = !self.searching();
        l
    }

    /// Abre una terminal nueva en la carpeta del tab activo.
    fn spawn_term(&mut self) -> Option<u64> {
        let cwd = self.active().and_then(|t| t.cwd());
        self.spawn_term_in(cwd.as_deref())
    }

    fn spawn_term_in(&mut self, cwd: Option<&Path>) -> Option<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let t = self.spawn_with_id(id, cwd)?;
        self.terms.insert(id, t);
        Some(id)
    }

    fn spawn_with_id(&self, id: u64, cwd: Option<&Path>) -> Option<Terminal> {
        let socket = self.server.as_ref().map(|s| s.path.as_path());
        Terminal::spawn(id, self.layout().grid_size(), self.proxy.clone(), socket, cwd)
            .map_err(|e| log::error!("no se pudo abrir la shell: {e}"))
            .ok()
    }

    /// Suspende un tab: guarda su pantalla y cierra su shell (y Claude, si corre ahí).
    /// Queda la vista previa; al reanudarlo, Claude retoma la misma sesión.
    fn suspend(&mut self, id: u64) -> bool {
        let Some(old) = self.terms.get_mut(&id) else { return false };
        if old.is_suspended() {
            return false;
        }
        let claude = old.agent.state != AgentState::None && old.agent.session_id.is_some();
        // Con Claude, su carpeta es donde se inició la sesión (la necesita --resume).
        let cwd = if claude { old.agent.cwd.as_ref().map(PathBuf::from).or_else(|| old.cwd()) } else { old.cwd() };
        let screen = screen::to_ansi(&old.term.lock());
        // Otra cosa que la shell o Claude en primer plano (una TUI, un servidor): se
        // vuelve a ejecutar al reanudar, como Claude con --resume.
        let command = if claude { None } else { old.foreground_command() };
        let title = old.title.take();
        let name = old.name.take();
        let mut agent = std::mem::take(&mut old.agent);
        let size = old.size();
        let last_used = old.last_used.get();
        let mut t = Terminal::frozen(id, size, self.proxy.clone(), Frozen { cwd, screen, command });
        t.last_used.set(last_used);
        t.title = title;
        t.name = name;
        if claude {
            agent.set_state(AgentState::Idle);
            agent.message = None;
            t.agent = agent;
        }
        // Al reemplazarla se suelta la terminal vieja: su shell recibe SIGHUP.
        self.terms.insert(id, t);
        self.window.request_redraw();
        true
    }

    /// Vuelve a abrir un tab suspendido en su carpeta; si tenía Claude, retoma la sesión.
    fn resume(&mut self, id: u64) -> bool {
        let Some(old) = self.terms.get(&id) else { return false };
        let Some(frozen) = &old.frozen else { return false };
        let cwd = frozen.cwd.clone();
        let command = frozen.command.clone();
        let session = old.agent.session_id.clone().filter(|_| old.agent.state != AgentState::None);
        let (title, name) = (old.title.clone(), old.name.clone());
        let Some(mut t) = self.spawn_with_id(id, cwd.as_deref()) else { return false };
        t.title = title;
        t.name = name;
        if let Some(session) = session {
            resume_claude(&mut t, &session, cwd.as_deref());
        } else if let Some(command) = command {
            t.write(format!("{command}\r").into_bytes());
        }
        self.terms.insert(id, t);
        self.window.request_redraw();
        true
    }

    /// Tabs del grupo que se pueden suspender sin cortar trabajo: no están trabajando
    /// ni esperando un permiso.
    fn suspendable(&self, gi: usize) -> Vec<u64> {
        let Some(g) = self.ws.groups.get(gi) else { return Vec::new() };
        g.tabs
            .iter()
            .copied()
            .filter(|id| {
                self.terms.get(id).is_some_and(|t| {
                    !t.is_suspended() && !matches!(t.agent.state, AgentState::Working | AgentState::NeedsYou)
                })
            })
            .collect()
    }

    /// ⌘S: suspende o reanuda el tab activo.
    fn toggle_suspend_tab(&mut self) {
        let Some(id) = self.ws.active_term() else { return };
        if self.terms.get(&id).is_some_and(Terminal::is_suspended) {
            self.resume(id);
        } else {
            self.suspend(id);
        }
    }

    /// ⌘⇧S: suspende el grupo activo (salvo los tabs que trabajan); si ya está todo
    /// suspendido, lo reanuda entero.
    fn toggle_suspend_group(&mut self, gi: usize) {
        let awake = self.suspendable(gi);
        if awake.is_empty() {
            let tabs = self.ws.groups.get(gi).map(|g| g.tabs.clone()).unwrap_or_default();
            for id in tabs {
                self.resume(id);
            }
        } else {
            for id in awake {
                self.suspend(id);
            }
        }
    }

    fn new_group(&mut self) -> Option<u64> {
        let id = self.spawn_term()?;
        self.ws.add_group(id);
        self.reveal_active_group();
        self.window.request_redraw();
        Some(id)
    }

    /// Hay texto en el buscador: la lista muestra resultados.
    fn searching(&self) -> bool {
        self.search.as_ref().is_some_and(|(q, _)| !q.trim().is_empty())
    }

    fn search_results(&self) -> Vec<ui::Found> {
        self.search.as_ref().map(|(q, _)| ui::search(&self.ws, &self.terms, q)).unwrap_or_default()
    }

    /// Filas de la lista: grupos + «Nuevo grupo», o los resultados de la búsqueda.
    fn list_len(&self) -> usize {
        if self.searching() { self.search_results().len() } else { self.ws.groups.len() + 1 }
    }

    /// Desplaza la lista para que se vea la fila `i`.
    fn reveal_row(&mut self, i: usize) {
        let rows = self.layout().list_rows();
        if i < self.list_scroll {
            self.list_scroll = i;
        } else if i >= self.list_scroll + rows {
            self.list_scroll = i + 1 - rows;
        }
    }

    fn reveal_active_group(&mut self) {
        if !self.searching() {
            self.reveal_row(self.ws.active);
        }
    }

    /// ⌘P o clic en el buscador.
    fn open_search(&mut self) {
        if self.search.is_none() {
            self.search = Some((String::new(), 0));
        }
        self.editing = None;
        self.menu = None;
        self.window.request_redraw();
    }

    fn close_search(&mut self) {
        self.search = None;
        self.list_scroll = 0;
        self.reveal_active_group();
        self.window.request_redraw();
    }

    /// Va al resultado `i` de la búsqueda (grupo, y tab si lo hay) y cierra el buscador.
    fn go_to_result(&mut self, i: usize) {
        let Some(f) = self.search_results().get(i).copied() else { return };
        self.ws.select_group(f.group);
        if let Some(ti) = f.tab {
            self.ws.select_tab(ti);
        }
        self.close_search();
        self.after_switch();
    }

    /// Teclado con el buscador abierto.
    fn search_key(&mut self, event: &KeyEvent) {
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

    /// Carpeta de un tab nuevo en el grupo activo: la del grupo, o si no tiene, la del tab activo.
    fn new_tab_cwd(&self) -> Option<PathBuf> {
        let dir = self.ws.group().and_then(|g| g.dir.clone()).filter(|d| d.is_dir());
        dir.or_else(|| self.active().and_then(|t| t.cwd()))
    }

    fn new_tab(&mut self) -> Option<u64> {
        if self.ws.groups.is_empty() {
            return self.new_group();
        }
        let cwd = self.new_tab_cwd();
        let id = self.spawn_term_in(cwd.as_deref())?;
        self.ws.add_tab(id);
        self.window.request_redraw();
        Some(id)
    }

    /// ⌘⇧T: tab nuevo que arranca Claude Code (en la carpeta del grupo).
    fn new_agent(&mut self) {
        if let Some(id) = self.new_tab() {
            // La shell todavía arranca: lo escrito queda en el buffer del PTY y se ejecuta al estar lista.
            self.write(id, b"claude\r".to_vec());
        }
    }

    /// La primera sesión de Claude de un grupo sin carpeta le da la suya; si el grupo aún
    /// se llama «Grupo N», también su nombre. Así sus tabs y agentes nuevos abren en ese proyecto.
    fn adopt_group_dir(&mut self, tab: u64, dir: &Path) {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        if home.as_deref() == Some(dir) || !dir.is_dir() {
            return;
        }
        let Some(gi) = self.ws.group_of(tab) else { return };
        let g = &mut self.ws.groups[gi];
        if g.dir.is_some() {
            return;
        }
        g.dir = Some(dir.to_path_buf());
        if workspace::is_default_name(&g.name) {
            if let Some(base) = dir.file_name() {
                g.name = base.to_string_lossy().into_owned();
            }
        }
        self.window.request_redraw();
    }

    /// Cierra una terminal. Devuelve `true` si ya no queda ninguna (hay que salir).
    /// Pide confirmación para eliminar el grupo `gi` con todos sus tabs.
    fn ask_delete_group(&mut self, gi: usize) {
        let Some(g) = self.ws.groups.get(gi) else { return };
        let Some(&anchor) = g.tabs.first() else { return };
        let terms: Vec<&Terminal> = g.tabs.iter().filter_map(|id| self.terms.get(id)).collect();
        let claude = terms.iter().filter(|t| t.agent.session_id.is_some()).count();
        let working = terms.iter().filter(|t| t.agent.state == AgentState::Working).count();
        self.confirm = Some(confirm::delete_group(&g.name, g.tabs.len(), claude, working, anchor));
        self.menu = None;
        self.editing = None;
        self.drag = None;
        self.hover = Hit::None;
        self.window.set_cursor(CursorIcon::Default);
        self.window.request_redraw();
    }

    /// Hace lo que se confirmó en el diálogo y lo cierra.
    fn run_confirm(&mut self) {
        let Some(c) = self.confirm.take() else { return };
        match c.pending {
            confirm::Pending::DeleteGroup { anchor } => {
                if let Some(gi) = self.ws.group_of(anchor) {
                    self.delete_group(gi);
                }
            }
        }
    }

    /// Elimina el grupo `gi` y cierra sus tabs (con lo que corre en ellos).
    fn delete_group(&mut self, gi: usize) {
        for id in self.ws.remove_group(gi) {
            self.terms.remove(&id);
        }
        if self.ws.groups.is_empty() {
            // Era el único: Telar sigue con un grupo nuevo en lugar de cerrarse.
            self.new_group();
        }
        // Los grupos pasados de su límite se indican por índice: se recalculan.
        self.enforce_limits();
        self.after_switch();
        self.save_session();
    }

    fn close_term(&mut self, id: u64) -> bool {
        self.terms.remove(&id);
        self.editing = None;
        let empty = self.ws.remove_term(id);
        self.window.request_redraw();
        empty
    }

    fn write(&self, id: u64, bytes: Vec<u8>) {
        if let Some(t) = self.terms.get(&id) {
            t.write(bytes);
        }
    }

    fn active(&self) -> Option<&Terminal> {
        self.ws.active_term().and_then(|id| self.terms.get(&id))
    }

    fn active_mode(&self) -> TermMode {
        self.active().map_or(TermMode::empty(), |t| *t.term.lock().mode())
    }

    /// Envía bytes al tab activo y baja al final del scrollback.
    fn send_input(&self, bytes: Vec<u8>) {
        if let Some(t) = self.active() {
            {
                let mut term = t.term.lock();
                if term.grid().display_offset() != 0 {
                    term.scroll_display(Scroll::Bottom);
                    self.window.request_redraw();
                }
            }
            t.touch();
            t.write(bytes);
        }
    }

    fn resize_terms(&mut self) {
        let size = self.layout().grid_size();
        for t in self.terms.values_mut() {
            t.resize(size);
        }
    }

    fn apply_font_size(&mut self) {
        self.fonts.set_size(self.font_pt * self.window.scale_factor() as f32);
        self.resize_terms();
        self.window.request_redraw();
    }

    fn copy_selection(&mut self) {
        let text = self.active().and_then(|t| t.term.lock().selection_to_string());
        if let (Some(text), Some(cb)) = (text, self.clipboard.as_mut()) {
            let _ = cb.set_text(text);
        }
    }

    fn paste(&mut self) {
        let Some(text) = self.clipboard.as_mut().and_then(|cb| cb.get_text().ok()) else { return };
        if let Some((_, buf)) = self.editing.as_mut() {
            buf.extend(text.chars().filter(|c| !c.is_control()));
            self.window.request_redraw();
            return;
        }
        let bytes = input::paste_bytes(&text, self.active_mode());
        self.send_input(bytes);
    }

    fn start_rename(&mut self, group: usize) {
        if let Some(g) = self.ws.groups.get(group) {
            self.editing = Some((Edit::Group(group), g.name.clone()));
            self.window.request_redraw();
        }
    }

    /// Renombrar un tab: se edita en la barra de tabs (el tab pasa a estar visible).
    fn start_rename_tab(&mut self, id: u64) {
        self.show_tab(id);
        if let Some(t) = self.terms.get(&id) {
            self.editing = Some((Edit::Tab(id), t.label().unwrap_or_default().to_string()));
            self.window.request_redraw();
        }
    }

    /// Lleva a la vista el tab `id` (su grupo y su posición).
    fn show_tab(&mut self, id: u64) {
        let Some(gi) = self.ws.group_of(id) else { return };
        let ti = self.ws.groups[gi].tabs.iter().position(|t| *t == id).unwrap_or(0);
        self.ws.select_group(gi);
        self.ws.select_tab(ti);
        self.after_switch();
    }

    /// ⌘K: borra la pantalla y el scrollback. Si en primer plano hay una shell o Claude,
    /// se le manda Ctrl-L para que vuelva a dibujar su prompt arriba.
    fn clear_terminal(&mut self, id: u64) {
        let Some(t) = self.terms.get(&id).filter(|t| !t.is_suspended()) else { return };
        {
            let mut term = t.term.lock();
            term.scroll_display(Scroll::Bottom);
            // All lleva la pantalla al scrollback; Saved vacía el scrollback.
            term.clear_screen(ClearMode::All);
            term.clear_screen(ClearMode::Saved);
            term.selection = None;
        }
        let redraw = t.process.as_deref().is_none_or(|p| proc::is_shell(p) || p == "claude");
        if redraw {
            t.write(b"\x0c".to_vec());
        }
        self.window.request_redraw();
    }

    /// Abre el menú contextual de un tab (`tab`) o de un grupo, en la posición del puntero.
    fn open_menu(&mut self, tab: Option<u64>, group: usize) {
        let suspended = |id: &u64| self.terms.get(id).is_some_and(Terminal::is_suspended);
        let group_asleep = self.suspendable(group).is_empty();
        let mut items = Vec::new();
        if let Some(id) = tab {
            if self.ws.active_term() != Some(id) {
                items.push(MenuItem::new(icons::EYE, "Mostrar", &[], Action::Show));
            }
            if suspended(&id) {
                items.push(MenuItem::new(icons::PLAY, "Reanudar", &["↩"], Action::Resume));
            } else {
                items.push(MenuItem::new(icons::MOON, "Suspender", &["⌘", "S"], Action::Suspend));
                items.push(MenuItem::new(icons::ERASER, "Limpiar", &["⌘", "K"], Action::Clear));
            }
            items.push(MenuItem::new(icons::PENCIL, "Renombrar tab", &[], Action::RenameTab));
            items.push(MenuItem::new(icons::X, "Cerrar tab", &["⌘", "W"], Action::CloseTab));
            items.push(MenuItem::separator());
        } else {
            items.push(MenuItem::new(icons::TERMINAL, "Nuevo tab", &["⌘", "T"], Action::NewTab));
            items.push(MenuItem::new(icons::SPARKLES, "Nuevo agente", &["⌘", "⇧", "T"], Action::NewAgent));
            items.push(MenuItem::separator());
        }
        items.push(MenuItem::new(icons::SQUARE_PEN, "Renombrar grupo", &["⌘", "⇧", "R"], Action::RenameGroup));
        if group_asleep {
            items.push(MenuItem::new(icons::PLAY, "Reanudar grupo", &["⌘", "⇧", "S"], Action::ResumeGroup));
        } else {
            items.push(MenuItem::new(icons::MOON, "Suspender grupo", &["⌘", "⇧", "S"], Action::SuspendGroup));
        }
        if self.config.idle.enabled() {
            let exempt = self.ws.groups.get(group).is_some_and(|g| g.idle_exempt);
            let (icon, label) = if exempt { (icons::TIMER, "Permitir suspender solo") } else { (icons::PIN, "Nunca suspender solo") };
            items.push(MenuItem::new(icon, label, &[], Action::ToggleIdleExempt));
        }
        items.push(MenuItem::separator());
        items.push(MenuItem::new(icons::TRASH, "Eliminar grupo", &[], Action::DeleteGroup));
        let l = self.layout();
        self.menu = Some(Menu::new(self.cursor.0, self.cursor.1, tab, group, items, &l));
        self.editing = None;
        self.window.set_cursor(CursorIcon::Default);
        self.window.request_redraw();
    }

    fn run_menu(&mut self, action: Action) {
        let Some(menu) = self.menu.take() else { return };
        let (tab, gi) = (menu.tab, menu.group);
        match (action, tab) {
            (Action::Show, Some(id)) => self.show_tab(id),
            (Action::Suspend, Some(id)) => {
                self.suspend(id);
            }
            (Action::Resume, Some(id)) => {
                self.resume(id);
            }
            (Action::Clear, Some(id)) => self.clear_terminal(id),
            (Action::RenameTab, Some(id)) => self.start_rename_tab(id),
            (Action::CloseTab, Some(id)) => {
                if self.close_term(id) {
                    // Era el último tab: Telar empieza de nuevo con un grupo vacío en lugar de cerrarse.
                    self.new_group();
                }
                self.resize_terms();
            }
            (Action::NewTab | Action::NewAgent, _) => {
                self.ws.select_group(gi);
                if action == Action::NewAgent {
                    self.new_agent();
                } else {
                    self.new_tab();
                }
                self.after_switch();
            }
            (Action::RenameGroup, _) => self.start_rename(gi),
            (Action::SuspendGroup | Action::ResumeGroup, _) => self.toggle_suspend_group(gi),
            (Action::DeleteGroup, _) => self.ask_delete_group(gi),
            (Action::ToggleIdleExempt, _) => {
                if let Some(g) = self.ws.groups.get_mut(gi) {
                    g.idle_exempt = !g.idle_exempt;
                    self.save_session();
                }
            }
            _ => {}
        }
        self.window.request_redraw();
    }

    /// Teclas con el menú abierto: flechas, Enter y Esc. Cualquier otra lo cierra.
    fn menu_key(&mut self, event: &KeyEvent) {
        let Some(menu) = self.menu.as_mut() else { return };
        match &event.logical_key {
            Key::Named(NamedKey::ArrowDown) => menu.step(1),
            Key::Named(NamedKey::ArrowUp) => menu.step(-1),
            Key::Named(NamedKey::Enter) => {
                if let Some(action) = menu.hover.and_then(|i| menu.items[i].action) {
                    self.run_menu(action);
                }
            }
            _ => self.menu = None,
        }
        self.window.request_redraw();
    }

    /// Devuelve `true` si hay que cerrar la app.
    fn on_key(&mut self, event: &KeyEvent) -> bool {
        let m = self.mods;

        if self.confirm.is_some() {
            match event.logical_key {
                Key::Named(NamedKey::Enter) => self.run_confirm(),
                Key::Named(NamedKey::Escape) => self.confirm = None,
                _ => {}
            }
            self.window.request_redraw();
            return false;
        }
        if self.menu.is_some() {
            self.menu_key(event);
            return false;
        }
        if self.settings.is_some() {
            self.settings_key(event);
            return false;
        }
        if self.search.is_some() {
            self.search_key(event);
            return false;
        }
        if self.stats_detail.is_some() {
            let close = event.logical_key == Key::Named(NamedKey::Escape)
                || (self.mods.super_key() && matches!(event.key_without_modifiers(), Key::Character(ref c) if c.as_str() == "w"));
            if close {
                self.toggle_stats();
            }
            return false;
        }

        // Renombrando un grupo o un tab: el teclado edita el nombre.
        if let Some((target, buf)) = self.editing.as_mut() {
            match &event.logical_key {
                Key::Named(NamedKey::Enter) => {
                    let name = buf.trim().to_string();
                    match *target {
                        Edit::Group(gi) => {
                            if let Some(g) = self.ws.groups.get_mut(gi).filter(|_| !name.is_empty()) {
                                g.name = name;
                            }
                        }
                        // Vacío: vuelve el título automático.
                        Edit::Tab(id) => {
                            if let Some(t) = self.terms.get_mut(&id) {
                                t.name = (!name.is_empty()).then_some(name);
                            }
                        }
                    }
                    self.editing = None;
                }
                Key::Named(NamedKey::Escape) => self.editing = None,
                Key::Named(NamedKey::Backspace) => {
                    buf.pop();
                }
                _ if m.super_key() => {
                    if matches!(event.key_without_modifiers(), Key::Character(ref s) if s.as_str() == "v") {
                        self.paste();
                    }
                }
                _ => {
                    if let Some(t) = &event.text {
                        buf.extend(t.chars().filter(|c| !c.is_control()));
                    }
                }
            }
            self.window.request_redraw();
            return false;
        }

        if m.super_key() {
            return self.shortcut(event);
        }
        if m.control_key() && event.logical_key == Key::Named(NamedKey::Tab) {
            self.ws.cycle_tab(if m.shift_key() { -1 } else { 1 });
            self.after_switch();
            return false;
        }

        if let Some(id) = self.ws.active_term().filter(|id| self.terms.get(id).is_some_and(Terminal::is_suspended)) {
            if event.logical_key == Key::Named(NamedKey::Enter) {
                self.resume(id);
            }
            return false;
        }
        if let Some(bytes) = input::key_to_bytes(event, m, self.active_mode()) {
            self.send_input(bytes);
        }
        false
    }

    /// Atajos con ⌘. Devuelve `true` si hay que cerrar la app.
    fn shortcut(&mut self, event: &KeyEvent) -> bool {
        let m = self.mods;
        if let Key::Named(named) = &event.logical_key {
            match (named, m.shift_key()) {
                (NamedKey::ArrowRight, true) => self.ws.cycle_tab(1),
                (NamedKey::ArrowLeft, true) => self.ws.cycle_tab(-1),
                (NamedKey::ArrowDown, true) => self.ws.cycle_group(1),
                (NamedKey::ArrowUp, true) => self.ws.cycle_group(-1),
                _ => return false,
            }
            self.after_switch();
            return false;
        }
        let Key::Character(key) = event.key_without_modifiers() else { return false };
        match key.to_lowercase().as_str() {
            "t" if m.shift_key() => self.new_agent(),
            "t" => {
                self.new_tab();
            }
            "n" => {
                self.new_group();
            }
            "w" => {
                if let Some(id) = self.ws.active_term() {
                    return self.close_term(id);
                }
            }
            "q" => return true,
            "j" => self.jump_to_attention(),
            "c" if m.shift_key() => self.cycle_awake(),
            "c" => self.copy_selection(),
            "v" => self.paste(),
            "r" if m.shift_key() => self.start_rename(self.ws.active),
            "s" if m.shift_key() => self.toggle_suspend_group(self.ws.active),
            "k" => {
                if let Some(id) = self.ws.active_term() {
                    self.clear_terminal(id);
                }
            }
            "s" => self.toggle_suspend_tab(),
            "=" | "+" => {
                self.font_pt = (self.font_pt + 1.0).min(36.0);
                self.apply_font_size();
            }
            "-" => {
                self.font_pt = (self.font_pt - 1.0).max(8.0);
                self.apply_font_size();
            }
            "0" => {
                self.font_pt = self.config.font_size;
                self.apply_font_size();
            }
            "," => self.toggle_settings(),
            "p" => self.open_search(),
            d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() => {
                let n = (d.as_bytes()[0] - b'0') as usize;
                if n > 0 {
                    self.ws.select_tab(n - 1);
                    self.after_switch();
                }
            }
            _ => {}
        }
        false
    }

    /// Al cambiar de tab o grupo: el tamaño puede haber cambiado (nº de tabs) y hay que repintar.
    fn after_switch(&mut self) {
        self.editing = None;
        self.reveal_active_group();
        self.mark_seen();
        self.resize_terms();
        self.window.request_redraw();
    }

    fn on_ime(&mut self, ime: Ime) {
        match ime {
            Ime::Preedit(text, _) => {
                self.preedit = text;
                self.window.request_redraw();
            }
            Ime::Commit(text) => {
                self.preedit.clear();
                if self.confirm.is_some() {
                    // Con la confirmación abierta, nada llega a la terminal.
                } else if let Some((_, buf)) = self.editing.as_mut() {
                    buf.push_str(&text);
                    self.window.request_redraw();
                } else {
                    self.send_input(text.into_bytes());
                }
            }
            Ime::Enabled | Ime::Disabled => {}
        }
    }

    fn on_cursor_moved(&mut self, pos: PhysicalPosition<f64>) {
        self.cursor = (pos.x as f32, pos.y as f32);
        if self.drag.is_some() {
            self.drag_move();
            return;
        }
        let l = self.layout();
        if let Some(c) = self.confirm.as_mut() {
            let hover = Some(c.hit(self.cursor.0, self.cursor.1, &l, &mut self.fonts));
            if hover != c.hover {
                c.hover = hover;
                let pointer = matches!(hover, Some(confirm::Hit::Cancel | confirm::Hit::Confirm));
                self.window.set_cursor(if pointer { CursorIcon::Pointer } else { CursorIcon::Default });
                self.window.request_redraw();
            }
            return;
        }
        if let Some(d) = self.stats_detail.as_mut() {
            let hover = Some(d.hit(self.cursor.0, self.cursor.1, &l));
            if hover != d.hover {
                d.hover = hover;
                self.window.set_cursor(if hover == Some(statsview::Hit::Close) { CursorIcon::Pointer } else { CursorIcon::Default });
                self.window.request_redraw();
            }
            return;
        }
        if let Some(st) = self.settings.as_mut() {
            let hit = st.hit(self.cursor.0, self.cursor.1, &l, &self.fonts, self.ws.groups.len());
            let hover = Some(hit);
            if hover != st.hover {
                st.hover = hover;
                let pointer = matches!(hit, settings::Hit::Close | settings::Hit::Section(_) | settings::Hit::Font(_) | settings::Hit::SizeDown | settings::Hit::SizeUp);
                self.window.set_cursor(if pointer { CursorIcon::Pointer } else { CursorIcon::Default });
                self.window.request_redraw();
            }
            return;
        }
        if let Some(menu) = self.menu.as_mut() {
            let hover = menu.item_at(self.cursor.0, self.cursor.1, self.window.scale_factor() as f32);
            if hover != menu.hover {
                menu.hover = hover;
                self.window.set_cursor(if hover.is_some() { CursorIcon::Pointer } else { CursorIcon::Default });
                self.window.request_redraw();
            }
            return;
        }
        let n_tabs = self.ws.group().map_or(0, |g| g.tabs.len());
        let hit = l.hit(self.cursor.0, self.cursor.1, self.ws.groups.len(), n_tabs);
        if hit != self.hover {
            self.hover = hit;
            let icon = match hit {
                Hit::Terminal => CursorIcon::Text,
                Hit::Group(_)
                | Hit::CloseGroup(_)
                | Hit::NewGroup
                | Hit::Tab(_)
                | Hit::CloseTab(_)
                | Hit::NewTab
                | Hit::Awake
                | Hit::Update
                | Hit::Settings
                | Hit::Stats => {
                    CursorIcon::Pointer
                }
                _ => CursorIcon::Default,
            };
            self.window.set_cursor(icon);
            self.window.request_redraw();
        }

        if self.selecting || self.mouse_button.is_some() {
            let (col, row, side) = l.cell_at(self.cursor.0, self.cursor.1);
            let mode = self.active_mode();
            if let Some(button) = self.mouse_button {
                if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) {
                    self.mouse_report(button + 32, true, col, row, mode);
                }
                return;
            }
            if let Some(t) = self.active() {
                let mut term = t.term.lock();
                let point = ui::grid_point(&term, col, row);
                if let Some(sel) = term.selection.as_mut() {
                    sel.update(point, side);
                }
                drop(term);
                self.window.request_redraw();
            }
        }
    }

    fn on_mouse(&mut self, state: ElementState, button: MouseButton) {
        let l = self.layout();
        let n_tabs = self.ws.group().map_or(0, |g| g.tabs.len());
        let hit = l.hit(self.cursor.0, self.cursor.1, self.ws.groups.len(), n_tabs);
        let pressed = state == ElementState::Pressed;

        // Confirmación abierta: solo sus botones; un clic fuera la cancela.
        if let Some(c) = &self.confirm {
            if pressed && button == MouseButton::Left {
                match c.hit(self.cursor.0, self.cursor.1, &l, &mut self.fonts) {
                    confirm::Hit::Confirm => self.run_confirm(),
                    confirm::Hit::Cancel | confirm::Hit::Outside => self.confirm = None,
                    confirm::Hit::Inside => {}
                }
                self.on_cursor_moved(PhysicalPosition::new(self.cursor.0 as f64, self.cursor.1 as f64));
                self.window.request_redraw();
            }
            return;
        }

        // Detalle de uso abierto: un clic fuera o en la ✕ lo cierra.
        if let Some(d) = &self.stats_detail {
            if pressed && button == MouseButton::Left && d.hit(self.cursor.0, self.cursor.1, &l) != statsview::Hit::Inside {
                self.toggle_stats();
            }
            return;
        }

        // Configuración abierta: los clics son del modal.
        if self.settings.is_some() {
            if pressed && button == MouseButton::Left {
                self.settings_click();
            }
            return;
        }

        // Menú abierto: un clic elige una opción o lo cierra; no llega a lo de abajo.
        if let Some(menu) = &self.menu {
            if pressed {
                let item = menu.item_at(self.cursor.0, self.cursor.1, l.s());
                match item.and_then(|i| menu.items[i].action) {
                    Some(action) if button == MouseButton::Left => self.run_menu(action),
                    _ => self.menu = None,
                }
                self.on_cursor_moved(PhysicalPosition::new(self.cursor.0 as f64, self.cursor.1 as f64));
                self.window.request_redraw();
            }
            return;
        }

        if !pressed {
            if button == MouseButton::Left {
                self.drag_end();
            }
            if let Some(b) = self.mouse_button.take() {
                let (col, row, _) = l.cell_at(self.cursor.0, self.cursor.1);
                self.mouse_report(b, false, col, row, self.active_mode());
            }
            self.selecting = false;
            return;
        }

        // macOS mueve la ventana al arrastrar en la barra de título, donde están los tabs:
        // solo se permite desde sus zonas vacías, así arrastrar un tab lo reordena.
        if button == MouseButton::Left {
            appicon::set_window_movable(&self.window, hit == Hit::Drag);
        }

        // Conteo de clics para doble/triple clic.
        let now = Instant::now();
        let count = if self.clicks.1 == hit && now - self.clicks.0 < DOUBLE_CLICK { self.clicks.2 % 3 + 1 } else { 1 };
        self.clicks = (now, hit, count);

        // Buscador: un clic fuera de él y de sus resultados lo cierra.
        if self.search.is_some() && !matches!(hit, Hit::Search | Hit::Group(_)) {
            self.close_search();
        }
        // Con resultados a la vista, las filas de la lista son resultados.
        if self.searching() {
            if let Hit::Group(i) = hit {
                match button {
                    MouseButton::Left => self.go_to_result(i),
                    MouseButton::Right => {
                        if let Some(f) = self.search_results().get(i).copied() {
                            let id = f.tab.and_then(|ti| self.ws.groups[f.group].tabs.get(ti).copied());
                            self.open_menu(id, f.group);
                        }
                    }
                    _ => {}
                }
                self.window.request_redraw();
                return;
            }
        }

        if hit != Hit::Terminal && button == MouseButton::Left {
            let edited = match (hit, self.editing.as_ref().map(|(e, _)| *e)) {
                (Hit::Group(i), Some(Edit::Group(g))) => i == g,
                (Hit::Tab(i), Some(Edit::Tab(id))) => self.ws.group().and_then(|g| g.tabs.get(i)) == Some(&id),
                _ => false,
            };
            if !edited {
                self.editing = None;
            }
        }

        match (hit, button) {
            (Hit::Group(i), MouseButton::Left) => {
                if count >= 2 {
                    self.start_rename(i);
                } else {
                    if self.ws.active != i {
                        self.ws.select_group(i);
                        self.after_switch();
                    }
                    self.drag = Some(Dragging { item: Dragged::Group(i), from: self.cursor, moved: false, drop_group: None });
                }
            }
            (Hit::NewGroup, MouseButton::Left) => {
                self.new_group();
            }
            (Hit::Tab(i), MouseButton::Left) => {
                let id = self.ws.group().and_then(|g| g.tabs.get(i).copied());
                if count >= 2 {
                    if let Some(id) = id {
                        self.start_rename_tab(id);
                    }
                } else {
                    self.ws.select_tab(i);
                    self.after_switch();
                    if let Some(id) = id {
                        self.drag = Some(Dragging { item: Dragged::Tab(id), from: self.cursor, moved: false, drop_group: None });
                    }
                }
            }
            (Hit::Tab(i) | Hit::CloseTab(i), MouseButton::Right) => {
                if let Some(id) = self.ws.group().and_then(|g| g.tabs.get(i).copied()) {
                    self.open_menu(Some(id), self.ws.active);
                }
            }
            (Hit::Group(i) | Hit::CloseGroup(i), MouseButton::Right) => self.open_menu(None, i),
            (Hit::CloseGroup(i), MouseButton::Left) => self.ask_delete_group(i),
            (Hit::Search, MouseButton::Left) => self.open_search(),
            (Hit::Tab(i), MouseButton::Middle) | (Hit::CloseTab(i), MouseButton::Left) => {
                if let Some(id) = self.ws.group().and_then(|g| g.tabs.get(i).copied()) {
                    self.close_term(id);
                    self.resize_terms();
                }
            }
            (Hit::NewTab, MouseButton::Left) => {
                self.new_tab();
            }
            (Hit::Awake, MouseButton::Left) => self.cycle_awake(),
            (Hit::Update, MouseButton::Left) => self.update_action(),
            (Hit::Settings, MouseButton::Left) => self.toggle_settings(),
            (Hit::Stats, MouseButton::Left) => self.toggle_stats(),
            (Hit::Drag, MouseButton::Left) => {
                if count == 2 {
                    let max = self.window.is_maximized();
                    self.window.set_maximized(!max);
                } else {
                    let _ = self.window.drag_window();
                }
            }
            (Hit::Terminal, _) => self.on_terminal_press(button, count),
            _ => {}
        }
        self.window.request_redraw();
    }

    /// Arrastrando: el grupo o tab se mueve en vivo al lugar bajo el puntero. Un tab
    /// llevado a la barra lateral marca el grupo donde va a quedar.
    fn drag_move(&mut self) {
        let Some(mut d) = self.drag.take() else { return };
        let l = self.layout();
        let (x, y) = self.cursor;
        if !d.moved {
            let (dx, dy) = (x - d.from.0, y - d.from.1);
            let min = 5.0 * l.s();
            if dx * dx + dy * dy < min * min {
                self.drag = Some(d);
                return;
            }
            d.moved = true;
            self.editing = None;
            self.window.set_cursor(CursorIcon::Grabbing);
        }
        match d.item {
            Dragged::Group(from) => {
                let n = self.ws.groups.len();
                let to = l.row_at(y).clamp(0, n as isize - 1) as usize;
                if self.ws.move_group(from, to) {
                    d.item = Dragged::Group(to);
                    self.reveal_row(to);
                }
            }
            Dragged::Tab(id) => {
                let src = self.ws.group_of(id);
                if x < l.sidebar_w() {
                    let row = l.row_at(y);
                    let row_ok = row >= 0 && l.row_visible(row as usize) && (row as usize) < self.ws.groups.len();
                    d.drop_group = (row_ok && Some(row as usize) != src).then_some(row as usize);
                } else {
                    d.drop_group = None;
                    if let Some(g) = src.filter(|g| *g == self.ws.active) {
                        let tabs = &self.ws.groups[g].tabs;
                        if let Some(from) = tabs.iter().position(|t| *t == id) {
                            let to = l.tab_at(x, tabs.len());
                            self.ws.move_tab(g, from, to);
                        }
                    }
                }
            }
        }
        self.drag = Some(d);
        self.window.request_redraw();
    }

    /// Al soltar: un tab sobre otro grupo pasa a ese grupo; lo demás ya quedó en su lugar.
    fn drag_end(&mut self) {
        let Some(d) = self.drag.take() else { return };
        if !d.moved {
            return;
        }
        if let (Dragged::Tab(id), Some(gi)) = (d.item, d.drop_group) {
            if self.ws.move_tab_to_group(id, gi) {
                self.after_switch();
            }
        }
        // Los grupos pasados de su límite se indican por índice: se recalculan.
        self.enforce_limits();
        self.save_session();
        self.hover = Hit::None;
        self.on_cursor_moved(PhysicalPosition::new(self.cursor.0 as f64, self.cursor.1 as f64));
        self.window.request_redraw();
    }

    fn on_terminal_press(&mut self, button: MouseButton, count: u8) {
        let l = self.layout();
        let (col, row, side) = l.cell_at(self.cursor.0, self.cursor.1);
        let mode = self.active_mode();
        let code = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            _ => return,
        };
        // La app pidió el mouse (vim, htop…): se le reporta. Shift fuerza selección local.
        if mode.intersects(TermMode::MOUSE_MODE) && !self.mods.shift_key() {
            self.mouse_button = Some(code);
            self.mouse_report(code, true, col, row, mode);
            return;
        }
        if button != MouseButton::Left {
            return;
        }
        let Some(t) = self.active() else { return };
        let mut term = t.term.lock();
        let point = ui::grid_point(&term, col, row);
        let ty = match count {
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        term.selection = Some(Selection::new(ty, point, side));
        drop(term);
        self.selecting = true;
    }

    /// Reporte de mouse al estilo xterm (SGR si está activo, si no X10).
    fn mouse_report(&self, button: u8, pressed: bool, col: usize, row: usize, mode: TermMode) {
        let m = self.mods;
        let b = button + 4 * m.shift_key() as u8 + 8 * m.alt_key() as u8 + 16 * m.control_key() as u8;
        let bytes = if mode.contains(TermMode::SGR_MOUSE) {
            format!("\x1b[<{};{};{}{}", b, col + 1, row + 1, if pressed { 'M' } else { 'm' }).into_bytes()
        } else {
            let b = if pressed { b } else { 3 + (b & !3) };
            let clamp = |v: usize| (32 + 1 + v).min(255) as u8;
            vec![0x1b, b'[', b'M', 32 + b, clamp(col), clamp(row)]
        };
        if let Some(id) = self.ws.active_term() {
            self.write(id, bytes);
        }
    }

    fn on_scroll(&mut self, delta: MouseScrollDelta) {
        if self.confirm.is_some() {
            return;
        }
        let l = self.layout();
        // Sobre la barra lateral: desplaza la lista de grupos (o de resultados).
        if self.cursor.0 < l.sidebar_w() && self.settings.is_none() && self.stats_detail.is_none() {
            let row = 32.0 * l.s();
            let px = match delta {
                MouseScrollDelta::LineDelta(_, y) => -y * row,
                MouseScrollDelta::PixelDelta(p) => -p.y as f32,
            };
            self.list_scroll_px += px;
            let rows = (self.list_scroll_px / row).trunc();
            if rows != 0.0 {
                self.list_scroll_px -= rows * row;
                let max = self.list_len().saturating_sub(l.list_rows());
                self.list_scroll = (self.list_scroll as isize + rows as isize).clamp(0, max as isize) as usize;
                self.window.request_redraw();
            }
            return;
        }
        if self.stats_detail.is_some() {
            let rows = match delta {
                MouseScrollDelta::LineDelta(_, y) => -y.round() as isize,
                MouseScrollDelta::PixelDelta(p) => (-p.y as f32 / (28.0 * l.s())).round() as isize,
            };
            let total = self.stats.as_ref().map_or(0, |sm| statsview::rows(sm, &self.ws, &self.terms).len());
            if let Some(d) = self.stats_detail.as_mut() {
                d.scroll_by(rows, total, &l);
            }
            self.window.request_redraw();
            return;
        }
        if let Some(st) = self.settings.as_mut() {
            let rows = match delta {
                MouseScrollDelta::LineDelta(_, y) => -y.round() as isize,
                MouseScrollDelta::PixelDelta(p) => (-p.y as f32 / (26.0 * l.s())).round() as isize,
            };
            if st.section == settings::IDLE_SECTION {
                let n = self.ws.groups.len();
                let max = n.saturating_sub(st.idle_capacity(&l, n));
                st.idle_scroll = (st.idle_scroll as isize + rows).clamp(0, max as isize) as usize;
                self.window.request_redraw();
                return;
            }
            if st.section == settings::PERF_SECTION {
                let n = self.ws.groups.len();
                let max = n.saturating_sub(st.perf_capacity(&l, n));
                st.perf_scroll = (st.perf_scroll as isize + rows).clamp(0, max as isize) as usize;
                self.window.request_redraw();
                return;
            }
            let page = st.page(&l, &self.fonts);
            st.scroll_by(rows, page);
            self.window.request_redraw();
            return;
        }
        if l.hit(self.cursor.0, self.cursor.1, self.ws.groups.len(), 0) != Hit::Terminal {
            return;
        }
        let px = match delta {
            MouseScrollDelta::LineDelta(_, y) => y * l.cell_h,
            MouseScrollDelta::PixelDelta(p) => p.y as f32,
        };
        self.scroll_px += px;
        let lines = (self.scroll_px / l.cell_h).trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_px -= lines as f32 * l.cell_h;

        let mode = self.active_mode();
        let (col, row, _) = l.cell_at(self.cursor.0, self.cursor.1);
        if mode.intersects(TermMode::MOUSE_MODE) {
            let button = if lines > 0 { 64 } else { 65 };
            for _ in 0..lines.unsigned_abs() {
                self.mouse_report(button, true, col, row, mode);
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let seq: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            let bytes = seq.repeat(lines.unsigned_abs() as usize);
            if let Some(id) = self.ws.active_term() {
                self.write(id, bytes);
            }
        } else if let Some(t) = self.active() {
            t.term.lock().scroll_display(Scroll::Delta(lines));
            self.window.request_redraw();
        }
    }

    /// El tab activo ya se vio: se limpia la campana y «terminó» pasa a «en espera».
    fn mark_seen(&mut self) {
        if let Some(t) = self.ws.active_term().and_then(|id| self.terms.get_mut(&id)) {
            t.attention = false;
            if t.agent.state == AgentState::Done {
                t.agent.set_state(AgentState::Idle);
            }
        }
    }

    /// Qué tan urgente es un tab: 2 = te necesita, 1 = terminó o sonó la campana.
    fn urgency(&self, id: u64) -> u8 {
        match self.terms.get(&id) {
            Some(t) if t.agent.state == AgentState::NeedsYou => 2,
            Some(t) if t.agent.state == AgentState::Done || t.attention => 1,
            _ => 0,
        }
    }

    /// ⌘J: salta al siguiente tab que te necesita; si no hay, al siguiente que terminó.
    fn jump_to_attention(&mut self) {
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

    fn on_ipc(&mut self, body: &Value) -> Value {
        match body["cmd"].as_str() {
            Some("hook") => {
                if let Some(tab) = body["tab"].as_u64() {
                    self.on_hook(tab, &body["payload"]);
                }
                json!({ "ok": true })
            }
            Some("tabs") => self.tabs_json(),
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
    fn resolve_tab(&self, spec: &Value) -> Result<u64, String> {
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
    fn ipc_new(&mut self, body: &Value) -> Value {
        let cwd = body["cwd"].as_str().map(PathBuf::from).filter(|d| d.is_absolute());
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
    fn ipc_group(&mut self, body: &Value) -> Value {
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
    fn ipc_suspend(&mut self, suspend: bool, body: &Value) -> Value {
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
                        return json!({ "ok": false, "error": format!("Claude está {} en el tab {id}; usa --force para suspenderlo igual", if busy == AgentState::Working { "trabajando" } else { "esperando tu respuesta" }) });
                    }
                    vec![id]
                }
                Err(e) => return json!({ "ok": false, "error": e }),
            },
        };
        let changed: Vec<u64> = ids.into_iter().filter(|id| if suspend { self.suspend(*id) } else { self.resume(*id) }).collect();
        json!({ "ok": true, "changed": changed })
    }

    /// Clic en el pie de versión: buscar, instalar o reiniciar, según el estado.
    fn update_action(&mut self) {
        use update::Status;
        match self.update.clone() {
            Status::Idle | Status::UpToDate | Status::Error(_) => {
                self.update_quiet = false;
                update::check(self.proxy.clone());
            }
            Status::Available(release) => {
                self.update = Status::Downloading(release.version.clone(), 0, release.size);
                update::install(self.proxy.clone(), release);
            }
            Status::Ready(_) => {
                // Guarda la sesión, sale y otro proceso abre la versión nueva al terminar este.
                self.save_session();
                update::relaunch();
                self.quit = true;
            }
            Status::Checking | Status::Downloading(..) | Status::Applying(_) => {}
        }
        self.window.request_redraw();
    }

    /// Nueva muestra de uso. Solo se repinta si cambia lo que se ve.
    fn on_stats(&mut self, sample: stats::Sample) {
        let shown = format!("{:.0}|{}|{:.0}", sample.cpu, stats::human_bytes(sample.mem), sample.gpu);
        let changed = shown != self.meters_shown;
        self.meters_shown = shown;
        self.stats = Some(sample);
        let over_changed = self.enforce_limits();
        self.enforce_idle();
        if changed || over_changed || self.stats_detail.is_some() || self.settings.is_some() {
            self.window.request_redraw();
        }
    }

    /// Revisa los límites de memoria con la última muestra. Con suspensión automática,
    /// suspende un tab (el inactivo que más usa) si algo está pasado; uno cada 4 s, para
    /// darle tiempo a la memoria a liberarse antes de decidir otro. Devuelve si cambió
    /// qué está pasado.
    fn enforce_limits(&mut self) -> bool {
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
                notify_macos(&place, &format!("Se suspendió «{name}» para respetar el límite de memoria"));
            }
        }
        changed
    }

    /// Estado de inactividad de cada tab y grupo. Antes, marca como usados el tab visible
    /// y los tabs donde Claude trabaja: un agente trabajando mucho rato no es inactividad,
    /// aunque no toques nada.
    fn idle_state(&self) -> (Vec<idle::Tab>, Vec<idle::Group>) {
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
    fn enforce_idle(&mut self) {
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
    fn idle_view(&self) -> settings::IdleView {
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
    fn set_idle(&mut self, f: impl FnOnce(&mut idle::Rules)) {
        f(&mut self.config.idle);
        self.config.save();
        self.enforce_idle();
    }

    /// − / + de un límite: 0 es Telar completo (configuración); 1.. el grupo i-1 (sesión).
    fn step_limit(&mut self, i: usize, delta: i32) {
        if i == 0 {
            self.config.ram_limit_mb = limits::step(self.config.ram_limit_mb, delta);
            self.config.save();
        } else if let Some(g) = self.ws.groups.get_mut(i - 1) {
            g.ram_limit_mb = limits::step(g.ram_limit_mb, delta);
            self.save_session();
        }
        self.enforce_limits();
        self.window.request_redraw();
    }

    /// Datos de la sección Rendimiento del modal.
    fn perf_view(&self) -> settings::Perf {
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
    fn toggle_stats(&mut self) {
        self.menu = None;
        self.settings = None;
        self.stats_detail = match self.stats_detail {
            Some(_) => None,
            None => Some(statsview::Detail::default()),
        };
        self.window.set_cursor(CursorIcon::Default);
        self.window.request_redraw();
    }

    /// ⌘, o el engranaje: abre o cierra la configuración.
    fn toggle_settings(&mut self) {
        self.menu = None;
        self.editing = None;
        self.settings = match self.settings {
            Some(_) => None,
            None => Some(settings::Settings::open(&self.config.font_family)),
        };
        self.window.set_cursor(CursorIcon::Default);
        self.window.request_redraw();
    }

    /// Cambia la familia de la terminal y la interfaz, y la guarda.
    fn set_font_family(&mut self, family: &str) {
        if family.eq_ignore_ascii_case(&self.fonts.family) {
            return;
        }
        self.fonts = Fonts::new(family, self.font_pt * self.window.scale_factor() as f32);
        self.config.font_family = self.fonts.family.clone();
        self.config.save();
        self.resize_terms();
        self.window.request_redraw();
    }

    /// Cambia el ícono de la app (Dock y Telar.app) y lo guarda.
    fn set_app_icon(&mut self, id: &str) {
        let result = appicon::apply(id, true);
        if let Some(st) = self.settings.as_mut() {
            st.icon_error = result.as_ref().err().cloned();
        }
        if result.is_ok() {
            self.config.app_icon = id.to_string();
            self.config.save();
        }
        self.window.request_redraw();
    }

    /// «Elegir archivo…»: selector de macOS; la imagen elegida pasa a ser el ícono propio.
    fn pick_app_icon(&mut self) {
        let Some(path) = appicon::pick_file() else { return };
        match appicon::import(&path) {
            Ok(()) => self.set_app_icon(appicon::CUSTOM),
            Err(e) => {
                if let Some(st) = self.settings.as_mut() {
                    st.icon_error = Some(e);
                }
            }
        }
        self.window.request_redraw();
    }

    /// Cambia el tema (interfaz y colores de la terminal) y lo guarda.
    fn set_theme(&mut self, name: &str) {
        if !theme::set(name) {
            return;
        }
        self.palette = theme::t().palette();
        self.config.theme = theme::t().name.to_string();
        self.config.save();
        self.window.request_redraw();
    }

    /// Tema anterior o siguiente de la lista (← → en la configuración).
    fn cycle_theme(&mut self, delta: isize) {
        let list = theme::themes();
        let i = list.iter().position(|t| t.name == theme::t().name).unwrap_or(0) as isize;
        let next = &list[(i + delta).rem_euclid(list.len() as isize) as usize];
        self.set_theme(next.name);
    }

    /// Tamaño desde la configuración: se aplica y queda como el de ⌘0.
    fn set_font_size(&mut self, pt: f32) {
        self.font_pt = pt.clamp(config::FONT_SIZE_MIN, config::FONT_SIZE_MAX);
        self.config.font_size = self.font_pt;
        self.config.save();
        self.apply_font_size();
    }

    fn settings_click(&mut self) {
        let l = self.layout();
        let Some(st) = self.settings.as_mut() else { return };
        match st.hit(self.cursor.0, self.cursor.1, &l, &self.fonts, self.ws.groups.len()) {
            settings::Hit::Outside | settings::Hit::Close => self.settings = None,
            settings::Hit::Section(i) => st.section = i,
            settings::Hit::Font(i) => {
                let page = st.page(&l, &self.fonts);
                st.select(i, page);
                let name = st.visible()[i].name.clone();
                self.set_font_family(&name);
            }
            settings::Hit::Theme(i) => {
                if let Some(t) = theme::themes().get(i) {
                    self.set_theme(t.name);
                }
            }
            settings::Hit::Icon(i) => {
                let id = appicon::CHOICES.get(i).map_or(appicon::CUSTOM, |c| c.id);
                if id == appicon::CUSTOM && !appicon::has_custom() {
                    self.pick_app_icon();
                } else {
                    self.set_app_icon(id);
                }
            }
            settings::Hit::PickIcon => self.pick_app_icon(),
            settings::Hit::LimitDown(i) => self.step_limit(i, -1),
            settings::Hit::LimitUp(i) => self.step_limit(i, 1),
            settings::Hit::AutoSuspend => {
                self.config.auto_suspend = !self.config.auto_suspend;
                self.config.save();
                self.enforce_limits();
            }
            settings::Hit::IdleTabDown => self.set_idle(|r| r.tab_min = idle::step(r.tab_min, -1)),
            settings::Hit::IdleTabUp => self.set_idle(|r| r.tab_min = idle::step(r.tab_min, 1)),
            settings::Hit::IdleGroupDown => self.set_idle(|r| r.group_min = idle::step(r.group_min, -1)),
            settings::Hit::IdleGroupUp => self.set_idle(|r| r.group_min = idle::step(r.group_min, 1)),
            settings::Hit::IdleWaiting => self.set_idle(|r| r.include_waiting = !r.include_waiting),
            settings::Hit::IdleRunning => self.set_idle(|r| r.include_running = !r.include_running),
            settings::Hit::IdleExempt(gi) => {
                if let Some(g) = self.ws.groups.get_mut(gi) {
                    g.idle_exempt = !g.idle_exempt;
                    self.save_session();
                }
            }
            settings::Hit::SizeDown => self.set_font_size(self.font_pt - 1.0),
            settings::Hit::SizeUp => self.set_font_size(self.font_pt + 1.0),
            settings::Hit::Inside => {}
        }
        self.window.request_redraw();
    }

    /// Teclado con la configuración abierta: escribir filtra, flechas eligen (y aplican),
    /// ⌘+/⌘- cambian el tamaño, Esc o ⌘, cierran.
    fn settings_key(&mut self, event: &KeyEvent) {
        let l = self.layout();
        let cmd = self.mods.super_key();
        let Some(st) = self.settings.as_mut() else { return };
        let page = st.page(&l, &self.fonts);
        let mut apply = false;
        // Las flechas y el filtro son de la sección Apariencia.
        if st.section == settings::ICON_SECTION {
            let close = event.logical_key == Key::Named(NamedKey::Escape)
                || (cmd && matches!(event.key_without_modifiers(), Key::Character(ref c) if c.as_str() == "," || c.as_str() == "w"));
            if close {
                self.settings = None;
            }
            self.window.request_redraw();
            return;
        }
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => self.settings = None,
            Key::Named(NamedKey::ArrowDown) => {
                st.select(st.selected + 1, page);
                apply = true;
            }
            Key::Named(NamedKey::ArrowUp) => {
                st.select(st.selected.saturating_sub(1), page);
                apply = true;
            }
            Key::Named(NamedKey::PageDown) => {
                st.select(st.selected + page, page);
                apply = true;
            }
            Key::Named(NamedKey::PageUp) => {
                st.select(st.selected.saturating_sub(page), page);
                apply = true;
            }
            Key::Named(NamedKey::Enter) => apply = true,
            Key::Named(NamedKey::ArrowLeft) => {
                self.cycle_theme(-1);
                return;
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.cycle_theme(1);
                return;
            }
            Key::Named(NamedKey::Backspace) => {
                st.filter.pop();
                st.select(0, page);
                st.scroll = 0;
            }
            _ if cmd => match event.key_without_modifiers() {
                Key::Character(c) if c.as_str() == "," || c.as_str() == "w" => self.settings = None,
                Key::Character(c) if c.as_str() == "=" || c.as_str() == "+" => self.set_font_size(self.font_pt + 1.0),
                Key::Character(c) if c.as_str() == "-" => self.set_font_size(self.font_pt - 1.0),
                Key::Character(c) if c.as_str() == "v" => {
                    if let Some(text) = self.clipboard.as_mut().and_then(|cb| cb.get_text().ok()) {
                        if let Some(st) = self.settings.as_mut() {
                            st.filter.extend(text.chars().filter(|c| !c.is_control()));
                            st.select(0, page);
                            st.scroll = 0;
                        }
                    }
                }
                _ => {}
            },
            _ => {
                if let Some(t) = &event.text {
                    st.filter.extend(t.chars().filter(|c| !c.is_control()));
                    st.select(0, page);
                    st.scroll = 0;
                }
            }
        }
        if apply {
            if let Some(name) = self.settings.as_ref().and_then(|st| st.visible().get(st.selected).map(|f| f.name.clone())) {
                self.set_font_family(&name);
            }
        }
        self.window.request_redraw();
    }

    /// ⌘⇧C o clic abajo en la barra lateral: apagado → siempre → con Claude.
    fn cycle_awake(&mut self) {
        self.awake.mode = self.awake.mode.next();
        self.update_awake();
        self.window.request_redraw();
    }

    fn update_awake(&mut self) {
        let working = self.terms.values().any(|t| t.agent.state == AgentState::Working);
        let before = self.awake.active();
        self.awake.update(working);
        if before != self.awake.active() {
            self.window.request_redraw();
        }
    }

    fn tabs_json(&self) -> Value {
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

    fn tab_json(&self, id: u64) -> Value {
        let Some(t) = self.terms.get(&id) else { return Value::Null };
        let group = self.ws.group_of(id).map(|gi| self.ws.groups[gi].name.clone());
        json!({
            "id": id,
            "group": group,
            "title": t.title,
            "name": t.name,
            "process": t.process,
            "state": t.agent.state.name(),
            "suspended": t.is_suspended(),
            "state_secs": t.agent.since.elapsed().as_secs(),
            "idle_secs": t.last_used.get().elapsed().as_secs(),
            "changes": t.agent.changes,
            "session_id": t.agent.session_id,
            "cwd": t.cwd().or_else(|| t.agent.cwd.as_ref().map(PathBuf::from)),
            "message": t.agent.message,
        })
    }

    /// Cada pocos segundos: nombre del proceso de cada tab y guardado de la sesión.
    fn periodic(&mut self) {
        let mut changed = false;
        for t in self.terms.values_mut() {
            let before = t.process.clone();
            t.refresh_process();
            changed |= before != t.process;
            // Claude se cerró sin avisar (sin SessionEnd): si hace rato que en primer plano
            // hay una shell, el tab vuelve a ser una shell normal y no se retoma al reabrir.
            let shell = t.process.as_deref().is_some_and(proc::is_shell);
            // Solo en «en espera» o «terminó»: mientras trabaja, una herramienta Bash larga
            // también puede dejar una shell en primer plano.
            let resting = matches!(t.agent.state, AgentState::Idle | AgentState::Done);
            if shell && resting && t.agent.since.elapsed() > Duration::from_secs(30) {
                t.agent.set_state(AgentState::None);
                changed = true;
            }
        }
        if changed {
            self.window.request_redraw();
        }
        self.update_awake();
        self.save_session();
    }

    fn saved_state(&self) -> Saved {
        let groups = self
            .ws
            .groups
            .iter()
            .map(|g| SavedGroup {
                name: g.name.clone(),
                dir: g.dir.clone(),
                ram_limit_mb: g.ram_limit_mb,
                idle_exempt: g.idle_exempt,
                active: g.active,
                tabs: g
                    .tabs
                    .iter()
                    .filter_map(|id| self.terms.get(id))
                    .map(|t| {
                        let claude = (t.agent.state != AgentState::None).then(|| t.agent.session_id.clone()).flatten();
                        let screen = t.frozen.as_ref().map(|f| f.screen.clone());
                        // Con Claude abierto, su carpeta es donde se inició la sesión (la necesita --resume).
                        let cwd = if t.is_suspended() {
                            t.cwd()
                        } else if claude.is_some() {
                            t.agent.cwd.as_ref().map(PathBuf::from).or_else(|| t.cwd())
                        } else {
                            t.cwd()
                        };
                        let command = t.frozen.as_ref().and_then(|f| f.command.clone());
                        SavedTab { title: t.title.clone(), name: t.name.clone(), cwd, claude, screen, command }
                    })
                    .collect(),
            })
            .collect();
        Saved { active: self.ws.active, groups, awake: self.awake.mode.name().to_string() }
    }

    fn save_session(&mut self) {
        if !self.persist {
            return;
        }
        let saved = self.saved_state();
        if self.last_saved.as_ref() != Some(&saved) {
            session::save(&saved);
            self.last_saved = Some(saved);
        }
    }

    /// Vuelve a abrir los grupos y tabs guardados; los que tenían Claude retoman su sesión.
    fn restore(&mut self, saved: Saved) {
        self.awake.mode = awake::Mode::parse(&saved.awake).unwrap_or_default();
        let mut groups = Vec::new();
        for g in saved.groups {
            let mut tabs = Vec::new();
            for tab in g.tabs {
                let id = self.next_id;
                // Suspendido: no arranca nada, solo vuelve su vista previa.
                let mut t = match tab.screen {
                    Some(screen) => {
                        self.next_id += 1;
                        let mut t = Terminal::frozen(id, self.layout().grid_size(), self.proxy.clone(), Frozen { cwd: tab.cwd.clone(), screen, command: tab.command.clone() });
                        if let Some(session) = &tab.claude {
                            t.agent.session_id = Some(session.clone());
                            t.agent.cwd = tab.cwd.as_ref().map(|c| c.display().to_string());
                            t.agent.set_state(AgentState::Idle);
                        }
                        t
                    }
                    None => {
                        let Some(id) = self.spawn_term_in(tab.cwd.as_deref()) else { continue };
                        let mut t = self.terms.remove(&id).unwrap();
                        if let Some(session) = &tab.claude {
                            resume_claude(&mut t, session, tab.cwd.as_deref());
                        }
                        t
                    }
                };
                t.title = tab.title;
                t.name = tab.name;
                self.terms.insert(id, t);
                tabs.push(id);
            }
            if !tabs.is_empty() {
                let active = g.active.min(tabs.len() - 1);
                groups.push(Group { name: g.name, tabs, active, dir: g.dir, ram_limit_mb: g.ram_limit_mb, idle_exempt: g.idle_exempt });
            }
        }
        self.ws.restore(groups, saved.active);
    }

    /// Evento de un hook de Claude Code en el tab `tab`.
    fn on_hook(&mut self, tab: u64, p: &Value) {
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
            "UserPromptSubmit" | "PreToolUse" | "PostToolUse" => Some(AgentState::Working),
            "Notification" => {
                // El aviso de «sigo esperando tu input» (a los 60 s) no cambia nada:
                // la sesión ya está en «terminó» o «en espera».
                let idle = p["notification_type"].as_str() == Some("idle_prompt")
                    || message.contains("waiting for your input");
                if idle { None } else { Some(AgentState::NeedsYou) }
            }
            "Stop" => Some(if visible { AgentState::Idle } else { AgentState::Done }),
            "SessionEnd" => Some(AgentState::None),
            _ => None,
        };
        let Some(new) = new else { return };
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
            notify_macos(&place, &what);
            self.window.request_user_attention(Some(UserAttentionType::Informational));
        }
        self.window.request_redraw();
    }

    /// «Grupo › título del tab», para las notificaciones.
    fn place_of(&self, tab: u64) -> String {
        let group = self.ws.groups.iter().find(|g| g.tabs.contains(&tab)).map_or("", |g| g.name.as_str());
        let title = self.terms.get(&tab).and_then(|t| t.title.clone()).unwrap_or_default();
        if title.is_empty() { group.to_string() } else { format!("{group} › {title}") }
    }

    /// Escenario del modo snapshot: varios grupos y tabs, y el texto de prueba.
    fn snapshot_setup(&mut self) {
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
    fn snapshot_overlays(&mut self) {
        // TELAR_SNAPSHOT_SEARCH=texto: captura con el buscador abierto.
        if let Some(q) = std::env::var("TELAR_SNAPSHOT_SEARCH").ok().filter(|q| !q.is_empty()) {
            self.search = Some((q, 0));
        }
        // TELAR_SNAPSHOT_STATS=1: captura con el detalle de uso abierto.
        if std::env::var_os("TELAR_SNAPSHOT_STATS").is_some() {
            self.toggle_stats();
        }
        // TELAR_SNAPSHOT_DELETE=n: captura con la confirmación de eliminar el grupo n.
        if let Some(gi) = std::env::var("TELAR_SNAPSHOT_DELETE").ok().and_then(|v| v.parse().ok()) {
            self.ask_delete_group(gi);
        }
        // TELAR_SNAPSHOT_HOVER=x,y: el puntero en ese punto (en puntos).
        if let Some((x, y)) = std::env::var("TELAR_SNAPSHOT_HOVER").ok().and_then(|v| {
            let (x, y) = v.split_once(',')?;
            Some((x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?))
        }) {
            let s = self.window.scale_factor();
            self.on_cursor_moved(PhysicalPosition::new(x * s, y * s));
        }
        // TELAR_SNAPSHOT_SETTINGS=1: captura con la configuración abierta.
        if let Ok(section) = std::env::var("TELAR_SNAPSHOT_SETTINGS") {
            self.toggle_settings();
            if let Some(st) = self.settings.as_mut() {
                match section.as_str() {
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
            _ => {}
        }
    }

    fn draw(&mut self) {
        // Si se borraron grupos o la ventana se achicó, la lista no queda desplazada de más.
        let max = self.list_len().saturating_sub(self.layout().list_rows());
        self.list_scroll = self.list_scroll.min(max);
        let l = self.layout();
        let results = self.search_results();
        let view = View {
            ws: &self.ws,
            terms: &self.terms,
            hover: self.hover,
            editing: self.editing.as_ref().map(|(e, s)| (*e, s.as_str())),
            preedit: &self.preedit,
            focused: self.focused,
            palette: &self.palette,
            time: self.started.elapsed().as_secs_f32(),
            awake: (self.awake.mode, self.awake.active()),
            menu: self.menu.as_ref(),
            update: &self.update,
            stats: self.stats.as_ref(),
            search: self.search.as_ref().map(|(q, sel)| (q.as_str(), *sel, results.as_slice())),
            ram_limit_mb: self.config.ram_limit_mb,
            over_groups: &self.over_groups,
            drop_group: self.drag.as_ref().and_then(|d| d.drop_group),
        };
        let mut ime_cell = ui::draw(&mut self.quads, &mut self.fg_quads, &mut self.fonts, &l, &view);
        if let Some(st) = &self.settings {
            let ctx = settings::Ctx {
                family: &self.config.font_family,
                size: self.font_pt,
                app_icon: &self.config.app_icon,
                perf: self.perf_view(),
                idle: self.idle_view(),
            };
            st.draw(&mut self.quads, &mut self.fonts, &l, &ctx);
            ime_cell = None;
        }
        if let (Some(d), Some(sample)) = (&self.stats_detail, &self.stats) {
            let rows = statsview::rows(sample, &self.ws, &self.terms);
            d.draw(&mut self.quads, &mut self.fonts, &l, sample, &rows, self.config.ram_limit_mb);
            ime_cell = None;
        }
        if let Some(c) = &self.confirm {
            c.draw(&mut self.quads, &mut self.fonts, &l);
            ime_cell = None;
        }
        if let Some((col, row)) = ime_cell {
            let (x, y) = l.cell_origin(col, row);
            self.window.set_ime_cursor_area(
                PhysicalPosition::new(x as f64, y as f64),
                PhysicalSize::new(l.cell_w as f64, l.cell_h as f64),
            );
        }
        let bg = theme::t().bg;
        let capture = self.snapshot.as_ref().filter(|s| s.capture_now).map(|s| s.path.clone());
        self.renderer.render(&self.quads, bg, &mut self.fonts, capture.as_deref());
        if std::mem::take(&mut self.fonts.atlas_reset) {
            self.window.request_redraw();
        }
    }
}

/// Notificación de macOS (sin bundle de app todavía, vía osascript).
/// Retoma en `t` (recién abierto) la sesión de Claude `session`.
fn resume_claude(t: &mut Terminal, session: &str, cwd: Option<&Path>) {
    // La shell todavía arranca: lo escrito queda en el buffer del PTY y se ejecuta al estar lista.
    t.write(format!("claude --resume {session}\r").into_bytes());
    // Ya cuenta como tab de Claude aunque su hook SessionStart aún no llegue:
    // si se cierra Telar antes, la sesión no se pierde.
    t.agent.session_id = Some(session.to_string());
    t.agent.cwd = cwd.map(|c| c.display().to_string());
    t.agent.set_state(AgentState::Idle);
}

fn notify_macos(title: &str, body: &str) {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("display notification \"{}\" with title \"Telar\" subtitle \"{}\"", esc(body), esc(title));
    // En un hilo que espera al proceso, para no dejar zombis.
    std::thread::spawn(move || {
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}
