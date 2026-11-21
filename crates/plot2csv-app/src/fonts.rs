//! The app's fonts: Inter for the UI, Hack for monospace text.
//!
//! Both are subsets (see `assets/fonts/README.md`). Icons are SVGs, not glyphs: see
//! [`crate::icons`]. On the web, eframe draws characters neither font has with the browser's
//! fonts.

use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily};

const INTER: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.subset.otf");
const HACK: &[u8] = include_bytes!("../assets/fonts/Hack-Regular.subset.ttf");

pub fn definitions() -> FontDefinitions {
    let mut defs = FontDefinitions::empty();
    defs.font_data.insert("Inter".into(), Arc::new(FontData::from_static(INTER)));
    defs.font_data.insert("Hack".into(), Arc::new(FontData::from_static(HACK)));
    defs.families.insert(FontFamily::Proportional, vec!["Inter".into()]);
    // Inter covers the few symbols Hack lacks (①–⑦, ⌘, ⌥).
    defs.families.insert(FontFamily::Monospace, vec!["Hack".into(), "Inter".into()]);
    defs
}

pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
}
