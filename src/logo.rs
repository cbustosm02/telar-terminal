//! Logo de Claude (el destello naranja) para los tabs donde corre Claude Code. Se dibuja
//! por código al tamaño exacto en píxeles: rayos con punta redondeada que salen del
//! centro, de largos un poco desparejos. Así no hace falta incluir una imagen y se ve
//! nítido en cualquier escala.

use crate::theme;

/// Rayos: ángulo (grados, 0 = arriba, sentido horario) y largo (fracción del radio).
const RAYS: [(f32, f32); 12] = [
    (0.0, 1.0),
    (29.0, 0.82),
    (61.0, 0.95),
    (90.0, 0.78),
    (118.0, 1.0),
    (152.0, 0.84),
    (181.0, 0.96),
    (209.0, 0.8),
    (240.0, 1.0),
    (270.0, 0.86),
    (300.0, 0.93),
    (331.0, 0.8),
];

/// Ancho de cada rayo en el centro y en la punta (fracción del radio).
const BASE_W: f32 = 0.1;
const TIP_W: f32 = 0.075;

/// Distancia de `p` al rayo que va del centro a `tip` (negativa adentro).
fn ray_distance(px: f32, py: f32, tx: f32, ty: f32) -> f32 {
    let len2 = tx * tx + ty * ty;
    let t = ((px * tx + py * ty) / len2).clamp(0.0, 1.0);
    let (dx, dy) = (px - tx * t, py - ty * t);
    let half = (BASE_W + (TIP_W - BASE_W) * t) / 2.0;
    (dx * dx + dy * dy).sqrt() - half
}

/// El punto (x, y) de un cuadrado de lado 1 cae dentro del logo.
fn coverage_at(x: f32, y: f32) -> bool {
    let r = 0.5 - TIP_W / 2.0 * 0.5;
    RAYS.iter().any(|&(deg, len)| {
        let a = deg.to_radians();
        let (tx, ty) = (a.sin() * r * len, -a.cos() * r * len);
        ray_distance(x - 0.5, y - 0.5, tx, ty) <= 0.0
    })
}

/// RGBA (sin premultiplicar) de `px`×`px`, con 4×4 muestras por píxel para los bordes.
pub fn claude(px: u32) -> Vec<u8> {
    let [r, g, b, _] = theme::CLAUDE;
    let to8 = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    let n = px.max(1) as usize;
    let mut out = Vec::with_capacity(n * n * 4);
    const SS: usize = 4;
    for y in 0..n {
        for x in 0..n {
            let hits = (0..SS * SS)
                .filter(|i| {
                    let sx = (x as f32 + (i % SS) as f32 / SS as f32 + 0.5 / SS as f32) / n as f32;
                    let sy = (y as f32 + (i / SS) as f32 / SS as f32 + 0.5 / SS as f32) / n as f32;
                    coverage_at(sx, sy)
                })
                .count();
            let a = hits as f32 / (SS * SS) as f32;
            out.extend_from_slice(&[to8(r), to8(g), to8(b), to8(a)]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centro_lleno_y_esquinas_vacias() {
        let px = 32;
        let img = claude(px);
        assert_eq!(img.len(), (px * px * 4) as usize);
        let alpha = |x: u32, y: u32| img[((y * px + x) * 4 + 3) as usize];
        assert_eq!(alpha(16, 16), 255, "el centro es sólido");
        assert_eq!(alpha(0, 0), 0, "las esquinas quedan transparentes");
        assert_eq!(alpha(31, 31), 0);
        assert!(alpha(16, 1) > 0, "el rayo de arriba llega casi al borde");
    }

    /// Vista previa: `TELAR_LOGO_PNG=x.png cargo test logo -- --ignored`.
    #[test]
    #[ignore]
    fn vista_previa() {
        let Some(path) = std::env::var_os("TELAR_LOGO_PNG") else { return };
        let px = 256;
        let file = std::fs::File::create(path).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), px, px);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&claude(px)).unwrap();
    }
}
