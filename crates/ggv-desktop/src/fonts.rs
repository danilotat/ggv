//! The interface font: Noto Sans Regular, bundled (OFL-1.1, see `fonts/OFL.txt`) so the app
//! looks the same on every system. It replaces egui's Ubuntu Light, which is thin and hard to
//! read at small sizes; its digits are all the same width, so coordinates and counts line up.
//! egui's emoji fonts stay as fallbacks, and Hack stays the monospace font.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

const NOTO_SANS: &[u8] = include_bytes!("../fonts/NotoSans-Regular.ttf");

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("NotoSans-Regular".into(), FontData::from_static(NOTO_SANS).into());
    if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
        family.insert(0, "NotoSans-Regular".into());
    }
    ctx.set_fonts(fonts);
}
