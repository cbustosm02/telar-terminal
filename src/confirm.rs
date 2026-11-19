//! Diálogo de confirmación: eliminar un grupo o quitar una cuenta de Claude guardada.
//! Enter o el botón rojo confirman; Esc, «Cancelar» o un clic fuera lo cierran.

use crate::font::Fonts;
use crate::kit::{Kit, Text, Variant, icons};
use crate::renderer::Quad;
use crate::theme;
use crate::ui::{Layout, Rect};

const PANEL_W: f32 = 420.0;
const LINE_H: f32 = 18.0;
const BUTTON_H: f32 = 30.0;

/// Lo que se hace al confirmar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pending {
    /// Eliminar el grupo que contiene este tab (por tab y no por índice: si mientras
    /// tanto se abre o se mueve un grupo, se elimina igual el que se eligió).
    DeleteGroup { anchor: u64 },
    /// Quitar la copia guardada de una cuenta.
    RemoveAccount { key: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Outside,
    Inside,
    Cancel,
    Confirm,
}

pub struct Confirm {
    pub title: String,
    pub lines: Vec<String>,
    pub confirm: &'static str,
    /// Ícono junto al título y si el botón es rojo (algo que no se deshace).
    pub icon: char,
    pub danger: bool,
    pub pending: Pending,
    pub hover: Option<Hit>,
}

struct Geometry {
    panel: Rect,
    cancel: Rect,
    confirm: Rect,
}

/// Cantidad con su palabra en singular o plural: «1 tab», «3 tabs».
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Texto del diálogo para eliminar un grupo: `tabs` en total, `claude` con una sesión de
/// Claude y `working` donde Claude trabaja ahora.
pub fn delete_group(name: &str, tabs: usize, claude: usize, working: usize, anchor: u64) -> Confirm {
    let mut lines = vec![if tabs == 1 {
        "Se cierra su tab y lo que corre en él.".to_string()
    } else {
        format!("Se cierran sus {} y lo que corre en ellos.", count(tabs, "tab", "tabs"))
    }];
    if working > 0 {
        lines.push(format!("Claude está trabajando en {}.", count(working, "tab", "tabs")));
    }
    if claude > 0 {
        lines.push("Las sesiones de Claude se pueden retomar con claude --resume.".into());
    }
    Confirm {
        title: format!("¿Eliminar el grupo «{name}»?"),
        lines,
        confirm: "Eliminar grupo",
        icon: icons::TRASH,
        danger: true,
        pending: Pending::DeleteGroup { anchor },
        hover: None,
    }
}

/// Quitar la copia guardada de la cuenta `email`.
pub fn remove_account(email: &str, plan: Option<&str>, key: String) -> Confirm {
    Confirm {
        // Con el plan: el mismo correo puede tener dos cuentas (personal y de equipo).
        title: match plan {
            Some(p) => format!("¿Quitar {email} ({p})?"),
            None => format!("¿Quitar {email}?"),
        },
        lines: vec![
            "Telar borra la copia guardada de esta cuenta.".into(),
            "Para volver a usarla, inicia sesión con ella y agrégala otra vez.".into(),
        ],
        confirm: "Quitar cuenta",
        icon: icons::USER_X,
        danger: true,
        pending: Pending::RemoveAccount { key },
        hover: None,
    }
}

impl Confirm {
    fn geometry(&self, l: &Layout, k: &mut Kit) -> Geometry {
        let s = l.s();
        let h = (24.0 + 22.0 + 8.0 + self.lines.len() as f32 * LINE_H + 22.0 + BUTTON_H + 20.0) * s;
        let w = (PANEL_W * s).min(l.w() - 40.0 * s);
        let panel = Rect { x: (l.w() - w) / 2.0, y: (l.h() - h) / 2.0, w, h };
        let by = panel.y + h - (20.0 + BUTTON_H) * s;
        let style = Text::label();
        let cw = k.measure(self.confirm, style) + 28.0 * s;
        let confirm = Rect { x: panel.x + w - 20.0 * s - cw, y: by, w: cw, h: BUTTON_H * s };
        let kw = k.measure("Cancelar", style) + 28.0 * s;
        let cancel = Rect { x: confirm.x - 8.0 * s - kw, y: by, w: kw, h: BUTTON_H * s };
        Geometry { panel, cancel, confirm }
    }

    pub fn hit(&self, x: f32, y: f32, l: &Layout, fonts: &mut Fonts) -> Hit {
        let mut out = Vec::new();
        let g = self.geometry(l, &mut Kit::new(&mut out, fonts, l.s()));
        if g.confirm.contains(x, y) {
            Hit::Confirm
        } else if g.cancel.contains(x, y) {
            Hit::Cancel
        } else if g.panel.contains(x, y) {
            Hit::Inside
        } else {
            Hit::Outside
        }
    }

    pub fn draw(&self, out: &mut Vec<Quad>, fonts: &mut Fonts, l: &Layout) {
        let s = l.s();
        let th = theme::t();
        let mut k = Kit::new(out, fonts, s);
        let g = self.geometry(l, &mut k);
        let p = g.panel;
        k.rect(Rect { x: 0.0, y: 0.0, w: l.w(), h: l.h() }, theme::alpha([0.0, 0.0, 0.0, 1.0], if th.dark { 0.5 } else { 0.25 }));
        k.panel(p, 12.0);
        // Ícono en un círculo suave (rojo si no se deshace), a la izquierda del título.
        let tint = if self.danger { th.danger } else { th.accent };
        let (ix, iy) = (p.x + 38.0 * s, p.y + 36.0 * s);
        k.dot(ix, iy, 30.0, theme::alpha(tint, 0.16));
        k.icon(ix, iy, self.icon, 15.0, tint);
        let tx = p.x + 64.0 * s;
        let tw = p.x + p.w - 20.0 * s - tx;
        k.text(tx, p.y + 26.0 * s, &self.title, Text::title(), tw);
        let mut y = p.y + (24.0 + 22.0 + 8.0) * s;
        for line in &self.lines {
            k.text(tx, y, line, Text::body().color(th.text_dim), tw);
            y += LINE_H * s;
        }
        k.button(g.cancel, "Cancelar", None, Variant::Subtle, self.hover == Some(Hit::Cancel));
        let variant = if self.danger { Variant::Danger } else { Variant::Primary };
        k.button(g.confirm, self.confirm, None, variant, self.hover == Some(Hit::Confirm));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texto_de_eliminar_un_grupo() {
        let c = delete_group("API", 1, 0, 0, 7);
        assert_eq!(c.title, "¿Eliminar el grupo «API»?");
        assert_eq!(c.lines, vec!["Se cierra su tab y lo que corre en él."]);
        assert_eq!(c.pending, Pending::DeleteGroup { anchor: 7 });
        let c = delete_group("Web", 4, 2, 1, 3);
        assert_eq!(
            c.lines,
            vec![
                "Se cierran sus 4 tabs y lo que corre en ellos.",
                "Claude está trabajando en 1 tab.",
                "Las sesiones de Claude se pueden retomar con claude --resume.",
            ]
        );
    }
}
