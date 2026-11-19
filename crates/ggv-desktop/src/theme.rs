//! The interface's look: a light theme of off-white surfaces, slate text and one blue accent
//! (with a matching dark one), and the toolbar's building blocks — grouped controls, painted
//! icons, the coordinate field and chips — so every control shares one size, spacing and style.
//!
//! Icons are drawn with the painter rather than taken from a font: the bundled fonts lack most
//! symbols, and drawn strokes look the same at every scale and in both themes.

use eframe::egui::{
    self, Atom, Color32, CornerRadius, Frame, Margin, Painter, Pos2, Rect, Response, Shadow, Stroke, Ui, Vec2, Visuals,
    pos2, vec2,
};

/// The colours of one theme.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// The toolbar and status bar.
    pub surface: Color32,
    /// Fields, menus, grouped controls.
    pub raised: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub accent: Color32,
    /// Text and icons on an accent fill.
    pub on_accent: Color32,
    pub accent_hover: Color32,
    /// A tinted background for selected and changed things.
    pub accent_soft: Color32,
    pub accent_soft_border: Color32,
    /// Accent-coloured text on `accent_soft`.
    pub accent_text: Color32,
}

/// Slate and blue (Tailwind's slate and blue scales).
pub const LIGHT: Palette = Palette {
    surface: Color32::from_rgb(0xF8, 0xFA, 0xFC),
    raised: Color32::WHITE,
    hover: Color32::from_rgb(0xF1, 0xF5, 0xF9),
    pressed: Color32::from_rgb(0xE2, 0xE8, 0xF0),
    border: Color32::from_rgb(0xE2, 0xE8, 0xF0),
    border_strong: Color32::from_rgb(0xCB, 0xD5, 0xE1),
    text: Color32::from_rgb(0x1E, 0x29, 0x3B),
    text_muted: Color32::from_rgb(0x64, 0x74, 0x8B),
    accent: Color32::from_rgb(0x25, 0x63, 0xEB),
    on_accent: Color32::WHITE,
    accent_hover: Color32::from_rgb(0x1D, 0x4E, 0xD8),
    accent_soft: Color32::from_rgb(0xEF, 0xF6, 0xFF),
    accent_soft_border: Color32::from_rgb(0xBF, 0xDB, 0xFE),
    accent_text: Color32::from_rgb(0x1D, 0x4E, 0xD8),
};

pub const DARK: Palette = Palette {
    surface: Color32::from_rgb(0x14, 0x1B, 0x26),
    raised: Color32::from_rgb(0x1E, 0x29, 0x3B),
    hover: Color32::from_rgb(0x27, 0x33, 0x47),
    pressed: Color32::from_rgb(0x33, 0x41, 0x55),
    border: Color32::from_rgb(0x2B, 0x37, 0x4A),
    border_strong: Color32::from_rgb(0x47, 0x55, 0x69),
    text: Color32::from_rgb(0xE2, 0xE8, 0xF0),
    text_muted: Color32::from_rgb(0x94, 0xA3, 0xB8),
    accent: Color32::from_rgb(0x3B, 0x82, 0xF6),
    on_accent: Color32::WHITE,
    accent_hover: Color32::from_rgb(0x60, 0xA5, 0xFA),
    accent_soft: Color32::from_rgb(0x17, 0x25, 0x54),
    accent_soft_border: Color32::from_rgb(0x1E, 0x40, 0xAF),
    accent_text: Color32::from_rgb(0x93, 0xC5, 0xFD),
};

/// The palette of the theme `ui` is drawn in.
pub fn palette(ui: &Ui) -> &'static Palette {
    if ui.visuals().dark_mode { &DARK } else { &LIGHT }
}

/// Corner radius of buttons, fields and groups.
pub const RADIUS: u8 = 6;
/// The height of every toolbar control.
pub const CONTROL_H: f32 = 26.0;
/// An icon's box.
pub const ICON: f32 = 14.0;
/// An icon-only button's width.
pub const ICON_BUTTON_W: f32 = 28.0;
/// The toolbar's margin around its row.
pub const TOOLBAR_MARGIN: Margin = Margin { left: 10, right: 10, top: 5, bottom: 5 };
/// The room between two groups.
pub const GROUP_GAP: f32 = 10.0;

/// Installs the visuals of both themes (switching between them swaps whole styles).
pub fn install(ctx: &egui::Context) {
    ctx.style_mut_of(egui::Theme::Light, |s| apply(&mut s.visuals, &LIGHT));
    ctx.style_mut_of(egui::Theme::Dark, |s| apply(&mut s.visuals, &DARK));
}

fn apply(v: &mut Visuals, p: &Palette) {
    let r = CornerRadius::same(RADIUS);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;

    w.inactive.bg_fill = p.raised;
    w.inactive.weak_bg_fill = p.raised;
    w.inactive.bg_stroke = Stroke::new(1.0, p.border_strong);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;

    w.hovered.bg_fill = p.hover;
    w.hovered.weak_bg_fill = p.hover;
    w.hovered.bg_stroke = Stroke::new(1.0, p.border_strong);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.pressed;
    w.active.weak_bg_fill = p.pressed;
    w.active.bg_stroke = Stroke::new(1.0, p.border_strong);
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    w.open.bg_fill = p.pressed;
    w.open.weak_bg_fill = p.pressed;
    w.open.bg_stroke = Stroke::new(1.0, p.border_strong);
    w.open.fg_stroke = Stroke::new(1.0, p.text);
    w.open.corner_radius = r;

    // Selected buttons and selected text: a tinted fill, accent text; focus rings in the accent.
    v.selection.bg_fill = if v.dark_mode { p.accent_soft_border.gamma_multiply(0.7) } else { Color32::from_rgb(0xDB, 0xEA, 0xFE) };
    v.selection.stroke = Stroke::new(1.0, if v.dark_mode { p.accent_text } else { p.accent });
    v.hyperlink_color = p.accent;
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);

    v.panel_fill = p.surface;
    v.window_fill = p.raised;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(8);
    v.extreme_bg_color = p.raised;
    v.faint_bg_color = p.hover;
    v.code_bg_color = p.hover;
    let shadow = Color32::from_black_alpha(if v.dark_mode { 90 } else { 22 });
    v.popup_shadow = Shadow { offset: [0, 4], blur: 14, spread: 0, color: shadow };
    v.window_shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: shadow };
}

/// The toolbar's frame: the surface colour with a margin, its bottom edge drawn by the panel.
pub fn toolbar_frame(ctx: &egui::Context) -> Frame {
    let p = if ctx.global_style().visuals.dark_mode { &DARK } else { &LIGHT };
    Frame::new().fill(p.surface).inner_margin(TOOLBAR_MARGIN)
}

/// Sets the toolbar's spacing on `ui`: one control height, tight padding.
pub fn toolbar_spacing(ui: &mut Ui) {
    let s = ui.spacing_mut();
    s.interact_size.y = CONTROL_H;
    s.button_padding = vec2(8.0, 3.0);
    s.item_spacing = vec2(4.0, 6.0);
    s.icon_spacing = 6.0;
}

/// The icons the toolbar uses, drawn in the button's text colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Back,
    Forward,
    ZoomIn,
    ZoomOut,
    Search,
    Folder,
    Reads,
    Variants,
    Eye,
    Split,
    Evidence,
    Command,
    More,
    ChevronDown,
}

/// An icon as a button atom.
pub fn icon(icon: Icon) -> Atom<'static> {
    let size = if icon == Icon::ChevronDown { Vec2::new(8.0, ICON) } else { Vec2::splat(ICON) };
    Atom::paint(size, move |ui, args| paint_icon(ui.painter(), args.rect, icon, args.fallback_text_color))
}

/// An icon in `muted` (menus' chevrons), as a button atom.
fn muted_icon(icon: Icon, muted: Color32) -> Atom<'static> {
    let size = if icon == Icon::ChevronDown { Vec2::new(8.0, ICON) } else { Vec2::splat(ICON) };
    Atom::paint(size, move |ui, args| {
        // Follow the button when it's disabled (its text colour faded), else stay muted.
        let c = if args.fallback_text_color.a() < 200 { args.fallback_text_color } else { muted };
        paint_icon(ui.painter(), args.rect, icon, c)
    })
}

pub fn paint_icon(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = Stroke::new(1.5, color);
    let c = rect.center();
    let at = |x: f32, y: f32| pos2(c.x + x, c.y + y);
    let line = |a: Pos2, b: Pos2| painter.line_segment([a, b], s);
    let path = |pts: Vec<Pos2>| {
        painter.add(egui::Shape::line(pts, s));
    };
    match icon {
        Icon::Back => path(vec![at(2.0, -5.0), at(-3.0, 0.0), at(2.0, 5.0)]),
        Icon::Forward => path(vec![at(-2.0, -5.0), at(3.0, 0.0), at(-2.0, 5.0)]),
        Icon::ChevronDown => path(vec![at(-3.5, -1.5), at(0.0, 2.0), at(3.5, -1.5)]),
        Icon::ZoomOut | Icon::ZoomIn => {
            line(at(-5.0, 0.0), at(5.0, 0.0));
            if icon == Icon::ZoomIn {
                line(at(0.0, -5.0), at(0.0, 5.0));
            }
        }
        Icon::Search => {
            painter.circle_stroke(at(-1.5, -1.5), 4.5, s);
            line(at(1.8, 1.8), at(5.5, 5.5));
        }
        Icon::Folder => {
            path(vec![at(-6.0, 5.0), at(-6.0, -4.5), at(-2.0, -4.5), at(-0.5, -3.0), at(6.0, -3.0), at(6.0, 5.0), at(-6.0, 5.0)]);
            line(at(-6.0, -0.5), at(6.0, -0.5));
        }
        Icon::Reads => {
            // Staggered reads, as the track draws them.
            line(at(-6.0, -4.0), at(3.0, -4.0));
            line(at(-3.0, 0.0), at(6.0, 0.0));
            line(at(-6.0, 4.0), at(1.0, 4.0));
        }
        Icon::Variants => {
            // A lollipop: a variant's mark over its base.
            line(at(-6.0, 5.0), at(6.0, 5.0));
            line(at(0.0, 5.0), at(0.0, -1.0));
            painter.circle_stroke(at(0.0, -3.5), 2.5, s);
        }
        Icon::Eye => {
            let pts = |sign: f32| (0..=12).map(|i| {
                let t = i as f32 / 12.0;
                let x = -6.5 + 13.0 * t;
                at(x, sign * 4.2 * (1.0 - (2.0 * t - 1.0).powi(2)))
            }).collect::<Vec<_>>();
            path(pts(1.0));
            path(pts(-1.0));
            painter.circle_stroke(c, 1.9, s);
        }
        Icon::Split => {
            painter.rect_stroke(Rect::from_center_size(c, vec2(12.0, 10.0)), 2.0, s, egui::StrokeKind::Middle);
            line(at(0.0, -5.0), at(0.0, 5.0));
        }
        Icon::Evidence => {
            // Bars: counts at a base.
            line(at(-4.0, 5.0), at(-4.0, 0.0));
            line(at(0.0, 5.0), at(0.0, -5.0));
            line(at(4.0, 5.0), at(4.0, -2.0));
        }
        Icon::Command => {
            painter.rect_stroke(Rect::from_center_size(c, vec2(13.0, 11.0)), 2.5, s, egui::StrokeKind::Middle);
            path(vec![at(-3.5, -2.0), at(-1.0, 0.0), at(-3.5, 2.0)]);
            line(at(0.5, 2.5), at(3.5, 2.5));
        }
        Icon::More => {
            for x in [-4.5, 0.0, 4.5] {
                painter.circle_filled(at(x, 0.0), 1.4, color);
            }
        }
    }
}

/// Draws `add` as one group: controls side by side on a raised fill inside a hairline border,
/// the controls themselves frameless until hovered (a segmented control).
pub fn group<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = palette(ui);
    Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0, p.border_strong))
        .corner_radius(RADIUS)
        .inner_margin(Margin::same(0))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            // The border is the group's edge, so the controls inside lose a pixel each way.
            ui.spacing_mut().interact_size.y = CONTROL_H - 2.0;
            ui.horizontal(|ui| {
                let w = &mut ui.visuals_mut().widgets;
                for v in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
                    v.bg_stroke = Stroke::NONE;
                    v.corner_radius = CornerRadius::same(RADIUS - 1);
                }
                add(ui)
            })
            .inner
        })
        .inner
}

/// The extra width a group adds around its controls (its border).
pub const GROUP_EDGE: f32 = 2.0;

/// A button in a group: frameless until hovered.
fn plain<'a>(atoms: impl egui::IntoAtoms<'a>) -> egui::Button<'a> {
    egui::Button::new(atoms).frame_when_inactive(false)
}

/// An icon-only button with its name for hover and screen readers.
pub fn icon_button(ui: &mut Ui, which: Icon, enabled: bool, name: &str, hover: &str) -> Response {
    let r = ui.add_enabled(enabled, plain(icon(which)).min_size(vec2(ICON_BUTTON_W, 0.0)));
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, name));
    r.on_hover_text(hover).on_disabled_hover_text(hover)
}

/// A labelled button with a leading icon.
pub fn labeled<'a>(which: Icon, label: &'a str) -> egui::Button<'a> {
    plain((icon(which), label))
}

/// A menu's button: icon, label, and a chevron; `compact`, the icon and chevron only (name it
/// with [`name_menu`]).
pub fn menu_button<'a>(ui: &Ui, which: Icon, label: &'a str, compact: bool) -> egui::Button<'a> {
    let muted = palette(ui).text_muted;
    if compact {
        plain((icon(which), muted_icon(Icon::ChevronDown, muted)))
    } else {
        plain((icon(which), label, muted_icon(Icon::ChevronDown, muted)))
    }
}

/// Gives a compact menu button its name, for hover and screen readers.
pub fn name_menu(r: &Response, compact: bool, name: &str) {
    if compact {
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, r.enabled(), name));
        if r.hovered() {
            r.clone().on_hover_text(name);
        }
    }
}

/// A toggle in a group: tinted with accent text while on.
pub fn toggle(ui: &mut Ui, on: &mut bool, which: Icon, label: &str) -> Response {
    let p = palette(ui);
    let (text, fill) = if *on { (p.accent_text, p.accent_soft) } else { (p.text, Color32::TRANSPARENT) };
    let mut r = ui.add(plain((icon(which), egui::RichText::new(label).color(text))).fill(fill).frame_when_inactive(*on));
    if r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, *on, label));
    r
}

/// The primary action: an accent fill (the coordinate field's Go).
pub fn primary(ui: &mut Ui, label: &str) -> Response {
    let p = palette(ui);
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.weak_bg_fill = p.accent;
        w.hovered.weak_bg_fill = p.accent_hover;
        w.active.weak_bg_fill = p.accent_hover;
        for v in [&mut w.inactive, &mut w.hovered, &mut w.active] {
            v.fg_stroke = Stroke::new(1.0, p.on_accent);
            v.bg_stroke = Stroke::NONE;
        }
        ui.add(egui::Button::new(egui::RichText::new(label).strong()))
    })
    .inner
}

/// A settings chip: tinted, rounded fully, accent text.
pub fn chip<'a>(ui: &Ui, text: impl Into<egui::WidgetText>) -> egui::Button<'a> {
    let p = palette(ui);
    let text: egui::WidgetText = text.into();
    egui::Button::new(text.color(p.accent_text))
        .fill(p.accent_soft)
        .stroke(Stroke::new(1.0, p.accent_soft_border))
        .corner_radius(CONTROL_H / 2.0)
}

/// A key name as a small muted badge (`Ctrl+K`) inside a button.
pub fn key_badge(ui: &Ui, keys: &str) -> egui::RichText {
    let p = palette(ui);
    egui::RichText::new(keys).small().color(p.text_muted)
}

/// A thin vertical rule between groups of the toolbar, with room either side.
pub fn divider(ui: &mut Ui) {
    let p = palette(ui);
    let space = (GROUP_GAP - ui.spacing().item_spacing.x).max(0.0);
    ui.add_space(space);
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, CONTROL_H - 8.0), egui::Sense::hover());
    ui.painter().line_segment([rect.center_top(), rect.center_bottom()], Stroke::new(1.0, p.border));
    ui.add_space(space);
}

/// The width [`divider`] takes, with the item spacing after it.
pub fn divider_w(ui: &Ui) -> f32 {
    2.0 * GROUP_GAP.max(ui.spacing().item_spacing.x) + 1.0
}

/// A switch's size: a pill with a knob that slides to the right when on.
pub const SWITCH: Vec2 = vec2(28.0, 16.0);

/// Paints a switch in `rect` (see [`SWITCH`]): the accent fill while on, a neutral one while off.
pub fn paint_switch(painter: &Painter, rect: Rect, on: bool, p: &Palette) {
    let r = rect.height() / 2.0;
    painter.rect_filled(rect, r, if on { p.accent } else { p.border_strong });
    let x = if on { rect.right() - r } else { rect.left() + r };
    painter.circle_filled(pos2(x, rect.center().y), r - 2.0, Color32::WHITE);
}

/// A menu row with a switch on the right: the whole row toggles it.
pub fn switch_row(ui: &mut Ui, on: &mut bool, label: &str, hint: Option<&str>) -> Response {
    let p = palette(ui);
    let h = CONTROL_H;
    let (rect, mut r) = ui.allocate_exact_size(vec2(ui.available_width(), h), egui::Sense::click());
    if r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if r.hovered() {
            painter.rect_filled(rect, RADIUS, p.hover);
        }
        let font = egui::FontId::proportional(13.0);
        painter.text(pos2(rect.left() + 8.0, rect.center().y), egui::Align2::LEFT_CENTER, label, font, p.text);
        let sw = Rect::from_center_size(pos2(rect.right() - 8.0 - SWITCH.x / 2.0, rect.center().y), SWITCH);
        paint_switch(painter, sw, *on, p);
        if let Some(hint) = hint {
            painter.text(pos2(sw.left() - 10.0, rect.center().y), egui::Align2::RIGHT_CENTER, hint, egui::FontId::proportional(11.5), p.text_muted);
        }
    }
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, true, *on, label));
    r
}

/// A menu row that runs an action: its label on the left, a shortcut hint on the right.
pub fn action_row(ui: &mut Ui, label: &str, hint: Option<&str>) -> Response {
    let p = palette(ui);
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), CONTROL_H), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if r.hovered() {
            painter.rect_filled(rect, RADIUS, p.hover);
        }
        painter.text(pos2(rect.left() + 8.0, rect.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(13.0), p.text);
        if let Some(hint) = hint {
            painter.text(pos2(rect.right() - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, hint, egui::FontId::proportional(11.5), p.text_muted);
        }
    }
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    r
}

/// A hairline between groups of menu rows.
pub fn menu_rule(ui: &mut Ui) {
    let p = palette(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), egui::Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, p.border));
}
