//! Renders the bundled app icon (`assets/desktop/dev.ferro.ferrotune.svg`)
//! to the ARGB32 pixmaps the StatusNotifierItem protocol carries.

use resvg::{tiny_skia, usvg};

/// Pixmap sizes offered to the panel; it picks the closest one.
pub const SIZES: [u32; 3] = [22, 32, 48];

const ICON_SVG: &str = crate::desktop::ICON_SVG;

/// Renders every size; `None` if the bundled SVG fails to parse.
pub fn render() -> Option<Vec<ksni::Icon>> {
    SIZES
        .iter()
        .map(|&size| render_svg(ICON_SVG, size).map(|p| to_argb(&p)))
        .collect()
}

fn render_svg(svg: &str, size: u32) -> Option<tiny_skia::Pixmap> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(size, size)?;
    let scale = size as f32 / tree.size().width().max(1.0);
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Some(pixmap)
}

/// tiny-skia stores premultiplied RGBA; SNI wants straight ARGB32 in
/// network byte order (A, R, G, B).
fn to_argb(p: &tiny_skia::Pixmap) -> ksni::Icon {
    let mut data = Vec::with_capacity(p.pixels().len() * 4);
    for px in p.pixels() {
        let c = px.demultiply();
        data.extend_from_slice(&[c.alpha(), c.red(), c.green(), c.blue()]);
    }
    ksni::Icon {
        width: p.width() as i32,
        height: p.height() as i32,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_every_size_as_argb() {
        let icons = render().unwrap();
        assert_eq!(icons.len(), SIZES.len());
        for (icon, size) in icons.iter().zip(SIZES) {
            assert_eq!(icon.width, size as i32);
            assert_eq!(icon.data.len(), (size * size * 4) as usize);
        }
        // Rounded tile: the corner is transparent, the middle is opaque.
        let i = &icons[2];
        assert_eq!(i.data[0], 0);
        let o = (24 * i.width as usize + 6) * 4;
        assert_eq!(i.data[o], 255);
    }
}
