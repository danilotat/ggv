//! The interface's type: Figtree (Regular, and Medium for track titles), with Noto Sans
//! Regular behind it for what Figtree lacks (Greek, ✓, ▸, ●, ⌘…), then egui's emoji fonts.
//! Hack stays the monospace font (ruler, coordinates, key names). All bundled, OFL-1.1 (see
//! `fonts/OFL-*.txt`), so the app looks the same on every system.
//!
//! Figtree's digits are proportional ("1" is narrower than "0") and egui can't switch on
//! tabular figures, so numbers meant to line up in columns use the monospace font.
//!
//! Text sizes are a step up from egui's defaults, which read small next to the desktop's own.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontId, TextStyle};

const FIGTREE: &[u8] = include_bytes!("../fonts/Figtree-Regular.ttf");
const FIGTREE_MEDIUM: &[u8] = include_bytes!("../fonts/Figtree-Medium.ttf");
const NOTO_SANS: &[u8] = include_bytes!("../fonts/NotoSans-Regular.ttf");

/// The family for titles (track headers): Figtree Medium.
pub fn medium() -> FontFamily {
    FontFamily::Name("medium".into())
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [("Figtree-Regular", FIGTREE), ("Figtree-Medium", FIGTREE_MEDIUM), ("NotoSans-Regular", NOTO_SANS)] {
        fonts.font_data.insert(name.into(), FontData::from_static(bytes).into());
    }
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.splice(0..0, ["Figtree-Regular".to_owned(), "NotoSans-Regular".to_owned()]);
    let mut titles = vec!["Figtree-Medium".to_owned()];
    titles.extend(proportional.iter().cloned());
    fonts.families.insert(medium(), titles);
    ctx.set_fonts(fonts);

    // Both themes: switching between light and dark swaps whole styles.
    ctx.all_styles_mut(|style| {
        use FontFamily::{Monospace, Proportional};
        style.text_styles = [
            (TextStyle::Small, FontId::new(11.0, Proportional)),
            (TextStyle::Body, FontId::new(14.0, Proportional)),
            (TextStyle::Button, FontId::new(14.0, Proportional)),
            (TextStyle::Heading, FontId::new(19.0, Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, Monospace)),
        ]
        .into();
    });
}
