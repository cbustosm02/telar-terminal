//! Una terminal = emulador VT (alacritty_terminal) + PTY con su shell.
//!
//! alacritty_terminal ya trae el PTY y un hilo lector (`EventLoop`), así que no
//! hace falta portable-pty. Cada terminal tiene su propio hilo de lectura; la
//! UI solo se entera por eventos `UserEvent::Term`.
//!
//! Una terminal suspendida no tiene PTY ni proceso: solo conserva su pantalla
//! (reconstruida desde `screen::to_ansi`) para la vista previa.

use std::borrow::Cow;
use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener, Notify, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;
use winit::event_loop::EventLoopProxy;

use crate::UserEvent;

/// Cada terminal que se crea (también al reanudar un tab, que conserva su id) tiene
/// una generación distinta. Así los eventos tardíos del proceso anterior se descartan.
static GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct Listener {
    id: u64,
    generation: u64,
    proxy: EventLoopProxy<UserEvent>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.proxy.send_event(UserEvent::Term(self.id, self.generation, event));
    }
}

impl Listener {
    fn new(id: u64, proxy: EventLoopProxy<UserEvent>) -> Self {
        Self { id, generation: GENERATION.fetch_add(1, Ordering::Relaxed), proxy }
    }
}

/// Lo que guarda un tab suspendido para poder reanudarlo.
pub struct Frozen {
    pub cwd: Option<PathBuf>,
    /// Pantalla como texto ANSI (también se guarda en la sesión).
    pub screen: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GridSize {
    pub cols: usize,
    pub rows: usize,
    pub cell_w: u16,
    pub cell_h: u16,
}

impl GridSize {
    pub fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.rows as u16,
            num_cols: self.cols as u16,
            cell_width: self.cell_w,
            cell_height: self.cell_h,
        }
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Estado de una sesión de Claude Code en el tab, según sus hooks.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AgentState {
    /// Shell normal, sin sesión de Claude.
    #[default]
    None,
    /// Hay sesión y está esperando tu prompt (ya la viste).
    Idle,
    Working,
    /// Pide un permiso o una respuesta.
    NeedsYou,
    /// Terminó y todavía no la has visto.
    Done,
}

impl AgentState {
    pub fn name(self) -> &'static str {
        match self {
            AgentState::None => "shell",
            AgentState::Idle => "idle",
            AgentState::Working => "working",
            AgentState::NeedsYou => "needs-you",
            AgentState::Done => "done",
        }
    }
}

#[derive(Debug)]
pub struct Agent {
    pub state: AgentState,
    pub since: Instant,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub message: Option<String>,
    /// Cuántas veces cambió de estado (lo usa `telar wait`).
    pub changes: u64,
}

impl Default for Agent {
    fn default() -> Self {
        Self { state: AgentState::None, since: Instant::now(), session_id: None, cwd: None, message: None, changes: 0 }
    }
}

impl Agent {
    pub fn set_state(&mut self, state: AgentState) -> bool {
        if state == self.state {
            return false;
        }
        self.state = state;
        self.since = Instant::now();
        self.changes += 1;
        true
    }
}

pub struct Terminal {
    pub term: Arc<FairMutex<Term<Listener>>>,
    /// `None` si está suspendida.
    notifier: Option<Notifier>,
    pub generation: u64,
    pub frozen: Option<Frozen>,
    pub title: Option<String>,
    /// Nombre puesto por el usuario (gana sobre el título que pone el programa).
    pub name: Option<String>,
    /// Sonó la campana mientras el tab no estaba visible.
    pub attention: bool,
    pub agent: Agent,
    /// Nombre del proceso en primer plano (se refresca cada pocos segundos).
    pub process: Option<String>,
    /// Copia del descriptor del PTY, para preguntar por el proceso en primer plano.
    pty: Option<OwnedFd>,
    size: GridSize,
}

impl Terminal {
    pub fn spawn(
        id: u64,
        size: GridSize,
        proxy: EventLoopProxy<UserEvent>,
        socket: Option<&Path>,
        cwd: Option<&Path>,
    ) -> std::io::Result<Self> {
        let listener = Listener::new(id, proxy);
        let mut env = HashMap::from([
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
            ("TERM_PROGRAM".to_string(), "telar".to_string()),
            ("TELAR_TAB_ID".to_string(), id.to_string()),
        ]);
        // Con esto los hooks de Claude Code (y la CLI) encuentran a este Telar y saben de qué tab vienen.
        if let Some(socket) = socket {
            env.insert("TELAR_SOCKET".into(), socket.display().to_string());
        }
        if let Ok(bin) = std::env::current_exe() {
            env.insert("TELAR_BIN".into(), bin.display().to_string());
        }
        let options = tty::Options {
            shell: None, // shell de login del usuario
            working_directory: cwd
                .filter(|d| d.is_dir())
                .map(Path::to_path_buf)
                .or_else(|| std::env::var_os("HOME").map(Into::into)),
            drain_on_exit: false,
            env,
        };
        let config = Config { scrolling_history: 10_000, ..Default::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));
        let pty = tty::new(&options, size.window_size(), id)?;
        let pty_fd = pty.file().try_clone().ok().map(OwnedFd::from);
        let listener_generation = listener.generation;
        let event_loop = EventLoop::new(term.clone(), listener, pty, false, false)?;
        let notifier = Notifier(event_loop.channel());
        event_loop.spawn();
        Ok(Self {
            term,
            notifier: Some(notifier),
            generation: listener_generation,
            frozen: None,
            title: None,
            name: None,
            attention: false,
            agent: Agent::default(),
            process: None,
            pty: pty_fd,
            size,
        })
    }

    /// Terminal suspendida: sin proceso, con la pantalla guardada en `frozen.screen`.
    pub fn frozen(id: u64, size: GridSize, proxy: EventLoopProxy<UserEvent>, frozen: Frozen) -> Self {
        let listener = Listener::new(id, proxy);
        let generation = listener.generation;
        let config = Config { scrolling_history: 0, ..Default::default() };
        let mut term = Term::new(config, &size, listener);
        crate::screen::load(&mut term, &frozen.screen);
        Self {
            term: Arc::new(FairMutex::new(term)),
            notifier: None,
            generation,
            frozen: Some(frozen),
            title: None,
            name: None,
            attention: false,
            agent: Agent::default(),
            process: None,
            pty: None,
            size,
        }
    }

    /// Nombre a mostrar: el del usuario, el título del programa o el proceso.
    pub fn label(&self) -> Option<&str> {
        self.name.as_deref().or(self.title.as_deref()).or(self.process.as_deref())
    }

    pub fn is_suspended(&self) -> bool {
        self.frozen.is_some()
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        if let Some(n) = &self.notifier {
            n.notify(bytes);
        }
    }

    pub fn resize(&mut self, size: GridSize) {
        if size == self.size {
            return;
        }
        self.size = size;
        if let Some(n) = &self.notifier {
            let _ = n.0.send(Msg::Resize(size.window_size()));
        }
        self.term.lock().resize(size);
    }

    pub fn size(&self) -> GridSize {
        self.size
    }

    fn foreground(&self) -> Option<i32> {
        self.pty.as_ref().and_then(crate::proc::foreground)
    }

    /// Carpeta actual del proceso en primer plano (la shell, o Claude si está corriendo).
    pub fn cwd(&self) -> Option<PathBuf> {
        if let Some(f) = &self.frozen {
            return f.cwd.clone();
        }
        self.foreground().and_then(crate::proc::cwd)
    }

    /// Proceso en primer plano del tab (su grupo de procesos). Para saber qué procesos
    /// son del tab se sube desde él hasta la shell (ver `Sample::tab_root`).
    pub fn foreground_pid(&self) -> Option<i32> {
        self.foreground()
    }

    pub fn refresh_process(&mut self) {
        self.process = self.foreground().and_then(crate::proc::name);
    }

    /// Texto de las últimas `lines` líneas lógicas (las líneas partidas por ancho se unen).
    pub fn text(&self, lines: usize) -> String {
        grid_text(&self.term.lock(), lines)
    }
}

/// Texto de las últimas `lines` líneas lógicas de `term` (pantalla y scrollback).
pub fn grid_text<L: EventListener>(term: &Term<L>, lines: usize) -> String {
    {
        let grid = term.grid();
        let cols = grid.columns();
        let top = grid.topmost_line().0;
        // Última fila con texto (o la del cursor): en una pantalla recién abierta
        // el contenido está arriba y el resto son filas vacías.
        let blank = |line: i32| (0..cols).all(|c| grid[Line(line)][Column(c)].c == ' ');
        let mut bottom = grid.bottommost_line().0;
        while bottom > grid.cursor.point.line.0 && bottom > top && blank(bottom) {
            bottom -= 1;
        }
        let start = (bottom - (lines as i32) * 4).max(top);
        let skip = Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER;
        let mut out: Vec<String> = Vec::new();
        let mut current = String::new();
        for line in start..=bottom {
            let row = &grid[Line(line)];
            for col in 0..cols {
                let cell = &row[Column(col)];
                if cell.flags.intersects(skip) {
                    continue;
                }
                current.push(cell.c);
                if let Some(extra) = cell.zerowidth() {
                    current.extend(extra);
                }
            }
            if !row[Column(cols - 1)].flags.contains(Flags::WRAPLINE) {
                out.push(current.trim_end().to_string());
                current.clear();
            }
        }
        if !current.is_empty() {
            out.push(current.trim_end().to_string());
        }
        while out.last().is_some_and(String::is_empty) {
            out.pop();
        }
        let from = out.len().saturating_sub(lines);
        out[from..].join("\n")
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Cierra el hilo lector; al soltar el PTY la shell recibe SIGHUP.
        if let Some(n) = &self.notifier {
            let _ = n.0.send(Msg::Shutdown);
        }
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;

    use super::*;

    fn term(cols: usize, rows: usize, input: &str) -> Term<VoidListener> {
        let size = GridSize { cols, rows, cell_w: 8, cell_h: 16 };
        let mut t = Term::new(Config { scrolling_history: 100, ..Default::default() }, &size, VoidListener);
        crate::screen::load(&mut t, input);
        t
    }

    #[test]
    fn ultimas_lineas_sin_filas_vacias_al_final() {
        let t = term(20, 10, "uno\r\ndos\r\ntres");
        assert_eq!(grid_text(&t, 2), "dos\ntres");
        assert_eq!(grid_text(&t, 50), "uno\ndos\ntres");
    }

    #[test]
    fn une_las_lineas_partidas_por_ancho() {
        // 25 caracteres en 10 columnas: una línea lógica en tres filas.
        let t = term(10, 6, "abcdefghijklmnopqrstuvwxy\r\nfin");
        assert_eq!(grid_text(&t, 2), "abcdefghijklmnopqrstuvwxy\nfin");
    }

    #[test]
    fn incluye_el_scrollback() {
        let lines: Vec<String> = (1..=30).map(|i| i.to_string()).collect();
        let t = term(10, 5, &lines.join("\r\n"));
        assert_eq!(grid_text(&t, 30).lines().count(), 30);
        assert_eq!(grid_text(&t, 3), "28\n29\n30");
    }

    #[test]
    fn caracteres_anchos_sin_espacios_de_relleno() {
        let t = term(20, 3, "日本語 ok");
        assert_eq!(grid_text(&t, 1), "日本語 ok");
    }

    #[test]
    fn estados_del_agente() {
        let mut a = Agent::default();
        assert_eq!(a.state.name(), "shell");
        assert!(a.set_state(AgentState::Working));
        assert!(!a.set_state(AgentState::Working), "mismo estado: no cuenta como cambio");
        assert!(a.set_state(AgentState::Done));
        assert_eq!(a.changes, 2);
        assert_eq!(a.state.name(), "done");
    }
}
