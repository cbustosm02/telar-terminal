//! Telar: terminal personal con grupos de tabs en una barra lateral.

mod app;
mod appicon;
mod awake;
mod cli;
mod claude_usage;
mod config;
mod confirm;
mod cuentas;
mod font;
mod fontlist;
mod hooks;
mod idle;
mod inbox;
mod input;
mod kit;
mod limits;
mod links;
mod notify;
mod logo;
mod ipc;
mod proc;
mod renderer;
mod screen;
mod session;
mod signing;
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

/// Ritmo del indicador de «trabajando»: ~12 fps con la ventana al frente y ~2 fps sin foco
/// (se ve igual de vivo de reojo y cuesta seis veces menos). Tapada, no se anima.
const SPINNER_TICK: Duration = Duration::from_millis(80);
const SPINNER_TICK_UNFOCUSED: Duration = Duration::from_millis(500);
/// Cada cuánto se busca en segundo plano una versión nueva.
const UPDATE_CHECK_EVERY: Duration = Duration::from_secs(60 * 60);
/// Cada cuánto se refrescan procesos/carpetas y se guarda la sesión (si cambió).
const SAVE_TICK: Duration = Duration::from_secs(5);
/// Sin cambios de tamaño durante este tiempo, el arrastre del borde terminó.
const RESIZE_SETTLE: Duration = Duration::from_millis(250);
/// Silencio tras el cual un Claude «trabajando» se da por detenido (ver `unstick_working`).
const STUCK_AFTER: Duration = Duration::from_secs(10);

pub enum UserEvent {
    /// Evento de la terminal `id`, de la generación indicada (ver `terminal::Listener`).
    Term(u64, u64, TermEvent),
    /// Avance de la búsqueda o instalación de una actualización.
    Update(update::Status),
    /// Uso de recursos (cada 2 s).
    Stats(Box<stats::Sample>),
    Ipc(ipc::Request),
    /// Lo elegido en un panel de macOS (`None`: se canceló).
    Picked(Picked),
    /// Cuenta de Claude Code (`claude auth status`); `None` si no se pudo leer.
    ClaudeAccount(Option<claude_usage::Account>),
    /// Cuentas guardadas y la clave de la activa.
    Accounts(Vec<cuentas::Saved>, Option<String>),
    /// Clic en una notificación (con el tab que avisó, si era de uno).
    NotificationClicked(Option<u64>),
    /// Uso consultado de cada cuenta (clave), o el error.
    UsageAll(Vec<(String, Result<claude_usage::Usage, String>)>),
    /// Terminó una operación de cuentas: mensaje para avisar, o el error.
    AccountDone(Result<String, String>),
}

/// Para qué se abrió un panel de macOS.
#[derive(Debug)]
pub enum Picked {
    StartDir(Option<PathBuf>),
    Icon(Option<PathBuf>),
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
        // Interno (scripts/vista-previa.sh): firma un paquete con la firma local estable.
        Some("__firmar") => {
            return finish(match args.get(1) {
                Some(p) if signing::sign(Path::new(p)) => Ok(String::new()),
                Some(_) => Err("se firmó ad hoc: no se pudo usar la firma local".into()),
                None => Err("uso: telar __firmar <ruta>".into()),
            });
        }
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
    /// La ventana está tapada por completo (otra app encima, minimizada, otro escritorio).
    occluded: bool,
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
    /// El hilo de muestreo mide solo si hace falta (ver `update_stats_needed`).
    stats_on: std::sync::Arc<std::sync::atomic::AtomicBool>,
    stats_detail: Option<statsview::Detail>,
    /// Bandeja de agentes abierta (⌘⇧J).
    inbox: Option<inbox::Inbox>,
    /// Diálogo de confirmación abierto (eliminar un grupo).
    confirm: Option<confirm::Confirm>,
    /// Enlace bajo el puntero que se abriría con un clic (su celda, para resaltarlo) y el
    /// enlace donde empezó el clic: se abre al soltar si no se arrastró para seleccionar.
    link_cell: Option<(usize, usize)>,
    link_press: Option<(links::Link, (f32, f32))>,
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
    /// Próxima búsqueda de versión nueva en segundo plano.
    next_update_check: Instant,
    /// Uso de Claude de cada cuenta (clave de `cuentas`) y el último error de su consulta.
    claude_use: HashMap<String, claude_usage::Usage>,
    usage_errors: HashMap<String, String>,
    /// Hay una consulta en curso, y cuándo empezó la última (solo la activa, y todas).
    usage_fetching: bool,
    usage_fetched: Option<Instant>,
    usage_fetched_all: Option<Instant>,
    claude_account: Option<claude_usage::Account>,
    /// Cuentas de Claude guardadas (para cambiar de cuenta) y la clave de la activa.
    accounts: Vec<cuentas::Saved>,
    account_key: Option<String>,
    /// Minuto que muestra la cuenta regresiva del reinicio: se repinta al cambiar.
    claude_minute: u64,
    /// Pedido de salir desde fuera del teclado (p. ej. «Reiniciar» tras actualizar).
    quit: bool,
    /// Cuándo redimensionar los tabs ocultos (al terminar de arrastrar el borde).
    resize_pending: Option<Instant>,
    /// Enlaces de la pantalla del tab visible, con la versión de pantalla en que se buscaron.
    links_cache: std::cell::RefCell<Option<(LinksKey, std::rc::Rc<Vec<links::Link>>)>>,
}

/// De qué pantalla son los enlaces guardados: tab, proceso, contenido, desplazamiento y tamaño.
type LinksKey = (u64, u64, u64, usize, usize, usize);

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
            stats_on: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            stats_detail: None,
            inbox: None,
            confirm: None,
            link_cell: None,
            link_press: None,
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
            occluded: false,
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
            next_update_check: Instant::now() + UPDATE_CHECK_EVERY,
            claude_use: claude_usage::load(),
            usage_errors: HashMap::new(),
            usage_fetching: false,
            usage_fetched: None,
            usage_fetched_all: None,
            claude_account: None,
            accounts: Vec::new(),
            account_key: None,
            claude_minute: 0,
            quit: false,
            resize_pending: None,
            links_cache: Default::default(),
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
        state.update_stats_needed();
        stats::spawn(state.proxy.clone(), state.stats_on.clone());
        if state.snapshot.is_none() {
            notify::init(state.proxy.clone());
            if let Some(secs) = std::env::var("TELAR_NOTIFY_TEST").ok().and_then(|s| s.parse().ok()) {
                notify::self_test(Duration::from_secs(secs), state.ws.active_term());
            }
        }
        if state.snapshot.is_none() {
            // Ícono del paquete (si la app se actualizó desde una versión sin él) y el elegido.
            update::ensure_bundle();
            let _ = appicon::apply(&state.config.app_icon, false);
        }
        if state.snapshot.is_none() {
            update::cleanup_leftovers();
            // Una versión nueva puede escuchar eventos de Claude Code que la anterior no registraba.
            match hooks::upgrade_hooks() {
                Ok(added) if !added.is_empty() => log::info!("hooks de Claude agregados: {}", added.join(", ")),
                Ok(_) => {}
                Err(e) => log::warn!("no se pudieron poner al día los hooks: {e}"),
            }
            update::check(state.proxy.clone());
            state.refresh_claude_account();
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
                // Una búsqueda en segundo plano que llega tarde no pisa una instalación en curso.
                let installing = matches!(st.update, update::Status::Downloading(..) | update::Status::Applying(_) | update::Status::Ready(_));
                if installing && matches!(status, update::Status::Available(_) | update::Status::UpToDate) {
                    return;
                }
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
            UserEvent::NotificationClicked(tab) => {
                st.window.focus_window();
                if let Some(id) = tab.filter(|id| st.terms.contains_key(id)) {
                    st.show_tab(id);
                }
                st.window.request_redraw();
                return;
            }
            UserEvent::Picked(picked) => {
                st.on_picked(picked);
                return;
            }
            UserEvent::ClaudeAccount(account) => {
                if account.is_some() {
                    st.claude_account = account;
                    st.window.request_redraw();
                }
                return;
            }
            UserEvent::Accounts(saved, current) => {
                st.accounts = saved;
                st.account_key = current;
                return;
            }
            UserEvent::AccountDone(result) => {
                st.account_done(result);
                return;
            }
            UserEvent::UsageAll(results) => {
                st.usage_fetching = false;
                for (key, r) in results {
                    match r {
                        Ok(u) => {
                            st.claude_use.insert(key.clone(), u);
                            st.usage_errors.remove(&key);
                        }
                        Err(e) => {
                            st.usage_errors.insert(key, e);
                        }
                    }
                }
                claude_usage::save(&st.claude_use);
                // Con el menú de cuentas abierto, sus porcentajes se ponen al día.
                let details: Vec<Option<String>> = st.accounts.iter().map(|a| st.usage_detail(&a.key)).collect();
                if let Some(menu) = st.menu.as_mut() {
                    for item in &mut menu.items {
                        if let Some(Action::SwitchAccount(i)) = item.action {
                            item.detail = details.get(i).cloned().flatten();
                        }
                    }
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
            t.changed();
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
                    // Un título vacío (Claude lo deja así al salir) es como no tener título.
                    t.title = (!title.trim().is_empty()).then_some(title);
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
                // El puntero simulado se vuelve a aplicar sobre lo que ya se escribió en la terminal.
                st.snapshot_hover();
                st.draw();
                event_loop.exit();
                return;
            }
        }
        // Un tab que esperaba a mostrarse (reanudación perezosa) arranca en cuanto se ve.
        if st.snapshot.is_none() {
            st.wake_lazy_active();
        }
        // Mientras algún agente trabaja, el indicador pulsa: se redibuja a ~12 fps.
        // Si nadie trabaja, la app vuelve a dormir hasta el próximo evento.
        if now >= st.next_save {
            st.periodic();
            st.next_save = now + SAVE_TICK;
        }
        let mut wake = Some(st.snapshot.as_ref().map_or(st.next_save, |s| s.at.min(st.next_save)));
        if !st.occluded && st.terms.values().any(|t| t.agent.state == AgentState::Working) {
            if now >= st.next_tick {
                st.window.request_redraw();
                st.next_tick = now + if st.focused { SPINNER_TICK } else { SPINNER_TICK_UNFOCUSED };
            }
            wake = wake.map(|w| w.min(st.next_tick));
        }
        if let Some(at) = st.resize_pending {
            if now >= at {
                st.resize_pending = None;
                st.resize_terms();
            } else {
                wake = wake.map(|w| w.min(at));
            }
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
            st.save_session_final();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(st) = self.state.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                st.renderer.resize(size.width, size.height);
                // Mientras se arrastra el borde, solo el tab visible: redimensionar todos
                // recalcula su historial y le manda SIGWINCH a cada programa. Los demás, al
                // soltar (cuando dejan de llegar cambios de tamaño).
                st.resize_visible();
                st.resize_pending = Some(Instant::now() + RESIZE_SETTLE);
                st.window.request_redraw();
            }
            WindowEvent::Occluded(occluded) => {
                st.occluded = occluded;
                if !occluded {
                    st.window.request_redraw();
                }
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
        l.claude = self.config.show_claude;
        l.meters = self.config.show_meters;
        l
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

    /// Cada pocos segundos: nombre del proceso de cada tab, vigilancia de los agentes,
    /// inactividad, uso de Claude, versiones nuevas y guardado de la sesión.
    fn periodic(&mut self) {
        self.update_stats_needed();
        // La suspensión por inactividad se revisa con este reloj (no depende del muestreo).
        self.enforce_idle();
        // Cada hora se vuelve a buscar una versión nueva: si salió otra después de la que
        // se muestra, el pie ya ofrece la última.
        if Instant::now() >= self.next_update_check && self.snapshot.is_none() {
            self.next_update_check = Instant::now() + UPDATE_CHECK_EVERY;
            self.refresh_claude_account();
            if matches!(self.update, update::Status::Idle | update::Status::UpToDate | update::Status::Available(_)) {
                update::check_quietly(self.proxy.clone());
            }
        }
        // El uso de la cuenta activa se consulta cada minuto; el de las demás, solo al abrir
        // el menú de cuentas (no se están usando).
        if self.usage_fetched.is_some_and(|t| t.elapsed() > Duration::from_secs(60)) {
            self.fetch_usage(false);
        }
        // La cuenta regresiva del reinicio de Claude avanza por minuto.
        if self.config.show_claude {
            let minute = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 60);
            if minute != self.claude_minute {
                self.claude_minute = minute;
                self.window.request_redraw();
            }
        }
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
        changed |= self.unstick_working();
        if changed {
            self.window.request_redraw();
        }
        self.update_awake();
        self.save_session();
    }

    fn draw(&mut self) {
        let empty_usage = claude_usage::Usage::default();
        // Si se borraron grupos o la ventana se achicó, la lista no queda desplazada de más.
        let max = self.list_len().saturating_sub(self.layout().list_rows());
        self.list_scroll = self.list_scroll.min(max);
        let l = self.layout();
        let results = self.search_results();
        let links = self.visible_links();
        let view = View {
            links: &links,
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
            link_cell: self.link_cell,
            claude_usage: self.account_key.as_ref().and_then(|k| self.claude_use.get(k)).unwrap_or(&empty_usage),
            claude_account: self.claude_account.as_ref(),
        };
        let mut ime_cell = ui::draw(&mut self.quads, &mut self.fg_quads, &mut self.fonts, &l, &view);
        if let Some(st) = &self.settings {
            let ctx = settings::Ctx {
                family: &self.config.font_family,
                size: self.font_pt,
                app_icon: &self.config.app_icon,
                perf: self.perf_view(),
                idle: self.idle_view(),
                start_dir: self.config.start_dir.as_deref(),
                show_claude: self.config.show_claude,
                show_meters: self.config.show_meters,
                lazy_restore: self.config.lazy_restore,
                scrollback: self.config.scrollback,
                full_disk: signing::has_full_disk_access(),
            };
            st.draw(&mut self.quads, &mut self.fonts, &l, &ctx);
            ime_cell = None;
        }
        if let (Some(d), Some(sample)) = (&self.stats_detail, &self.stats) {
            let rows = statsview::rows(sample, &self.ws, &self.terms);
            d.draw(&mut self.quads, &mut self.fonts, &l, sample, &rows, self.config.ram_limit_mb);
            ime_cell = None;
        }
        if let Some(b) = &self.inbox {
            let entries = inbox::entries(&self.ws, &self.terms);
            b.draw(&mut self.quads, &mut self.fonts, &l, &entries, self.started.elapsed().as_secs_f32());
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

/// La sesión de Claude `session` tiene conversación guardada y se puede retomar.
///
/// Claude Code avisa su `session_id` al abrirse (hook SessionStart), pero su archivo
/// (`~/.claude/projects/<carpeta>/<id>.jsonl`) recién aparece con el primer mensaje. Un
/// Claude abierto sin escribirle nada deja un id sin conversación: `claude --resume`
/// fallaría con «No conversation found».
fn claude_session_exists(session: &str) -> bool {
    if session.is_empty() || session.contains(['/', '.']) {
        return false;
    }
    let base = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")));
    let Some(projects) = base.map(|b| b.join("projects")) else { return false };
    let file = format!("{session}.jsonl");
    std::fs::read_dir(projects)
        .map(|dirs| dirs.flatten().any(|d| d.path().join(&file).is_file()))
        .unwrap_or(false)
}

/// Retoma en `t` (recién abierto) la sesión de Claude `session`, si tiene conversación;
/// si no, el tab queda en la shell. Devuelve si la retomó.
fn resume_claude(t: &mut Terminal, session: &str, cwd: Option<&Path>) -> bool {
    if !claude_session_exists(session) {
        log::info!("la sesión de Claude {session} no tiene conversación: el tab abre en la shell");
        return false;
    }
    // La shell todavía arranca: lo escrito queda en el buffer del PTY y se ejecuta al estar lista.
    t.write(format!("claude --resume {session}\r").into_bytes());
    // Ya cuenta como tab de Claude aunque su hook SessionStart aún no llegue:
    // si se cierra Telar antes, la sesión no se pierde.
    t.agent.session_id = Some(session.to_string());
    t.agent.cwd = cwd.map(|c| c.display().to_string());
    t.agent.set_state(AgentState::Idle);
    true
}

/// Notificación de macOS que no lleva a ningún tab (ver `notify`).
fn notify_macos(title: &str, body: &str) {
    notify::send(title, body, None);
}
