//! UI icons: Font Awesome Free solid SVGs (CC BY 4.0), in `assets/icons/`.
//!
//! Each SVG is one filled path. It is rasterized with `vello_cpu`, the rasterizer epaint
//! already uses for glyphs, into a white alpha mask at the screen's pixel density, cached as
//! a texture, and tinted when drawn.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use egui::load::SizedTexture;
use egui::{Color32, ColorImage, Image, Pos2, Rect, RichText, TextStyle, TextureHandle, TextureOptions};
use vello_cpu::{color, kurbo};

macro_rules! icons {
    ($($variant:ident => $name:literal),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Icon {
            $($variant),*
        }

        impl Icon {
            pub const ALL: &[Icon] = &[$(Icon::$variant),*];

            /// The Font Awesome name, also the `{name}` token in [`rich_label`].
            pub fn name(self) -> &'static str {
                match self {
                    $(Icon::$variant => $name),*
                }
            }

            fn svg(self) -> &'static str {
                match self {
                    $(Icon::$variant => include_str!(concat!("../assets/icons/", $name, ".svg"))),*
                }
            }
        }
    };
}

icons! {
    ArrowsRotate => "arrows-rotate",
    ChevronLeft => "chevron-left",
    ChevronRight => "chevron-right",
    CircleCheck => "circle-check",
    CircleXmark => "circle-xmark",
    Download => "download",
    Eye => "eye",
    EyeDropper => "eye-dropper",
    FolderOpen => "folder-open",
    Gear => "gear",
    Plus => "plus",
    RotateLeft => "rotate-left",
    RotateRight => "rotate-right",
    TrashCan => "trash-can",
    Xmark => "xmark",
}

impl Icon {
    pub fn from_name(name: &str) -> Option<Icon> {
        Self::ALL.iter().copied().find(|i| i.name() == name)
    }
}

/// Icon height relative to the font size of the text next to it.
const EM: f32 = 0.85;

/// The value of attribute `name` in the first tag of `s` that has it.
fn attr<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let start = s.find(&key)? + key.len();
    let len = s[start..].find('"')?;
    Some(&s[start..start + len])
}

struct Outline {
    path: kurbo::BezPath,
    /// The `viewBox`: its height is what the caller's icon height means.
    view: kurbo::Rect,
    /// `view` grown to the path's bounds: Font Awesome paths may overshoot the viewBox.
    frame: kurbo::Rect,
}

fn outline(icon: Icon) -> Outline {
    let svg = icon.svg();
    let vb: Vec<f64> = attr(svg, "viewBox")
        .map(|v| v.split_whitespace().filter_map(|n| n.parse().ok()).collect())
        .unwrap_or_default();
    assert!(vb.len() == 4 && vb[3] > 0.0, "{}.svg: bad viewBox", icon.name());
    let view = kurbo::Rect::new(vb[0], vb[1], vb[0] + vb[2], vb[1] + vb[3]);
    let d = svg.find("<path").and_then(|i| attr(&svg[i..], "d"));
    let path = d
        .and_then(|d| kurbo::BezPath::from_svg(d).ok())
        .unwrap_or_else(|| panic!("{}.svg: no parsable <path d=…>", icon.name()));
    let frame = view.union(kurbo::Shape::bounding_box(&path));
    Outline { path, view, frame }
}

/// White, premultiplied mask of `icon` whose viewBox is `height_px` pixels tall.
pub fn rasterize(icon: Icon, height_px: u32) -> ColorImage {
    let o = outline(icon);
    let s = f64::from(height_px.max(1)) / o.view.height();
    let w = (o.frame.width() * s).ceil().clamp(1.0, 2048.0) as u16;
    let h = (o.frame.height() * s).ceil().clamp(1.0, 2048.0) as u16;
    let mut ctx = vello_cpu::RenderContext::new(w, h);
    ctx.set_transform(kurbo::Affine::scale(s) * kurbo::Affine::translate((-o.frame.x0, -o.frame.y0)));
    ctx.set_paint(color::OpaqueColor::<color::Srgb>::WHITE);
    ctx.fill_path(&o.path);
    let mut dest = vello_cpu::Pixmap::new(w, h);
    ctx.render(&mut dest, &mut vello_cpu::Resources::new());
    let pixels = dest
        .data_as_u8_slice()
        .chunks_exact(4)
        .map(|p| Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3]))
        .collect();
    ColorImage::new([usize::from(w), usize::from(h)], pixels)
}

/// Textures by (icon, viewBox height in physical pixels).
#[derive(Clone, Default)]
struct Cache(Arc<Mutex<HashMap<(Icon, u32), TextureHandle>>>);

/// `icon` as a texture whose viewBox is `height` points tall, sized in points.
pub fn texture(ctx: &egui::Context, icon: Icon, height: f32) -> SizedTexture {
    let ppp = ctx.pixels_per_point();
    let px = (height * ppp).round().max(1.0) as u32;
    let cache = ctx.data_mut(|d| d.get_temp_mut_or_default::<Cache>(egui::Id::unique("plot2csv_icons")).clone());
    let mut map = cache.0.lock().unwrap_or_else(|e| e.into_inner());
    // Zooming leaves textures for old scales behind: drop them all now and then.
    if map.len() > 256 {
        map.clear();
    }
    let tex = map
        .entry((icon, px))
        .or_insert_with(|| ctx.load_texture(format!("icon-{}-{px}", icon.name()), rasterize(icon, px), TextureOptions::LINEAR));
    SizedTexture::new(tex.id(), tex.size_vec2() / ppp)
}

fn font_size(ui: &egui::Ui, style: &TextStyle) -> f32 {
    style.resolve(ui.style()).size
}

/// `icon` `height` points tall, tinted `color`.
pub fn sized(ctx: &egui::Context, icon: Icon, height: f32, color: Color32) -> Image<'static> {
    let t = texture(ctx, icon, height);
    Image::from_texture(t).fit_to_exact_size(t.size).tint(color)
}

/// `icon` sized and coloured for a button's text, e.g. `ui.button((icons::image(ui, Icon::Plus), "Add"))`.
pub fn image(ui: &egui::Ui, icon: Icon) -> Image<'static> {
    image_tinted(ui, icon, ui.visuals().widgets.inactive.text_color())
}

/// [`image`] in another colour.
pub fn image_tinted(ui: &egui::Ui, icon: Icon, color: Color32) -> Image<'static> {
    sized(ui.ctx(), icon, EM * font_size(ui, &TextStyle::Button), color)
}

/// Paint `icon` `height` points tall, centred on `center`.
pub fn paint(ui: &egui::Ui, icon: Icon, center: Pos2, height: f32, color: Color32) {
    let t = texture(ui.ctx(), icon, height);
    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    ui.painter().image(t.id, Rect::from_center_size(center, t.size), uv, color);
}

/// A wrapping label in `style` and `color` whose `{icon-name}` tokens are drawn as icons.
///
/// Tokens that name no icon are shown as written.
pub fn rich_label(ui: &mut egui::Ui, text: &str, style: TextStyle, color: Color32) {
    let height = EM * font_size(ui, &style);
    ui.horizontal_wrapped(|ui| {
        // Pieces sit flush; rows are as far apart as the lines of one wrapped label.
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
        let text_part = |ui: &mut egui::Ui, s: &str| {
            if !s.is_empty() {
                ui.label(RichText::new(s).text_style(style.clone()).color(color));
            }
        };
        let mut rest = text;
        let mut plain = 0; // bytes of `rest` known to hold no icon token
        while let Some(open) = rest[plain..].find('{').map(|i| plain + i) {
            let Some(close) = rest[open..].find('}').map(|i| open + i) else { break };
            match Icon::from_name(&rest[open + 1..close]) {
                Some(icon) => {
                    text_part(ui, &rest[..open]);
                    ui.add(sized(ui.ctx(), icon, height, color));
                    rest = &rest[close + 1..];
                    plain = 0;
                }
                None => plain = close + 1,
            }
        }
        text_part(ui, rest);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_rasterizes() {
        for &icon in Icon::ALL {
            for px in [16, 32] {
                let img = rasterize(icon, px);
                let [w, h] = img.size;
                assert!(h >= px as usize && w > 0, "{}: {w}x{h} at {px}px", icon.name());
                let ink = img.pixels.iter().filter(|c| c.a() > 128).count();
                assert!(ink * 10 > w * h, "{}: only {ink} of {} pixels inked at {px}px", icon.name(), w * h);
                assert!(img.pixels.iter().any(|c| c.a() == 0), "{}: no transparent pixels", icon.name());
            }
        }
    }

    #[test]
    fn names_round_trip() {
        for &icon in Icon::ALL {
            assert_eq!(Icon::from_name(icon.name()), Some(icon));
        }
        assert_eq!(Icon::from_name("nope"), None);
    }
}
