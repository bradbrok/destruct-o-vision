//! Text for text modules: fonts, and the signed distance field their kernels
//! read with `text_sdf`.
//!
//! The text is laid out and rasterized on the CPU whenever it or its font
//! changes, then turned into a distance field with an exact Euclidean
//! distance transform. Distances are in line heights (the font's ascent minus
//! descent), negative inside the letters, and reach `SPREAD` beyond them.

use std::collections::HashMap;

use ab_glyph::{Font, FontArc, Glyph, PxScale, ScaleFont, point};

/// How far the field reaches beyond the letters, in line heights.
pub const SPREAD: f32 = dov_format::glue::TEXT_SPREAD;
/// Rendered line height in pixels. Long text renders smaller so the texture
/// stays within `MAX_WIDTH` × `MAX_HEIGHT`.
const LINE_PX: f32 = 128.0;
const MAX_WIDTH: f32 = 4096.0;
const MAX_HEIGHT: f32 = 2048.0;
/// Lines beyond this are dropped. The text itself is already capped at
/// `dov_core::TEXT_MAX_CHARS` by the param bank.
const MAX_LINES: usize = 8;
const FAR: f32 = 1e20;

/// Candidate files for each entry of `dov_core::FONTS`, tried in order.
fn candidates(font: usize) -> &'static [&'static str] {
    match font {
        0 => &[
            "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
            "C:\\Windows\\Fonts\\arialbd.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        ],
        1 => &[
            "/System/Library/Fonts/Supplemental/Arial Black.ttf",
            "C:\\Windows\\Fonts\\ariblk.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        ],
        2 => &[
            "/System/Library/Fonts/Supplemental/Impact.ttf",
            "C:\\Windows\\Fonts\\impact.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansCondensed-Bold.ttf",
        ],
        3 => &[
            "/System/Library/Fonts/Supplemental/Times New Roman Bold.ttf",
            "C:\\Windows\\Fonts\\timesbd.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSerif-Bold.ttf",
        ],
        4 => &[
            "/System/Library/Fonts/Supplemental/Courier New Bold.ttf",
            "C:\\Windows\\Fonts\\courbd.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf",
        ],
        5 => &[
            "/System/Library/Fonts/Supplemental/Brush Script.ttf",
            "C:\\Windows\\Fonts\\BRUSHSCI.TTF",
        ],
        _ => &[
            "/System/Library/Fonts/Supplemental/Chalkduster.ttf",
            "C:\\Windows\\Fonts\\comicbd.ttf",
        ],
    }
}

/// Loaded fonts, by index into `dov_core::FONTS`.
#[derive(Default)]
pub struct Fonts {
    loaded: HashMap<usize, FontArc>,
}

impl Fonts {
    pub fn get(&mut self, font: usize) -> FontArc {
        self.loaded
            .entry(font)
            .or_insert_with(|| {
                let file = candidates(font)
                    .iter()
                    .find_map(|path| FontArc::try_from_vec(std::fs::read(path).ok()?).ok());
                file.unwrap_or_else(|| {
                    log::warn!("font {font}: no system font found, using the bundled one");
                    let bundled = if font == 4 {
                        epaint_default_fonts::HACK_REGULAR
                    } else {
                        epaint_default_fonts::UBUNTU_LIGHT
                    };
                    FontArc::try_from_slice(bundled).expect("bundled font parses")
                })
            })
            .clone()
    }
}

/// A rendered distance field.
pub struct TextField {
    pub width: u32,
    pub height: u32,
    /// Signed distance per texel, in line heights.
    pub texels: Vec<f32>,
    /// Size of the laid-out text, in line heights.
    pub size: [f32; 2],
    /// The texture's extent in text coordinates (line heights, origin at the
    /// text's center, y up): left, bottom, right, top.
    pub rect: [f32; 4],
}

/// Lines, centered, at `px` per line. Returns positioned glyphs and the
/// text's width and height in pixels.
fn layout(font: &FontArc, text: &str, px: f32) -> (Vec<Glyph>, f32, f32) {
    let scaled = font.as_scaled(PxScale::from(px));
    let advance = scaled.height() + scaled.line_gap();
    let lines: Vec<&str> = text.split('\n').collect();
    let widths: Vec<f32> = lines
        .iter()
        .map(|line| {
            let mut caret = 0.0;
            let mut prev = None;
            for c in line.chars() {
                let id = font.glyph_id(c);
                if let Some(p) = prev {
                    caret += scaled.kern(p, id);
                }
                caret += scaled.h_advance(id);
                prev = Some(id);
            }
            caret
        })
        .collect();
    let width = widths.iter().copied().fold(0.0, f32::max);
    let height = scaled.height() + advance * (lines.len() - 1) as f32;
    let mut glyphs = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let mut caret = (width - widths[i]) / 2.0;
        let baseline = scaled.ascent() + advance * i as f32;
        let mut prev = None;
        for c in line.chars() {
            let id = font.glyph_id(c);
            if let Some(p) = prev {
                caret += scaled.kern(p, id);
            }
            glyphs.push(id.with_scale_and_position(px, point(caret, baseline)));
            caret += scaled.h_advance(id);
            prev = Some(id);
        }
    }
    (glyphs, width, height)
}

pub fn render(font: &FontArc, text: &str) -> TextField {
    // Text can come from any patch file: keep the texture and the work bounded.
    let text: String = text
        .chars()
        .take(dov_core::TEXT_MAX_CHARS)
        .collect::<String>()
        .split('\n')
        .take(MAX_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    let text = if text.trim().is_empty() {
        " "
    } else {
        text.as_str()
    };
    let (mut glyphs, mut width, mut height) = layout(font, text, LINE_PX);
    let pad = 2.0 * SPREAD * LINE_PX;
    let fit = (MAX_WIDTH / (width + pad)).min(MAX_HEIGHT / (height + pad));
    let mut px = LINE_PX;
    if fit < 1.0 {
        px = LINE_PX * fit;
        (glyphs, width, height) = layout(font, text, px);
    }

    // The texture covers the layout box and every glyph's ink (within a line
    // height of the box, against fonts with wild metrics), plus the spread.
    let outlines: Vec<_> = glyphs.into_iter().filter_map(|g| font.outline_glyph(g)).collect();
    let (mut x0, mut y0, mut x1, mut y1) = (0.0f32, 0.0f32, width, height);
    for o in &outlines {
        let b = o.px_bounds();
        x0 = x0.min(b.min.x).max(-px);
        y0 = y0.min(b.min.y).max(-px);
        x1 = x1.max(b.max.x).min(width + px);
        y1 = y1.max(b.max.y).min(height + px);
    }
    let pad = (SPREAD * px).ceil() + 1.0;
    let (ox, oy) = ((x0 - pad).floor(), (y0 - pad).floor());
    let w = ((x1 + pad).ceil() - ox) as usize;
    let h = ((y1 + pad).ceil() - oy) as usize;

    let mut coverage = vec![0.0f32; w * h];
    for o in &outlines {
        let b = o.px_bounds();
        o.draw(|x, y, c| {
            let px_x = (b.min.x - ox) as i64 + i64::from(x);
            let px_y = (b.min.y - oy) as i64 + i64::from(y);
            if (0..w as i64).contains(&px_x) && (0..h as i64).contains(&px_y) {
                let i = px_y as usize * w + px_x as usize;
                coverage[i] = (coverage[i] + c).min(1.0);
            }
        });
    }

    let inside: Vec<bool> = coverage.iter().map(|&c| c >= 0.5).collect();
    let to_inside = edt(&inside, w, h, true);
    let to_outside = edt(&inside, w, h, false);
    let texels = inside
        .iter()
        .zip(to_inside.iter().zip(&to_outside))
        .map(|(&is_in, (&a, &b))| {
            // Pixel centers are half a pixel from the boundary between them.
            let d = if is_in { 0.5 - b.sqrt() } else { a.sqrt() - 0.5 };
            (d / px).clamp(-SPREAD, SPREAD)
        })
        .collect();

    // Text coordinates: line heights, origin at the layout box's center, y up.
    let (cx, cy) = (width / 2.0, height / 2.0);
    TextField {
        width: w as u32,
        height: h as u32,
        texels,
        size: [width / px, height / px],
        rect: [
            (ox - cx) / px,
            (cy - (oy + h as f32)) / px,
            (ox + w as f32 - cx) / px,
            (cy - oy) / px,
        ],
    }
}

/// Squared distance from each pixel to the nearest pixel whose `mask` equals
/// `target` (Felzenszwalb and Huttenlocher's linear-time transform).
fn edt(mask: &[bool], w: usize, h: usize, target: bool) -> Vec<f32> {
    let mut grid: Vec<f32> = mask
        .iter()
        .map(|&m| if m == target { 0.0 } else { FAR })
        .collect();
    let n = w.max(h);
    let (mut f, mut d) = (vec![0.0; n], vec![0.0; n]);
    let (mut v, mut z) = (vec![0usize; n], vec![0.0f32; n + 1]);
    for x in 0..w {
        for y in 0..h {
            f[y] = grid[y * w + x];
        }
        edt_1d(&f[..h], &mut d[..h], &mut v, &mut z);
        for y in 0..h {
            grid[y * w + x] = d[y];
        }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&grid[y * w..(y + 1) * w]);
        edt_1d(&f[..w], &mut d[..w], &mut v, &mut z);
        grid[y * w..(y + 1) * w].copy_from_slice(&d[..w]);
    }
    grid
}

/// One dimension of the transform: the lower envelope of parabolas.
fn edt_1d(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    let sq = |q: usize| (q * q) as f32;
    let mut k = 0;
    v[0] = 0;
    z[0] = -FAR;
    z[1] = FAR;
    for q in 1..n {
        let mut s;
        loop {
            let p = v[k];
            s = ((f[q] + sq(q)) - (f[p] + sq(p))) / (2.0 * (q as f32 - p as f32));
            if s > z[k] || k == 0 {
                break;
            }
            k -= 1;
        }
        if s <= z[k] {
            // Only reachable at k == 0: this parabola hides the first one.
            v[0] = q;
            z[1] = FAR;
            continue;
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = FAR;
    }
    k = 0;
    for (q, out) in d.iter_mut().enumerate().take(n) {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let dq = q as f32 - v[k] as f32;
        *out = dq * dq + f[v[k]];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_transform_is_exact_on_a_line() {
        // One inside pixel at x = 3 on a 7-pixel line.
        let mask = [false, false, false, true, false, false, false];
        let d = edt(&mask, 7, 1, true);
        assert_eq!(d, [9.0, 4.0, 1.0, 0.0, 1.0, 4.0, 9.0]);
    }

    #[test]
    fn distance_transform_is_euclidean_in_2d() {
        let (w, h) = (5, 5);
        let mut mask = vec![false; w * h];
        mask[2 * w + 2] = true;
        let d = edt(&mask, w, h, true);
        assert_eq!(d[0], 8.0, "corner is (2, 2) away");
        assert_eq!(d[w + 3], 2.0);
        assert_eq!(d[2 * w + 4], 4.0);
    }

    #[test]
    fn hostile_text_keeps_the_texture_small() {
        let font = FontArc::try_from_slice(epaint_default_fonts::HACK_REGULAR).unwrap();
        for text in ["W".repeat(100_000), "W\n".repeat(100_000), "\n".repeat(100_000)] {
            let field = render(&font, &text);
            assert!(
                field.width <= 4400 && field.height <= 2400,
                "{}x{}",
                field.width,
                field.height
            );
            assert_eq!(field.texels.len(), (field.width * field.height) as usize);
        }
    }

    #[test]
    fn letters_are_negative_inside_and_positive_outside() {
        let font = FontArc::try_from_slice(epaint_default_fonts::HACK_REGULAR).unwrap();
        let field = render(&font, "I");
        let (w, h) = (field.width as usize, field.height as usize);
        let at = |x: usize, y: usize| field.texels[y * w + x];
        // The middle of an I is ink; the texture's corner is empty space.
        assert!(at(w / 2, h / 2) < 0.0, "center {}", at(w / 2, h / 2));
        assert_eq!(at(0, 0), SPREAD);
        assert!(field.rect[0] < -field.size[0] / 2.0 && field.rect[2] > field.size[0] / 2.0);
        assert!(field.rect[1] < -0.5 && field.rect[3] > 0.5);
    }
}
