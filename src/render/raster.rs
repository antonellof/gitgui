//! Triangle rasterizer for epaint meshes.
//!
//! Vertex colors and textures are premultiplied alpha in gamma space, so
//! blending is `dst = src + dst * (255 - src_a) / 255` per channel with no
//! gamma conversion. Textures are sampled bilinearly. Shared edges follow the
//! top-left fill rule so a quad made of two triangles never double-blends.

use std::collections::HashMap;

use epaint::textures::TexturesDelta;
use epaint::{ClippedPrimitive, ImageData, Mesh, Primitive, TextureId};

#[derive(Clone)]
pub struct Texture {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<[u8; 4]>,
}

impl Texture {
    fn white() -> Self {
        Self {
            w: 1,
            h: 1,
            rgba: vec![[255; 4]],
        }
    }

    #[inline]
    fn texel(&self, x: usize, y: usize) -> [u8; 4] {
        self.rgba[y * self.w + x]
    }

    /// Bilinear sample at texture-space uv in [0, 1].
    #[inline]
    fn sample(&self, u: f32, v: f32) -> [u8; 4] {
        if self.w == 1 && self.h == 1 {
            return self.rgba[0];
        }
        let fx = u * self.w as f32 - 0.5;
        let fy = v * self.h as f32 - 0.5;
        let x0f = fx.floor();
        let y0f = fy.floor();
        let tx = fx - x0f;
        let ty = fy - y0f;
        let maxx = self.w as i32 - 1;
        let maxy = self.h as i32 - 1;
        let x0 = (x0f as i32).clamp(0, maxx) as usize;
        let y0 = (y0f as i32).clamp(0, maxy) as usize;
        let x1 = (x0f as i32 + 1).clamp(0, maxx) as usize;
        let y1 = (y0f as i32 + 1).clamp(0, maxy) as usize;
        if tx < 1e-4 && ty < 1e-4 {
            return self.texel(x0, y0);
        }
        let p00 = self.texel(x0, y0);
        let p10 = self.texel(x1, y0);
        let p01 = self.texel(x0, y1);
        let p11 = self.texel(x1, y1);
        let mut out = [0u8; 4];
        for c in 0..4 {
            let top = p00[c] as f32 * (1.0 - tx) + p10[c] as f32 * tx;
            let bot = p01[c] as f32 * (1.0 - tx) + p11[c] as f32 * tx;
            out[c] = (top * (1.0 - ty) + bot * ty + 0.5) as u8;
        }
        out
    }
}

/// Where the rasterizer writes. `rgba` is `w * h * 4` bytes, row-major.
pub struct Target<'a> {
    pub w: usize,
    pub h: usize,
    pub rgba: &'a mut [u8],
}

#[inline]
fn blend(dst: &mut [u8], src: [u8; 4]) {
    let sa = src[3] as u32;
    if sa == 255 {
        dst[0] = src[0];
        dst[1] = src[1];
        dst[2] = src[2];
        dst[3] = 255;
    } else if sa != 0 {
        let inv = 255 - sa;
        for c in 0..4 {
            dst[c] = (src[c] as u32 + (dst[c] as u32 * inv + 127) / 255).min(255) as u8;
        }
    }
}

#[inline]
fn mul_color(tex: [u8; 4], col: [u8; 4]) -> [u8; 4] {
    if col == [255; 4] {
        return tex;
    }
    let mut out = [0u8; 4];
    for c in 0..4 {
        out[c] = ((tex[c] as u32 * col[c] as u32 + 127) / 255) as u8;
    }
    out
}

#[derive(Clone, Copy)]
struct V {
    x: f32,
    y: f32,
    u: f32,
    v: f32,
    c: [u8; 4],
}

#[derive(Clone, Copy)]
struct ClipPx {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

pub struct Rasterizer {
    textures: HashMap<TextureId, Texture>,
    white: Texture,
}

impl Default for Rasterizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Rasterizer {
    pub fn new() -> Self {
        Self {
            textures: HashMap::new(),
            white: Texture::white(),
        }
    }

    /// Apply `TexturesDelta::set`. Call before painting.
    pub fn apply_set(&mut self, delta: &TexturesDelta) {
        for (id, deltas) in &delta.set {
            for d in deltas {
                let ImageData::Color(img) = &d.image;
                let [iw, ih] = img.size;
                let src: Vec<[u8; 4]> = img.pixels.iter().map(|c| c.to_array()).collect();
                match d.pos {
                    None => {
                        self.textures.insert(
                            *id,
                            Texture {
                                w: iw,
                                h: ih,
                                rgba: src,
                            },
                        );
                    }
                    Some([px, py]) => {
                        if let Some(t) = self.textures.get_mut(id) {
                            if px >= t.w {
                                continue;
                            }
                            for row in 0..ih {
                                let ty = py + row;
                                if ty >= t.h {
                                    break;
                                }
                                let n = iw.min(t.w.saturating_sub(px));
                                let dst = &mut t.rgba[ty * t.w + px..ty * t.w + px + n];
                                dst.copy_from_slice(&src[row * iw..row * iw + n]);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Apply `TexturesDelta::free`. Call after painting.
    pub fn apply_free(&mut self, delta: &TexturesDelta) {
        for id in &delta.free {
            self.textures.remove(id);
        }
    }

    #[cfg(test)]
    pub fn texture(&self, id: TextureId) -> Option<&Texture> {
        self.textures.get(&id)
    }

    pub fn paint(&self, target: &mut Target, pixels_per_point: f32, prims: &[ClippedPrimitive]) {
        for prim in prims {
            let Primitive::Mesh(mesh) = &prim.primitive else {
                continue;
            };
            let r = prim.clip_rect;
            let clip = ClipPx {
                x0: ((r.min.x * pixels_per_point).round().max(0.0) as usize).min(target.w),
                y0: ((r.min.y * pixels_per_point).round().max(0.0) as usize).min(target.h),
                x1: ((r.max.x * pixels_per_point).round().max(0.0) as usize).min(target.w),
                y1: ((r.max.y * pixels_per_point).round().max(0.0) as usize).min(target.h),
            };
            if clip.x0 >= clip.x1 || clip.y0 >= clip.y1 {
                continue;
            }
            let tex = self.textures.get(&mesh.texture_id).unwrap_or(&self.white);
            self.draw_mesh(target, pixels_per_point, clip, mesh, tex);
        }
    }

    fn draw_mesh(&self, target: &mut Target, ppp: f32, clip: ClipPx, mesh: &Mesh, tex: &Texture) {
        let verts: Vec<V> = mesh
            .vertices
            .iter()
            .map(|v| V {
                x: v.pos.x * ppp,
                y: v.pos.y * ppp,
                u: v.uv.x,
                v: v.uv.y,
                c: v.color.to_array(),
            })
            .collect();
        for tri in mesh.indices.as_chunks::<3>().0 {
            let a = verts[tri[0] as usize];
            let b = verts[tri[1] as usize];
            let c = verts[tri[2] as usize];
            draw_triangle(target, clip, tex, a, b, c);
        }
    }
}

/// Edge function: positive on one side of the directed line a -> b.
#[inline]
fn edge(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (bx - ax) * (py - ay) - (by - ay) * (px - ax)
}

/// Top-left rule bias for an edge a -> b of a triangle with positive area
/// (as computed by [`edge`]). Returns 0 for top or left edges, which own
/// their boundary pixels, and a small positive epsilon otherwise so the
/// boundary is excluded.
#[inline]
fn edge_bias(ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let dx = bx - ax;
    let dy = by - ay;
    let top_left = (dy == 0.0 && dx < 0.0) || dy > 0.0;
    if top_left {
        0.0
    } else {
        1e-5
    }
}

fn draw_triangle(target: &mut Target, clip: ClipPx, tex: &Texture, a: V, b: V, mut c: V) {
    let mut b = b;
    let mut area = edge(a.x, a.y, b.x, b.y, c.x, c.y);
    if area == 0.0 {
        return;
    }
    if area < 0.0 {
        std::mem::swap(&mut b, &mut c);
        area = -area;
    }
    let min_x = a.x.min(b.x).min(c.x).floor().max(clip.x0 as f32) as usize;
    let max_x = (a.x.max(b.x).max(c.x).ceil().max(0.0) as usize).min(clip.x1);
    let min_y = a.y.min(b.y).min(c.y).floor().max(clip.y0 as f32) as usize;
    let max_y = (a.y.max(b.y).max(c.y).ceil().max(0.0) as usize).min(clip.y1);
    if min_x >= max_x || min_y >= max_y {
        return;
    }

    let flat = a.c == b.c && b.c == c.c && a.u == b.u && b.u == c.u && a.v == b.v && b.v == c.v;
    let flat_color = if flat {
        mul_color(tex.sample(a.u, a.v), a.c)
    } else {
        [0; 4]
    };
    if flat && flat_color[3] == 0 {
        return;
    }

    // Edge functions w0 (edge b->c, weight of a), w1 (c->a, weight of b), w2 (a->b, weight of c).
    let bias0 = edge_bias(b.x, b.y, c.x, c.y);
    let bias1 = edge_bias(c.x, c.y, a.x, a.y);
    let bias2 = edge_bias(a.x, a.y, b.x, b.y);
    let px0 = min_x as f32 + 0.5;
    let py0 = min_y as f32 + 0.5;
    let mut w0_row = edge(b.x, b.y, c.x, c.y, px0, py0) - bias0;
    let mut w1_row = edge(c.x, c.y, a.x, a.y, px0, py0) - bias1;
    let mut w2_row = edge(a.x, a.y, b.x, b.y, px0, py0) - bias2;
    let w0_dx = -(c.y - b.y);
    let w1_dx = -(a.y - c.y);
    let w2_dx = -(b.y - a.y);
    let w0_dy = c.x - b.x;
    let w1_dy = a.x - c.x;
    let w2_dy = b.x - a.x;
    let inv_area = 1.0 / area;
    let stride = target.w * 4;

    if flat {
        // Flat triangles: solve the three edge inequalities per row to get
        // the covered span and fill it directly. This is the hot path for
        // panel backgrounds, which cover most of the frame.
        let opaque = flat_color[3] == 255;
        for y in min_y..max_y {
            let mut xs = min_x as f32;
            let mut xe = max_x as f32;
            for (w, dx) in [(w0_row, w0_dx), (w1_row, w1_dx), (w2_row, w2_dx)] {
                // w(x) = w + dx * (x - min_x) >= 0
                if dx > 0.0 {
                    xs = xs.max(min_x as f32 + (-w / dx).ceil());
                } else if dx < 0.0 {
                    xe = xe.min(min_x as f32 + (-w / dx).floor() + 1.0);
                } else if w < 0.0 {
                    xs = xe;
                }
            }
            let xs = (xs.max(min_x as f32)) as usize;
            let xe = (xe.min(max_x as f32).max(0.0)) as usize;
            if xs < xe {
                let row = &mut target.rgba[y * stride + xs * 4..y * stride + xe * 4];
                if opaque {
                    row.as_chunks_mut::<4>().0.fill(flat_color);
                } else {
                    for px in row.as_chunks_mut::<4>().0 {
                        blend(px, flat_color);
                    }
                }
            }
            w0_row += w0_dy;
            w1_row += w1_dy;
            w2_row += w2_dy;
        }
        return;
    }

    // Textured or gradient triangles (text, icons, gradients).
    let same_color = a.c == b.c && b.c == c.c;
    let (tw, th) = (tex.w as f32, tex.h as f32);
    // uv gradients in texel space, from the barycentric gradients.
    let du_dx = (a.u * w0_dx + b.u * w1_dx + c.u * w2_dx) * inv_area * tw;
    let du_dy = (a.u * w0_dy + b.u * w1_dy + c.u * w2_dy) * inv_area * tw;
    let dv_dx = (a.v * w0_dx + b.v * w1_dx + c.v * w2_dx) * inv_area * th;
    let dv_dy = (a.v * w0_dy + b.v * w1_dy + c.v * w2_dy) * inv_area * th;
    // Texel at the first pixel center of the bounding box.
    let l0 = (w0_row + bias0) * inv_area;
    let l1 = (w1_row + bias1) * inv_area;
    let l2 = 1.0 - l0 - l1;
    let mut ut_row = (a.u * l0 + b.u * l1 + c.u * l2) * tw - 0.5;
    let mut vt_row = (a.v * l0 + b.v * l1 + c.v * l2) * th - 0.5;
    // 1:1 axis-aligned mapping (pixel-snapped glyphs): nearest sampling is
    // exact and much cheaper than bilinear.
    let one_to_one = (du_dx.abs() - 1.0).abs() < 1e-3
        && (dv_dy.abs() - 1.0).abs() < 1e-3
        && du_dy.abs() < 1e-3
        && dv_dx.abs() < 1e-3
        && (ut_row - ut_row.round()).abs() < 1e-2
        && (vt_row - vt_row.round()).abs() < 1e-2;
    let maxx = tex.w as i32 - 1;
    let maxy = tex.h as i32 - 1;

    for y in min_y..max_y {
        let mut w0 = w0_row;
        let mut w1 = w1_row;
        let mut w2 = w2_row;
        let mut ut = ut_row;
        let mut vt = vt_row;
        let row = &mut target.rgba[y * stride..(y + 1) * stride];
        for x in min_x..max_x {
            if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                let texel = if one_to_one {
                    let tx = (ut.round() as i32).clamp(0, maxx) as usize;
                    let ty = (vt.round() as i32).clamp(0, maxy) as usize;
                    tex.texel(tx, ty)
                } else {
                    tex.sample((ut + 0.5) / tw, (vt + 0.5) / th)
                };
                if texel[3] != 0 || !same_color {
                    let col = if same_color {
                        a.c
                    } else {
                        let l0 = (w0 + bias0) * inv_area;
                        let l1 = (w1 + bias1) * inv_area;
                        let l2 = 1.0 - l0 - l1;
                        let mut col = [0u8; 4];
                        for (i, ch) in col.iter_mut().enumerate() {
                            *ch = (a.c[i] as f32 * l0
                                + b.c[i] as f32 * l1
                                + c.c[i] as f32 * l2
                                + 0.5)
                                .clamp(0.0, 255.0) as u8;
                        }
                        col
                    };
                    let src = mul_color(texel, col);
                    if src[3] != 0 {
                        blend(&mut row[x * 4..x * 4 + 4], src);
                    }
                }
            }
            w0 += w0_dx;
            w1 += w1_dx;
            w2 += w2_dx;
            ut += du_dx;
            vt += dv_dx;
        }
        w0_row += w0_dy;
        w1_row += w1_dy;
        w2_row += w2_dy;
        ut_row += du_dy;
        vt_row += dv_dy;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use epaint::{Color32, Rect, Vertex};
    use std::sync::Arc;

    fn target(w: usize, h: usize) -> Vec<u8> {
        vec![0; w * h * 4]
    }

    fn mesh(tri: &[(f32, f32)], color: Color32, tex: TextureId) -> Mesh {
        let mut m = Mesh::with_texture(tex);
        for (i, (x, y)) in tri.iter().enumerate() {
            m.vertices.push(Vertex {
                pos: epaint::pos2(*x, *y),
                uv: epaint::WHITE_UV,
                color,
            });
            let _ = i;
        }
        for k in 0..tri.len() / 3 {
            m.indices
                .extend_from_slice(&[k as u32 * 3, k as u32 * 3 + 1, k as u32 * 3 + 2]);
        }
        m
    }

    fn count_color(buf: &[u8], rgba: [u8; 4]) -> usize {
        buf.as_chunks::<4>()
            .0
            .iter()
            .filter(|p| **p == rgba)
            .count()
    }

    fn prim(m: Mesh, clip: Rect) -> ClippedPrimitive {
        ClippedPrimitive {
            clip_rect: clip,
            primitive: Primitive::Mesh(m),
        }
    }

    #[test]
    fn single_triangle_pixel_count() {
        // Right triangle covering half of a 10x10 square: 45 pixel centers
        // lie strictly inside, 10 on the diagonal, and the diagonal is
        // excluded or included by the top-left rule consistently.
        let (w, h) = (10usize, 10usize);
        let mut buf = target(w, h);
        let r = Rasterizer::new();
        let m = mesh(
            &[(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)],
            Color32::RED,
            TextureId::default(),
        );
        let clip = Rect::from_min_max(epaint::pos2(0.0, 0.0), epaint::pos2(10.0, 10.0));
        r.paint(
            &mut Target {
                w,
                h,
                rgba: &mut buf,
            },
            1.0,
            &[prim(m, clip)],
        );
        let n = count_color(&buf, [255, 0, 0, 255]);
        assert!(n == 45 || n == 55, "got {n} red pixels");
        // Opposite triangle fills the rest exactly once: total = 100.
        let m2 = mesh(
            &[(10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
            Color32::RED,
            TextureId::default(),
        );
        r.paint(
            &mut Target {
                w,
                h,
                rgba: &mut buf,
            },
            1.0,
            &[prim(m2, clip)],
        );
        assert_eq!(count_color(&buf, [255, 0, 0, 255]), 100);
    }

    #[test]
    fn shared_edge_is_not_double_blended() {
        // Two half-transparent triangles forming a quad: every pixel must be
        // blended exactly once, so all 100 pixels end with the same value.
        let (w, h) = (10usize, 10usize);
        let mut buf = target(w, h);
        let r = Rasterizer::new();
        let col = Color32::from_rgba_premultiplied(128, 0, 0, 128);
        let m = mesh(
            &[
                (0.0, 0.0),
                (10.0, 0.0),
                (0.0, 10.0),
                (10.0, 0.0),
                (10.0, 10.0),
                (0.0, 10.0),
            ],
            col,
            TextureId::default(),
        );
        let clip = Rect::from_min_max(epaint::pos2(0.0, 0.0), epaint::pos2(10.0, 10.0));
        r.paint(
            &mut Target {
                w,
                h,
                rgba: &mut buf,
            },
            1.0,
            &[prim(m, clip)],
        );
        let first = &buf[0..4];
        assert_eq!(first, &[128, 0, 0, 128]);
        assert_eq!(count_color(&buf, [128, 0, 0, 128]), 100);
    }

    #[test]
    fn clip_rect_limits_drawing() {
        let (w, h) = (10usize, 10usize);
        let mut buf = target(w, h);
        let r = Rasterizer::new();
        let m = mesh(
            &[
                (0.0, 0.0),
                (10.0, 0.0),
                (0.0, 10.0),
                (10.0, 0.0),
                (10.0, 10.0),
                (0.0, 10.0),
            ],
            Color32::WHITE,
            TextureId::default(),
        );
        let clip = Rect::from_min_max(epaint::pos2(2.0, 3.0), epaint::pos2(5.0, 7.0));
        r.paint(
            &mut Target {
                w,
                h,
                rgba: &mut buf,
            },
            1.0,
            &[prim(m, clip)],
        );
        assert_eq!(count_color(&buf, [255; 4]), 3 * 4);
        let idx = |x: usize, y: usize| (y * w + x) * 4;
        assert_eq!(&buf[idx(2, 3)..idx(2, 3) + 4], &[255; 4]);
        assert_eq!(&buf[idx(1, 3)..idx(1, 3) + 4], &[0; 4]);
        assert_eq!(&buf[idx(5, 3)..idx(5, 3) + 4], &[0; 4]);
        assert_eq!(&buf[idx(4, 6)..idx(4, 6) + 4], &[255; 4]);
        assert_eq!(&buf[idx(4, 7)..idx(4, 7) + 4], &[0; 4]);
    }

    #[test]
    fn pixels_per_point_scales_positions_and_clip() {
        let (w, h) = (20usize, 20usize);
        let mut buf = target(w, h);
        let r = Rasterizer::new();
        let m = mesh(
            &[
                (0.0, 0.0),
                (5.0, 0.0),
                (0.0, 5.0),
                (5.0, 0.0),
                (5.0, 5.0),
                (0.0, 5.0),
            ],
            Color32::WHITE,
            TextureId::default(),
        );
        let clip = Rect::from_min_max(epaint::pos2(0.0, 0.0), epaint::pos2(10.0, 10.0));
        r.paint(
            &mut Target {
                w,
                h,
                rgba: &mut buf,
            },
            2.0,
            &[prim(m, clip)],
        );
        assert_eq!(count_color(&buf, [255; 4]), 100);
    }

    #[test]
    fn textured_quad_with_2x2_texture() {
        // 2x2 texture: red, green / blue, white. Drawn as a 4x4 quad with
        // nearest-aligned uvs so each texel covers a 2x2 block.
        let (w, h) = (4usize, 4usize);
        let mut buf = target(w, h);
        let mut r = Rasterizer::new();
        let id = TextureId::User(7);
        let img = epaint::ColorImage {
            size: [2, 2],
            source_size: epaint::vec2(2.0, 2.0),
            pixels: vec![Color32::RED, Color32::GREEN, Color32::BLUE, Color32::WHITE],
        };
        let mut delta = TexturesDelta::default();
        delta.set.insert(
            id,
            smallvec_one(epaint::ImageDelta::full(
                ImageData::Color(Arc::new(img)),
                Default::default(),
            )),
        );
        r.apply_set(&delta);
        delta.clear();
        assert_eq!(r.texture(id).unwrap().w, 2);

        let mut m = Mesh::with_texture(id);
        let quad = [
            ((0.0, 0.0), (0.0, 0.0)),
            ((4.0, 0.0), (1.0, 0.0)),
            ((4.0, 4.0), (1.0, 1.0)),
            ((0.0, 4.0), (0.0, 1.0)),
        ];
        for ((x, y), (u, v)) in quad {
            m.vertices.push(Vertex {
                pos: epaint::pos2(x, y),
                uv: epaint::pos2(u, v),
                color: Color32::WHITE,
            });
        }
        m.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
        let clip = Rect::from_min_max(epaint::pos2(0.0, 0.0), epaint::pos2(4.0, 4.0));
        r.paint(
            &mut Target {
                w,
                h,
                rgba: &mut buf,
            },
            1.0,
            &[prim(m, clip)],
        );
        let px = |x: usize, y: usize| &buf[(y * w + x) * 4..(y * w + x) * 4 + 4];
        // Corners are pure texels; the middle is a bilinear mix.
        assert_eq!(px(0, 0), &[255, 0, 0, 255]);
        assert_eq!(px(3, 0), &[0, 255, 0, 255]);
        assert_eq!(px(0, 3), &[0, 0, 255, 255]);
        assert_eq!(px(3, 3), &[255, 255, 255, 255]);
        // Pixel center (1.5, 1.5) maps to uv 0.375: mostly red with some
        // green and blue bleeding in from the bilinear filter.
        let mid = px(1, 1);
        assert!(
            mid[0] > mid[1] && mid[1] > 30 && mid[2] > 30,
            "expected a red-dominant mix, got {mid:?}"
        );

        // Partial update of the top-left texel to black.
        let patch = epaint::ColorImage {
            size: [1, 1],
            source_size: epaint::vec2(1.0, 1.0),
            pixels: vec![Color32::BLACK],
        };
        let mut delta = TexturesDelta::default();
        delta.set.insert(
            id,
            smallvec_one(epaint::ImageDelta::partial(
                [0, 0],
                ImageData::Color(Arc::new(patch)),
                Default::default(),
            )),
        );
        r.apply_set(&delta);
        delta.clear();
        assert_eq!(r.texture(id).unwrap().rgba[0], [0, 0, 0, 255]);
        let mut delta = TexturesDelta::default();
        delta.free.insert(id);
        r.apply_free(&delta);
        delta.clear();
        assert!(r.texture(id).is_none());
    }

    #[test]
    fn transparent_flat_triangle_is_skipped_and_blend_math() {
        let mut dst = [100u8, 100, 100, 255];
        blend(&mut dst, [0, 0, 0, 0]);
        assert_eq!(dst, [100, 100, 100, 255]);
        blend(&mut dst, [128, 0, 0, 128]);
        assert_eq!(dst, [178, 50, 50, 255]);
        blend(&mut dst, [1, 2, 3, 255]);
        assert_eq!(dst, [1, 2, 3, 255]);
    }

    fn smallvec_one<T>(v: T) -> smallvec::SmallVec<[T; 1]> {
        let mut s = smallvec::SmallVec::new();
        s.push(v);
        s
    }
}
