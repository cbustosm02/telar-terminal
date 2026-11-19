//! Panel de uso de recursos (clic en los medidores de la barra lateral): totales de Telar
//! y el detalle por grupo, tab y proceso. Se actualiza solo con cada muestra (cada 2 s).

use std::collections::HashMap;

use crate::font::Fonts;
use crate::kit::{Kit, Text, icons};
use crate::renderer::Quad;
use crate::stats::{Sample, human_bytes};
use crate::terminal::{AgentState, Terminal};
use crate::theme::{self, Rgba};
use crate::ui::{Layout, Rect};
use crate::workspace::Workspace;

const PANEL_W: f32 = 760.0;
const PANEL_H: f32 = 640.0;
const ROW_H: f32 = 28.0;

/// Color de un medidor según su nivel (0–1): verde, ámbar, rojo.
pub fn level_color(level: f32, warn: f32, crit: f32) -> Rgba {
    let th = theme::t();
    if level >= crit {
        th.danger
    } else if level >= warn {
        th.attention
    } else {
        th.done
    }
}

/// Umbrales (ámbar, rojo) de cada medidor: CPU y GPU en % de la máquina; RAM en
/// fracción de la memoria del Mac.
pub const THRESHOLDS: [(f32, f32); 3] = [(0.5, 0.8), (0.25, 0.5), (0.5, 0.8)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Outside,
    Inside,
    Close,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    App,
    Group,
    Tab { icon: char, color: Rgba },
    Proc,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub kind: Kind,
    /// Pasado de su límite de memoria (grupos).
    pub over: bool,
    pub name: String,
    pub detail: String,
    /// % de un núcleo.
    pub cpu: f32,
    pub mem: u64,
}

/// Filas del detalle: la interfaz de Telar, cada grupo con sus tabs y los procesos de
/// cada tab, y al final lo que Telar lanzó fuera de los tabs (p. ej. caffeinate).
pub fn rows(sample: &Sample, ws: &Workspace, terms: &HashMap<u64, Terminal>) -> Vec<Row> {
    let mut out = Vec::new();
    let Some(app) = sample.procs.first() else { return out };
    out.push(Row {
        over: false,
        kind: Kind::App,
        name: "Telar (interfaz)".into(),
        detail: format!("pid {} · GPU {:.0} %", app.pid, sample.gpu),
        cpu: app.cpu,
        mem: app.mem,
    });
    let mut claimed = vec![app.pid];
    for g in &ws.groups {
        let start = out.len();
        out.push(Row { kind: Kind::Group, over: false, name: g.name.clone(), detail: String::new(), cpu: 0.0, mem: 0 });
        let (mut gcpu, mut gmem) = (0.0, 0);
        for (i, id) in g.tabs.iter().enumerate() {
            let Some(t) = terms.get(id) else { continue };
            let root = t.foreground_pid().and_then(|pid| sample.tab_root(pid));
            // Los que no se pueden leer (el `login` de root entre Telar y la shell) no se listan.
            let procs: Vec<_> = root.map(|r| sample.subtree(r)).unwrap_or_default();
            claimed.extend(procs.iter().map(|p| p.pid));
            let procs: Vec<_> = procs.into_iter().filter(|p| p.readable).collect();
            let (cpu, mem) = (procs.iter().map(|p| p.cpu).sum::<f32>(), procs.iter().map(|p| p.mem).sum::<u64>());
            gcpu += cpu;
            gmem += mem;
            let th = theme::t();
            let (icon, color, state) = if t.is_suspended() {
                (icons::MOON, th.text_dim, "suspendido".to_string())
            } else if t.agent.state != AgentState::None {
                let state = match t.agent.state {
                    AgentState::Working => "Claude trabajando",
                    AgentState::NeedsYou => "Claude te necesita",
                    AgentState::Done => "Claude terminó",
                    _ => "Claude",
                };
                (icons::SPARKLES, th.accent, state.to_string())
            } else {
                // Shell: la carpeta donde está.
                let dir = t.cwd().and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()));
                (icons::TERMINAL, th.text_dim, dir.unwrap_or_else(|| "shell".into()))
            };
            let name = t.label().map(String::from).unwrap_or_else(|| format!("Tab {}", i + 1));
            out.push(Row { kind: Kind::Tab { icon, color }, over: false, name, detail: state, cpu, mem });
            let mut procs = procs;
            procs.sort_by(|a, b| b.mem.cmp(&a.mem));
            out.extend(procs.into_iter().map(|p| Row {
                over: false,
                kind: Kind::Proc,
                name: p.name.clone(),
                detail: format!("pid {}", p.pid),
                cpu: p.cpu,
                mem: p.mem,
            }));
        }
        out[start].cpu = gcpu;
        out[start].mem = gmem;
        out[start].over = g.ram_limit_mb > 0 && gmem > g.ram_limit_mb * 1024 * 1024;
        out[start].detail = if g.ram_limit_mb > 0 {
            format!("{} tabs · límite {}", g.tabs.len(), crate::limits::label(g.ram_limit_mb))
        } else {
            format!("{} tabs", g.tabs.len())
        };
    }
    let others: Vec<_> = sample.procs.iter().filter(|p| p.readable && !claimed.contains(&p.pid)).collect();
    if !others.is_empty() {
        out.push(Row {
            over: false,
            kind: Kind::Group,
            name: "Otros procesos de Telar".into(),
            detail: format!("{}", others.len()),
            cpu: others.iter().map(|p| p.cpu).sum(),
            mem: others.iter().map(|p| p.mem).sum(),
        });
        out.extend(others.iter().map(|p| Row { over: false, kind: Kind::Proc, name: p.name.clone(), detail: format!("pid {}", p.pid), cpu: p.cpu, mem: p.mem }));
    }
    out
}

#[derive(Default)]
pub struct Detail {
    pub scroll: usize,
    pub hover: Option<Hit>,
}

struct Geometry {
    panel: Rect,
    close: Rect,
    cards: [Rect; 4],
    header_y: f32,
    list: Rect,
    rows: usize,
}

fn geometry(l: &Layout) -> Geometry {
    let s = l.s();
    let (w, h) = ((PANEL_W * s).min(l.w() - 40.0 * s), (PANEL_H * s).min(l.h() - 40.0 * s));
    let panel = Rect { x: (l.w() - w) / 2.0, y: (l.h() - h) / 2.0, w, h };
    let close = Rect { x: panel.x + w - 38.0 * s, y: panel.y + 12.0 * s, w: 26.0 * s, h: 26.0 * s };
    let (cx, cw) = (panel.x + 24.0 * s, w - 48.0 * s);
    let gap = 10.0 * s;
    let card_w = (cw - 3.0 * gap) / 4.0;
    let cy = panel.y + 64.0 * s;
    let cards = std::array::from_fn(|i| Rect { x: cx + i as f32 * (card_w + gap), y: cy, w: card_w, h: 76.0 * s });
    let header_y = cy + 76.0 * s + 22.0 * s;
    let list_y = header_y + 24.0 * s;
    let rows = (((panel.y + h - 16.0 * s - list_y) / (ROW_H * s)).floor() as usize).max(1);
    let list = Rect { x: cx, y: list_y, w: cw, h: rows as f32 * ROW_H * s };
    Geometry { panel, close, cards, header_y, list, rows }
}

impl Detail {
    pub fn hit(&self, x: f32, y: f32, l: &Layout) -> Hit {
        let g = geometry(l);
        if !g.panel.contains(x, y) {
            Hit::Outside
        } else if g.close.contains(x, y) {
            Hit::Close
        } else {
            Hit::Inside
        }
    }

    pub fn scroll_by(&mut self, delta: isize, total: usize, l: &Layout) {
        let max = total.saturating_sub(geometry(l).rows);
        self.scroll = (self.scroll as isize + delta).clamp(0, max as isize) as usize;
    }

    pub fn draw(&self, out: &mut Vec<Quad>, fonts: &mut Fonts, l: &Layout, sample: &Sample, rows: &[Row], ram_limit_mb: u64) {
        let s = l.s();
        let g = geometry(l);
        let th = theme::t();
        let mut k = Kit::new(out, fonts, s);
        let p = g.panel;
        k.rect(Rect { x: 0.0, y: 0.0, w: l.w(), h: l.h() }, theme::alpha([0.0, 0.0, 0.0, 1.0], if th.dark { 0.5 } else { 0.25 }));
        k.panel(p, 12.0);
        k.icon(p.x + 34.0 * s, p.y + 30.0 * s, icons::ACTIVITY, 17.0, th.accent);
        k.text(p.x + 52.0 * s, p.y + 20.0 * s, "Uso de recursos", Text::title(), p.w - 120.0 * s);
        k.text(p.x + 52.0 * s, p.y + 40.0 * s, "Telar y todo lo que corre en sus tabs · se actualiza cada 2 s", Text::small(), p.w - 120.0 * s);
        k.icon_button(g.close, icons::X, 15.0, self.hover == Some(Hit::Close));

        // Tarjetas: CPU, memoria, GPU y procesos.
        let levels = sample.levels();
        let n_tabs = rows.iter().filter(|r| matches!(r.kind, Kind::Tab { .. })).count();
        let cards = [
            ("CPU", format!("{:.0} %", sample.cpu), format!("de {} núcleos", sample.cores), Some(0)),
            (
                "MEMORIA",
                human_bytes(sample.mem),
                if ram_limit_mb > 0 {
                    format!("límite {}", crate::limits::label(ram_limit_mb))
                } else {
                    format!("de {} del Mac", human_bytes(sample.mem_machine))
                },
                Some(1),
            ),
            ("GPU", format!("{:.0} %", sample.gpu), format!("GPU total {:.0} %", sample.gpu_device), Some(2)),
            ("PROCESOS", sample.procs.iter().filter(|p| p.readable).count().to_string(), format!("en {n_tabs} tabs"), None),
        ];
        for (r, (label, value, sub, meter)) in g.cards.iter().zip(cards) {
            k.bordered(*r, 8.0, th.chrome, th.border);
            k.text(r.x + 12.0 * s, r.y + 10.0 * s, label, Text::caption(), r.w - 24.0 * s);
            k.text(r.x + 12.0 * s, r.y + 26.0 * s, &value, Text::title().weight(590), r.w - 24.0 * s);
            k.text(r.x + 12.0 * s, r.y + 48.0 * s, &sub, Text::small(), r.w - 24.0 * s);
            if let Some(i) = meter {
                let (level, warn, crit, fill) = if i == 1 {
                    crate::limits::ram_meter(sample.mem, sample.mem_machine, ram_limit_mb)
                } else {
                    (levels[i], THRESHOLDS[i].0, THRESHOLDS[i].1, levels[i])
                };
                let bar = Rect { x: r.x + 12.0 * s, y: r.y + r.h - 8.0 * s, w: r.w - 24.0 * s, h: 3.0 * s };
                k.rounded(bar, 1.5, th.border);
                k.rounded(Rect { w: (bar.w * fill.clamp(0.0, 1.0)).max(bar.h), ..bar }, 1.5, level_color(level, warn, crit));
            }
        }

        // Tabla.
        let (lx, lw) = (g.list.x, g.list.w);
        let col_mem = lx + lw - 10.0 * s;
        let col_cpu = col_mem - 110.0 * s;
        let right = |k: &mut Kit, x: f32, y: f32, text: &str, style: Text| {
            let w = k.measure(text, style);
            k.text(x - w, y, text, style, w + 1.0);
        };
        k.text(lx + 10.0 * s, g.header_y, "TAB / PROCESO", Text::caption(), 200.0 * s);
        right(&mut k, col_cpu, g.header_y, "CPU (1 NÚCLEO)", Text::caption());
        right(&mut k, col_mem, g.header_y, "MEMORIA", Text::caption());
        k.rect(Rect { x: lx, y: g.list.y - 6.0 * s, w: lw, h: s }, th.border);

        for (i, row) in rows.iter().enumerate().skip(self.scroll).take(g.rows) {
            let y = g.list.y + (i - self.scroll) as f32 * ROW_H * s;
            let r = Rect { x: lx, y, w: lw, h: ROW_H * s - 2.0 * s };
            let ty = r.y + (r.h - k.line_h(Text::body())) / 2.0;
            let (indent, style, icon): (f32, Text, Option<(char, Rgba)>) = match &row.kind {
                Kind::App => (0.0, Text::label(), Some((icons::TERMINAL, th.accent))),
                Kind::Group => {
                    k.rounded(r, 6.0, th.hover);
                    (0.0, Text::label(), Some((icons::FOLDER, th.text_dim)))
                }
                Kind::Tab { icon, color } => (18.0, Text::body(), Some((*icon, *color))),
                Kind::Proc => (40.0, Text::small().color(theme::mix(th.text, th.text_dim, 0.4)), None),
            };
            let mut x = r.x + 10.0 * s + indent * s;
            if let Some((ic, color)) = icon {
                k.icon(x + 7.0 * s, r.y + r.h / 2.0, ic, 14.0, color);
                x += 22.0 * s;
            }
            let name_w = k.text(x, ty, &row.name, style, (col_cpu - 140.0 * s - x).max(40.0 * s));
            if !row.detail.is_empty() {
                k.text(x + name_w + 8.0 * s, ty + 1.0 * s, &row.detail, Text::small(), (col_cpu - 90.0 * s - x - name_w).max(0.0));
            }
            let cpu_color = level_color(row.cpu / 100.0, 0.5, 0.9);
            let cpu_text = format!("{:.1} %", row.cpu.max(0.0) + 0.0).replace('.', ",");
            right(&mut k, col_cpu, ty, &cpu_text, Text::body().color(if row.cpu >= 50.0 { cpu_color } else { style.color }));
            let mem = if row.mem == 0 && matches!(row.kind, Kind::Tab { .. }) { "—".to_string() } else { human_bytes(row.mem) };
            right(&mut k, col_mem, ty, &mem, Text::body().color(if row.over { th.danger } else { style.color }));
        }
        if rows.len() > g.rows {
            let track = g.list;
            let h = (track.h * g.rows as f32 / rows.len() as f32).max(24.0 * s);
            let y = track.y + (track.h - h) * self.scroll as f32 / (rows.len() - g.rows) as f32;
            k.rounded(Rect { x: track.x + track.w - 3.0 * s, y, w: 3.0 * s, h }, 1.5, th.border);
        }
    }
}
