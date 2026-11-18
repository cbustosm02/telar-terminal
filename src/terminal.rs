//! Una terminal = emulador VT (alacritty_terminal) + PTY con su shell.
//!
//! alacritty_terminal ya trae el PTY y un hilo lector (`EventLoop`), así que no
//! hace falta portable-pty. Cada terminal tiene su propio hilo de lectura; la
//! UI solo se entera por eventos `UserEvent::Term`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener, Notify, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;
use winit::event_loop::EventLoopProxy;

use crate::UserEvent;

#[derive(Clone)]
pub struct Listener {
    id: u64,
    proxy: EventLoopProxy<UserEvent>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.proxy.send_event(UserEvent::Term(self.id, event));
    }
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
}

impl Default for Agent {
    fn default() -> Self {
        Self { state: AgentState::None, since: Instant::now(), session_id: None, cwd: None, message: None }
    }
}

pub struct Terminal {
    pub term: Arc<FairMutex<Term<Listener>>>,
    notifier: Notifier,
    pub title: Option<String>,
    /// Sonó la campana mientras el tab no estaba visible.
    pub attention: bool,
    pub agent: Agent,
    size: GridSize,
}

impl Terminal {
    pub fn spawn(id: u64, size: GridSize, proxy: EventLoopProxy<UserEvent>, socket: Option<&Path>) -> std::io::Result<Self> {
        let listener = Listener { id, proxy };
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
            working_directory: std::env::var_os("HOME").map(Into::into),
            drain_on_exit: false,
            env,
        };
        let config = Config { scrolling_history: 10_000, ..Default::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));
        let pty = tty::new(&options, size.window_size(), id)?;
        let event_loop = EventLoop::new(term.clone(), listener, pty, false, false)?;
        let notifier = Notifier(event_loop.channel());
        event_loop.spawn();
        Ok(Self { term, notifier, title: None, attention: false, agent: Agent::default(), size })
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.notifier.notify(bytes);
    }

    pub fn resize(&mut self, size: GridSize) {
        if size == self.size {
            return;
        }
        self.size = size;
        let _ = self.notifier.0.send(Msg::Resize(size.window_size()));
        self.term.lock().resize(size);
    }

    pub fn size(&self) -> GridSize {
        self.size
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Cierra el hilo lector; al soltar el PTY la shell recibe SIGHUP.
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}
