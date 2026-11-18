//! Telar: terminal personal con grupos de tabs en una barra lateral.

mod font;
mod input;
mod renderer;
mod terminal;
mod theme;
mod ui;
mod workspace;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::Event as TermEvent;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::color::COUNT;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::Rgb;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::macos::WindowAttributesExtMacOS;
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{CursorIcon, Window, WindowId};

use crate::font::Fonts;
use crate::renderer::{Quad, Renderer};
use crate::terminal::Terminal;
use crate::theme::Rgba;
use crate::ui::{Hit, Layout, View};
use crate::workspace::Workspace;

const FONT_PT: f32 = 13.0;
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

pub enum UserEvent {
    Term(u64, TermEvent),
}

fn main() {
    env_logger::init();
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App { proxy: event_loop.create_proxy(), state: None };
    event_loop.run_app(&mut app).expect("event loop");
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
    editing: Option<(usize, String)>,
    preedit: String,
    focused: bool,
    clipboard: Option<arboard::Clipboard>,
    quads: Vec<Quad>,
    fg_quads: Vec<Quad>,
    snapshot: Option<Snapshot>,
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
            .with_title_hidden(true);
        let window = Arc::new(event_loop.create_window(attrs).expect("ventana"));
        window.set_ime_allowed(true);
        let renderer = pollster::block_on(Renderer::new(window.clone()));
        let scale = window.scale_factor() as f32;
        let mut state = State {
            fonts: Fonts::new(FONT_PT * scale),
            font_pt: FONT_PT,
            palette: theme::palette(),
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
            preedit: String::new(),
            focused: true,
            clipboard: arboard::Clipboard::new().ok(),
            quads: Vec::new(),
            fg_quads: Vec::new(),
            snapshot: std::env::var_os("TELAR_SNAPSHOT").map(|p| Snapshot {
                path: p.into(),
                at: Instant::now() + Duration::from_millis(2500),
                capture_now: false,
            }),
        };
        state.new_group();
        if state.snapshot.is_some() {
            state.snapshot_setup();
        }
        self.state = Some(state);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, UserEvent::Term(id, event): UserEvent) {
        let Some(st) = self.state.as_mut() else { return };
        let visible = st.ws.active_term() == Some(id);
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
        let Some(snap) = st.snapshot.as_mut() else { return };
        if Instant::now() >= snap.at {
            snap.capture_now = true;
            st.draw();
            event_loop.exit();
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(snap.at));
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
                st.window.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => st.mods = m.state(),
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
            WindowEvent::MouseInput { state, button, .. } => st.on_mouse(state, button),
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
        Layout::new(size.width as f32, size.height as f32, self.window.scale_factor() as f32, &self.fonts, n_tabs)
    }

    fn spawn_term(&mut self) -> Option<u64> {
        let id = self.next_id;
        self.next_id += 1;
        match Terminal::spawn(id, self.layout().grid_size(), self.proxy.clone()) {
            Ok(t) => {
                self.terms.insert(id, t);
                Some(id)
            }
            Err(e) => {
                log::error!("no se pudo abrir la shell: {e}");
                None
            }
        }
    }

    fn new_group(&mut self) {
        if let Some(id) = self.spawn_term() {
            self.ws.add_group(id);
            self.window.request_redraw();
        }
    }

    fn new_tab(&mut self) {
        if self.ws.groups.is_empty() {
            return self.new_group();
        }
        if let Some(id) = self.spawn_term() {
            self.ws.add_tab(id);
            self.window.request_redraw();
        }
    }

    /// Cierra una terminal. Devuelve `true` si ya no queda ninguna (hay que salir).
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
            self.editing = Some((group, g.name.clone()));
            self.window.request_redraw();
        }
    }

    /// Devuelve `true` si hay que cerrar la app.
    fn on_key(&mut self, event: &KeyEvent) -> bool {
        let m = self.mods;

        // Renombrando un grupo: el teclado edita el nombre.
        if let Some((gi, buf)) = self.editing.as_mut() {
            match &event.logical_key {
                Key::Named(NamedKey::Enter) => {
                    let name = buf.trim().to_string();
                    let gi = *gi;
                    if !name.is_empty() {
                        if let Some(g) = self.ws.groups.get_mut(gi) {
                            g.name = name;
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
            "t" => self.new_tab(),
            "n" => self.new_group(),
            "w" => {
                if let Some(id) = self.ws.active_term() {
                    return self.close_term(id);
                }
            }
            "q" => return true,
            "c" => self.copy_selection(),
            "v" => self.paste(),
            "r" if m.shift_key() => self.start_rename(self.ws.active),
            "=" | "+" => {
                self.font_pt = (self.font_pt + 1.0).min(36.0);
                self.apply_font_size();
            }
            "-" => {
                self.font_pt = (self.font_pt - 1.0).max(8.0);
                self.apply_font_size();
            }
            "0" => {
                self.font_pt = FONT_PT;
                self.apply_font_size();
            }
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
        if let Some(t) = self.ws.active_term().and_then(|id| self.terms.get_mut(&id)) {
            t.attention = false;
        }
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
                if let Some((_, buf)) = self.editing.as_mut() {
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
        let l = self.layout();
        let n_tabs = self.ws.group().map_or(0, |g| g.tabs.len());
        let hit = l.hit(self.cursor.0, self.cursor.1, self.ws.groups.len(), n_tabs);
        if hit != self.hover {
            self.hover = hit;
            let icon = match hit {
                Hit::Terminal => CursorIcon::Text,
                Hit::Group(_) | Hit::NewGroup | Hit::Tab(_) | Hit::CloseTab(_) | Hit::NewTab => CursorIcon::Pointer,
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

        if !pressed {
            if let Some(b) = self.mouse_button.take() {
                let (col, row, _) = l.cell_at(self.cursor.0, self.cursor.1);
                self.mouse_report(b, false, col, row, self.active_mode());
            }
            self.selecting = false;
            return;
        }

        // Conteo de clics para doble/triple clic.
        let now = Instant::now();
        let count = if self.clicks.1 == hit && now - self.clicks.0 < DOUBLE_CLICK { self.clicks.2 % 3 + 1 } else { 1 };
        self.clicks = (now, hit, count);

        if hit != Hit::Terminal && button == MouseButton::Left {
            if !matches!(hit, Hit::Group(i) if self.editing.as_ref().is_some_and(|(g, _)| *g == i)) {
                self.editing = None;
            }
        }

        match (hit, button) {
            (Hit::Group(i), MouseButton::Left) => {
                if count >= 2 {
                    self.start_rename(i);
                } else if self.ws.active != i {
                    self.ws.select_group(i);
                    self.after_switch();
                }
            }
            (Hit::NewGroup, MouseButton::Left) => self.new_group(),
            (Hit::Tab(i), MouseButton::Left) => {
                self.ws.select_tab(i);
                self.after_switch();
            }
            (Hit::Tab(i), MouseButton::Middle) | (Hit::CloseTab(i), MouseButton::Left) => {
                if let Some(id) = self.ws.group().and_then(|g| g.tabs.get(i).copied()) {
                    self.close_term(id);
                    self.resize_terms();
                }
            }
            (Hit::NewTab, MouseButton::Left) => self.new_tab(),
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
        let l = self.layout();
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

    fn draw(&mut self) {
        let l = self.layout();
        let view = View {
            ws: &self.ws,
            terms: &self.terms,
            hover: self.hover,
            editing: self.editing.as_ref().map(|(g, s)| (*g, s.as_str())),
            preedit: &self.preedit,
            focused: self.focused,
            palette: &self.palette,
        };
        let ime_cell = ui::draw(&mut self.quads, &mut self.fg_quads, &mut self.fonts, &l, &view);
        if let Some((col, row)) = ime_cell {
            let (x, y) = l.cell_origin(col, row);
            self.window.set_ime_cursor_area(
                PhysicalPosition::new(x as f64, y as f64),
                PhysicalSize::new(l.cell_w as f64, l.cell_h as f64),
            );
        }
        let bg = theme::hex(theme::TERM_BG);
        let capture = self.snapshot.as_ref().filter(|s| s.capture_now).map(|s| s.path.clone());
        self.renderer.render(&self.quads, bg, &mut self.fonts, capture.as_deref());
        if std::mem::take(&mut self.fonts.atlas_reset) {
            self.window.request_redraw();
        }
    }
}
