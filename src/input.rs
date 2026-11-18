//! Teclado → bytes para el PTY (secuencias xterm).
//!
//! Option no se usa como Meta: en el teclado latinoamericano de macOS hace falta
//! para escribir caracteres como @ o #. Solo Option+flechas/borrar se traducen a
//! movimientos por palabra, como en Terminal.app.

use alacritty_terminal::term::TermMode;
use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

/// Parámetro de modificadores de xterm: 1 + shift + 2·alt + 4·ctrl.
fn modifier_param(m: ModifiersState) -> u8 {
    1 + m.shift_key() as u8 + 2 * m.alt_key() as u8 + 4 * m.control_key() as u8
}

pub fn key_to_bytes(event: &KeyEvent, mods: ModifiersState, mode: TermMode) -> Option<Vec<u8>> {
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let m = modifier_param(mods);

    if let Key::Named(named) = &event.logical_key {
        let csi = |final_byte: char| -> Vec<u8> {
            if m > 1 {
                format!("\x1b[1;{m}{final_byte}").into_bytes()
            } else if app_cursor {
                format!("\x1bO{final_byte}").into_bytes()
            } else {
                format!("\x1b[{final_byte}").into_bytes()
            }
        };
        let tilde = |n: u8| -> Vec<u8> {
            if m > 1 { format!("\x1b[{n};{m}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
        };
        let only_alt = mods.alt_key() && !mods.control_key() && !mods.shift_key();
        let bytes = match named {
            // Shift+Enter y Option+Enter mandan ESC CR: Claude Code lo toma como salto de línea.
            NamedKey::Enter if mods.alt_key() || mods.shift_key() => b"\x1b\r".to_vec(),
            NamedKey::Enter => b"\r".to_vec(),
            NamedKey::Tab => if mods.shift_key() { b"\x1b[Z".to_vec() } else { b"\t".to_vec() },
            NamedKey::Backspace => {
                if mods.control_key() {
                    vec![0x08]
                } else if mods.alt_key() {
                    b"\x1b\x7f".to_vec()
                } else {
                    vec![0x7f]
                }
            }
            NamedKey::Escape => b"\x1b".to_vec(),
            NamedKey::Space if mods.control_key() => vec![0],
            NamedKey::Space => return event.text.as_ref().map(|t| t.as_bytes().to_vec()),
            NamedKey::ArrowLeft if only_alt => b"\x1bb".to_vec(),
            NamedKey::ArrowRight if only_alt => b"\x1bf".to_vec(),
            NamedKey::ArrowUp => csi('A'),
            NamedKey::ArrowDown => csi('B'),
            NamedKey::ArrowRight => csi('C'),
            NamedKey::ArrowLeft => csi('D'),
            NamedKey::Home => csi('H'),
            NamedKey::End => csi('F'),
            NamedKey::Insert => tilde(2),
            NamedKey::Delete => tilde(3),
            NamedKey::PageUp => tilde(5),
            NamedKey::PageDown => tilde(6),
            NamedKey::F1 => csi_ss3('P', m),
            NamedKey::F2 => csi_ss3('Q', m),
            NamedKey::F3 => csi_ss3('R', m),
            NamedKey::F4 => csi_ss3('S', m),
            NamedKey::F5 => tilde(15),
            NamedKey::F6 => tilde(17),
            NamedKey::F7 => tilde(18),
            NamedKey::F8 => tilde(19),
            NamedKey::F9 => tilde(20),
            NamedKey::F10 => tilde(21),
            NamedKey::F11 => tilde(23),
            NamedKey::F12 => tilde(24),
            _ => return None,
        };
        return Some(bytes);
    }

    // Ctrl + letra/símbolo → carácter de control, según la tecla sin modificadores.
    if mods.control_key() {
        if let Key::Character(s) = event.key_without_modifiers() {
            if let Some(c) = s.chars().next() {
                let ctrl = match c.to_ascii_lowercase() {
                    c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
                    '@' | ' ' | '2' => Some(0),
                    '[' | '3' => Some(0x1b),
                    '\\' | '4' => Some(0x1c),
                    ']' | '5' => Some(0x1d),
                    '^' | '6' => Some(0x1e),
                    '_' | '-' | '7' => Some(0x1f),
                    '?' | '8' => Some(0x7f),
                    _ => None,
                };
                if let Some(b) = ctrl {
                    return Some(if mods.alt_key() { vec![0x1b, b] } else { vec![b] });
                }
            }
        }
    }

    event.text.as_ref().filter(|t| !t.is_empty()).map(|t| t.as_bytes().to_vec())
}

fn csi_ss3(final_byte: char, m: u8) -> Vec<u8> {
    if m > 1 { format!("\x1b[1;{m}{final_byte}").into_bytes() } else { format!("\x1bO{final_byte}").into_bytes() }
}

/// Texto pegado, con bracketed paste si la aplicación lo pidió.
pub fn paste_bytes(text: &str, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let clean = text.replace('\x1b', "");
        format!("\x1b[200~{clean}\x1b[201~").into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}
