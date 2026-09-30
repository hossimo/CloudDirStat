//! Cushion shading (van Wijk & van de Wetering, as used by WinDirStat): every level of
//! the tree adds a parabolic ridge to the surface, so nested folders read as bumps
//! and the boundaries between them show up as dark creases.

use std::collections::{HashMap, HashSet};

use clouddirstat_core::{NodeId, Rect, Tree};
use eframe::egui::{Color32, ColorImage};

const HEIGHT: f32 = 0.38;
const SCALE_PER_LEVEL: f32 = 0.91;
const AMBIENT: f32 = 0.13;
const BRIGHTNESS: f32 = 1.2;
/// Light from the top left, mostly from the front: (-1, -1, 10) normalized.
const LIGHT: [f32; 3] = [
    -1.0 / LIGHT_LENGTH,
    -1.0 / LIGHT_LENGTH,
    10.0 / LIGHT_LENGTH,
];
const LIGHT_LENGTH: f32 = 10.099_505; // sqrt(1² + 1² + 10²)
pub const BACKGROUND: Color32 = Color32::from_rgb(0x20, 0x20, 0x20);

/// Renders a layout (in pixel coordinates) into an image of `size` pixels.
/// Only nodes without laid-out children are painted; folders shape the surface.
pub fn render(
    tree: &Tree,
    layout: &[(NodeId, Rect)],
    size: [usize; 2],
    color: impl Fn(NodeId) -> Color32,
) -> ColorImage {
    let mut image = ColorImage::filled(size, BACKGROUND);
    let parents: HashSet<NodeId> = layout
        .iter()
        .skip(1)
        .filter_map(|&(id, _)| tree.node(id).parent())
        .collect();
    let mut surfaces: HashMap<NodeId, (Surface, f32)> = HashMap::new();

    for &(id, rect) in layout {
        let (mut surface, height) = match tree.node(id).parent().and_then(|p| surfaces.get(&p)) {
            Some(&(surface, height)) => (surface, height * SCALE_PER_LEVEL),
            None => (Surface::default(), HEIGHT),
        };
        surface.add_ridge(rect, height);

        if parents.contains(&id) {
            surfaces.insert(id, (surface, height));
        } else {
            paint(&mut image, rect, surface, color(id));
        }
    }
    image
}

/// Coefficients of `z = x2·x² + x1·x + y2·y² + y1·y`.
#[derive(Clone, Copy, Default)]
struct Surface {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Surface {
    fn add_ridge(&mut self, rect: Rect, height: f32) {
        if rect.w > 0.0 {
            let factor = 4.0 * height / rect.w;
            self.x2 -= factor;
            self.x1 += factor * (2.0 * rect.x + rect.w);
        }
        if rect.h > 0.0 {
            let factor = 4.0 * height / rect.h;
            self.y2 -= factor;
            self.y1 += factor * (2.0 * rect.y + rect.h);
        }
    }

    fn brightness(&self, x: f32, y: f32) -> f32 {
        let nx = -(2.0 * self.x2 * x + self.x1);
        let ny = -(2.0 * self.y2 * y + self.y1);
        let cosine = (nx * LIGHT[0] + ny * LIGHT[1] + LIGHT[2]) / (nx * nx + ny * ny + 1.0).sqrt();
        (AMBIENT + (1.0 - AMBIENT) * cosine.max(0.0)) * BRIGHTNESS
    }
}

fn paint(image: &mut ColorImage, rect: Rect, surface: Surface, color: Color32) {
    let [width, height] = image.size;
    let columns = pixel_range(rect.x, rect.w, width);
    let rows = pixel_range(rect.y, rect.h, height);

    for y in rows {
        let row = &mut image.pixels[y * width..(y + 1) * width];
        for x in columns.clone() {
            let light = surface.brightness(x as f32 + 0.5, y as f32 + 0.5);
            row[x] = shade(color, light);
        }
    }
}

fn pixel_range(start: f32, length: f32, limit: usize) -> std::ops::Range<usize> {
    let clamp = |value: f32| (value.round().max(0.0) as usize).min(limit);
    clamp(start)..clamp(start + length)
}

fn shade(color: Color32, light: f32) -> Color32 {
    let channel = |value: u8| (f32::from(value) * light).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(channel(color.r()), channel(color.g()), channel(color.b()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clouddirstat_core::{Entry, EntryKind, squarify};

    fn tree_of(files: &[(&str, u64)]) -> Tree {
        let mut tree = Tree::new();
        for &(key, size) in files {
            tree.insert(&Entry {
                key: key.to_owned(),
                size,
                storage_class: "STANDARD".to_owned(),
                kind: EntryKind::Current,
                last_modified: None,
            });
        }
        tree
    }

    fn luminance(color: Color32) -> u32 {
        u32::from(color.r()) + u32::from(color.g()) + u32::from(color.b())
    }

    #[test]
    fn covers_every_pixel_and_is_brightest_in_the_middle() {
        let tree = tree_of(&[("only.bin", 1)]);
        let layout = squarify(&tree, Tree::ROOT, Rect::new(0.0, 0.0, 40.0, 20.0), 0.0);

        let image = render(&tree, &layout, [40, 20], |_| Color32::from_gray(150));

        assert!(image.pixels.iter().all(|&pixel| pixel != BACKGROUND));
        let center = luminance(image.pixels[10 * 40 + 20]);
        let corner = luminance(image.pixels[19 * 40 + 39]);
        assert!(
            center > corner,
            "center {center} should be brighter than corner {corner}"
        );
    }

    #[test]
    fn leaves_background_outside_the_layout() {
        let tree = tree_of(&[("a", 1)]);
        let layout = squarify(&tree, Tree::ROOT, Rect::new(0.0, 0.0, 5.0, 5.0), 0.0);

        let image = render(&tree, &layout, [10, 10], |_| Color32::from_gray(150));

        assert_ne!(image.pixels[0], BACKGROUND);
        assert_eq!(image.pixels[9 * 10 + 9], BACKGROUND);
    }

    #[test]
    fn folder_boundaries_are_darker_than_folder_centers() {
        let tree = tree_of(&[("left/a", 1), ("right/b", 1)]);
        let layout = squarify(&tree, Tree::ROOT, Rect::new(0.0, 0.0, 40.0, 20.0), 0.0);

        let image = render(&tree, &layout, [40, 20], |_| Color32::from_gray(150));

        let at = |x: usize| luminance(image.pixels[10 * 40 + x]);
        assert!(
            at(19) < at(10),
            "the edge of a folder should be darker than its middle"
        );
    }
}
