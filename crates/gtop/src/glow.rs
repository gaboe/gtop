use crate::palette::Tokens;
use gpui::{prelude::*, *};
use std::{cell::RefCell, collections::HashMap, fmt::Write, sync::Arc};

pub fn shadow(color: u32, radius: f32, alpha: u8) -> BoxShadow {
    BoxShadow {
        inset: false,
        color: rgba((color << 8) | u32::from(alpha)).into(),
        offset: point(px(0.), px(0.)),
        blur_radius: px(radius),
        spread_radius: px(0.),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Face {
    Display,
    Mono,
}
impl Face {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Display => include_bytes!("../assets/fonts/DotoDisplay.ttf"),
            Self::Mono => include_bytes!("../assets/fonts/JetBrainsMono.ttf"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    face: Face,
    glyph: u16,
    size: u32,
    scale: u32,
    color: u32,
    hot: bool,
    weight: u16,
}

struct GlyphGlow {
    image: Arc<RenderImage>,
    bounds: Bounds<Pixels>,
}

thread_local! {
    static GLYPHS: RefCell<HashMap<GlyphKey, Option<GlyphGlow>>> = RefCell::new(HashMap::new());
}

#[derive(Default)]
struct Outline(String);
impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.0, "M{x} {}", -y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.0, "L{x} {}", -y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let _ = write!(self.0, "Q{x1} {} {x} {}", -y1, -y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let _ = write!(self.0, "C{x1} {} {x2} {} {x} {}", -y1, -y2, -y);
    }
    fn close(&mut self) {
        self.0.push('Z');
    }
}

fn rasterize(key: GlyphKey) -> Option<GlyphGlow> {
    let mut face = ttf_parser::Face::parse(key.face.bytes(), 0).expect("bundled glow font");
    face.set_variation(ttf_parser::Tag::from_bytes(b"wght"), f32::from(key.weight));
    let mut outline = Outline::default();
    let rect = face.outline_glyph(ttf_parser::GlyphId(key.glyph), &mut outline)?;
    let size = f32::from_bits(key.size);
    let scale = f32::from_bits(key.scale);
    let factor = size / f32::from(face.units_per_em());
    let radius = if key.face == Face::Display { 8. } else { 6. };
    let pad = radius * 1.5;
    let x = f32::from(rect.x_min) * factor - pad;
    let y = -f32::from(rect.y_max) * factor - pad;
    let width = ((f32::from(rect.width()) * factor + pad * 2.) * scale).ceil() as u32;
    let height = ((f32::from(rect.height()) * factor + pad * 2.) * scale).ceil() as u32;
    let opacity = if key.face == Face::Mono {
        0.45
    } else if key.hot {
        0.65
    } else {
        0.60
    };
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="{x} {y} {} {}"><defs><filter id="g" x="-100%" y="-100%" width="300%" height="300%"><feGaussianBlur stdDeviation="{}"/></filter></defs><g filter="url(#g)" opacity="{opacity}"><path fill="#{:06x}" transform="scale({factor})" d="{}"/></g></svg>"##,
        width,
        height,
        width as f32 / scale,
        height as f32 / scale,
        radius / 2.,
        key.color,
        outline.0
    );
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default())
        .expect("glyph glow SVG");
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).expect("glyph glow dimensions");
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    let mut bytes = pixmap.take();
    for pixel in bytes.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = (u32::from(*channel) * 255)
                .checked_div(alpha)
                .unwrap_or(0)
                .min(255) as u8;
        }
        pixel.swap(0, 2);
    }
    let image = image::RgbaImage::from_raw(width, height, bytes).expect("glyph glow pixels");
    Some(GlyphGlow {
        image: Arc::new(RenderImage::new(vec![image::Frame::new(image)])),
        bounds: Bounds::new(
            point(px(x), px(y)),
            gpui::size(px(width as f32 / scale), px(height as f32 / scale)),
        ),
    })
}

// GPUI has no text-shadow primitive. Cache the blurred font outlines, then paint one sprite per glyph.
pub fn text(text: impl Into<SharedString>, hot: bool, t: &Tokens) -> Div {
    let text = text.into();
    let color = if hot { t.base.hot } else { t.base.led };
    div()
        .relative()
        .when(!t.light, |div| {
            let text: SharedString = text.lines().next().unwrap_or_default().to_owned().into();
            div.child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let style = window.text_style();
                        let face = match style.font_family.as_ref() {
                            "Doto Display" => Face::Display,
                            "JetBrains Mono" => Face::Mono,
                            _ => return,
                        };
                        let font_size = style.font_size.to_pixels(window.rem_size());
                        let line = window.text_system().shape_line(
                            text.clone(),
                            font_size,
                            &[style.to_run(text.len())],
                            None,
                        );
                        let offset_x = match style.text_align {
                            TextAlign::Right => bounds.size.width - line.width(),
                            TextAlign::Center => (bounds.size.width - line.width()) / 2.,
                            _ => px(0.),
                        };
                        let baseline = bounds.origin
                            + point(
                                offset_x,
                                (bounds.size.height - line.ascent - line.descent) / 2.
                                    + line.ascent,
                            );
                        let scale = window.scale_factor();
                        GLYPHS.with(|cache| {
                            let mut cache = cache.borrow_mut();
                            let font_id = window.text_system().resolve_font(&style.font());
                            for run in &line.runs {
                                if run.font_id != font_id {
                                    continue;
                                }
                                for glyph in &run.glyphs {
                                    if glyph.is_emoji {
                                        continue;
                                    }
                                    let Ok(id) = u16::try_from(glyph.id.0) else {
                                        continue;
                                    };
                                    let key = GlyphKey {
                                        face,
                                        glyph: id,
                                        size: f32::from(font_size).to_bits(),
                                        scale: scale.to_bits(),
                                        color,
                                        hot,
                                        weight: style.font_weight.0 as u16,
                                    };
                                    if !cache.contains_key(&key) {
                                        if cache.len() >= 1024 {
                                            for (_, glow) in cache.drain() {
                                                if let Some(glow) = glow {
                                                    let _ = window.drop_image(glow.image);
                                                }
                                            }
                                        }
                                        cache.insert(key, rasterize(key));
                                    }
                                    if let Some(Some(glow)) = cache.get(&key) {
                                        let image_bounds = Bounds::new(
                                            baseline + glyph.position + glow.bounds.origin,
                                            glow.bounds.size,
                                        );
                                        let _ = window.paint_image(
                                            image_bounds,
                                            image_bounds,
                                            Corners::default(),
                                            glow.image.clone(),
                                            0,
                                            false,
                                        );
                                    }
                                }
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
        })
        .child(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    fn key(scale: f32, hot: bool) -> GlyphKey {
        let face = ttf_parser::Face::parse(Face::Display.bytes(), 0).unwrap();
        GlyphKey {
            face: Face::Display,
            glyph: face.glyph_index('9').unwrap().0,
            size: 46_f32.to_bits(),
            scale: scale.to_bits(),
            color: if hot { 0xff3b1f } else { 0xffb000 },
            hot,
            weight: 900,
        }
    }

    #[test]
    fn glyph_halo_is_transparent_padded_and_scales_with_the_display() {
        let one = rasterize(key(1., false)).unwrap();
        let two = rasterize(key(2., false)).unwrap();
        let bytes = one.image.as_bytes(0).unwrap();
        assert!(bytes.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
        assert!(bytes.as_chunks::<4>().0.iter().all(|p| p[3] < 153));
        assert!(f32::from(one.bounds.origin.x) < 0.);
        assert!((f32::from(one.bounds.size.width - two.bounds.size.width)).abs() <= 1.);
        assert!((two.image.size(0).width.0 - one.image.size(0).width.0 * 2).abs() <= 1);
        assert_ne!(
            bytes,
            rasterize(key(1., true)).unwrap().image.as_bytes(0).unwrap()
        );
    }

    #[test]
    fn spaces_have_no_glow() {
        let face = ttf_parser::Face::parse(Face::Display.bytes(), 0).unwrap();
        let mut key = key(1., false);
        key.glyph = face.glyph_index(' ').unwrap().0;
        assert!(rasterize(key).is_none());
    }
}
