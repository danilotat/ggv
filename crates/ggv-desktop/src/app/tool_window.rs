//! The frame every tool window shares: a title bar of our own (egui's centres the title in the
//! heading style and adds a collapse arrow), drawn edge to edge in the surface colour with the
//! title left-aligned at body size and a close button on the right; then the content inside one
//! margin, its sections one gap apart.
//!
//! The window is drawn with `title_bar(false)`: dragging the bar (or any empty part of the
//! window) moves it, and its edges resize it.

use eframe::egui::{self, Align2, CornerRadius, FontId, Frame, Id, Margin, Pos2, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};

use crate::theme;

/// The title bar's height: one control row and a little.
pub const TITLE_H: f32 = theme::CONTROL_H + 6.0;
/// The margin around a tool window's content.
pub const MARGIN: i8 = 12;
/// The space between a tool window's sections (rows of controls, a note, the results).
pub const GAP: f32 = 8.0;
/// The title's size: body text, one step up.
const TITLE_SIZE: f32 = 14.5;

/// Where a tool window opens.
enum Spot {
    /// egui's choice.
    Auto,
    /// Beside what it mustn't hide (see [`super::inspector::placed`]), once the pane is settled.
    Placed((Pos2, Align2), bool),
    /// At a point, by a corner or the centre, then wherever it is dragged.
    At(Pos2, Align2),
    /// Fixed at a point of the screen (a dialog): not movable.
    Anchored(Align2, Vec2),
}

/// A tool window: title, id, where it first opens, and its size.
pub struct ToolWindow<'a> {
    title: &'a str,
    id: Id,
    spot: Spot,
    default_width: f32,
    min_width: f32,
    default_size: Option<Vec2>,
    max_size: Option<Vec2>,
    resizable: bool,
    closable: bool,
}

impl<'a> ToolWindow<'a> {
    pub fn new(title: &'a str, id: Id) -> Self {
        Self { title, id, spot: Spot::Auto, default_width: 420.0, min_width: 280.0, default_size: None, max_size: None, resizable: true, closable: true }
    }

    /// Where it first opens (see [`super::inspector::placed`]): beside what it mustn't hide, once
    /// the pane is `settled`.
    pub fn placed(mut self, spot: (Pos2, Align2), settled: bool) -> Self {
        self.spot = Spot::Placed(spot, settled);
        self
    }

    /// First opens with its `pivot` at `pos`; it can then be dragged.
    pub fn at(mut self, pos: Pos2, pivot: Align2) -> Self {
        self.spot = Spot::At(pos, pivot);
        self
    }

    /// Fixed with its `align` point at the screen's, offset by `offset` (a dialog): not movable.
    pub fn anchored(mut self, align: Align2, offset: Vec2) -> Self {
        self.spot = Spot::Anchored(align, offset);
        self
    }

    pub fn default_size(mut self, size: Vec2) -> Self {
        self.default_size = Some(size);
        self
    }

    pub fn max_size(mut self, size: Vec2) -> Self {
        self.max_size = Some(size);
        self
    }

    /// Whether its edges resize it (they do by default); one that isn't fits its content.
    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    /// No close button: a dialog closed by its own buttons.
    pub fn no_close(mut self) -> Self {
        self.closable = false;
        self
    }

    pub fn default_width(mut self, w: f32) -> Self {
        self.default_width = w;
        self
    }

    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }

    /// Shows the window while `open`; its close button clears `open`.
    pub fn show<R>(self, ctx: &egui::Context, open: &mut bool, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
        if !*open {
            return None;
        }
        let window = egui::Window::new(self.title);
        let mut window = match self.spot {
            Spot::Auto => window.id(self.id),
            Spot::Placed(spot, settled) => super::inspector::placed(ctx, window, self.id, spot, settled),
            Spot::At(pos, pivot) => window.id(self.id).pivot(pivot).default_pos(pos),
            Spot::Anchored(align, offset) => window.id(self.id).anchor(align, offset),
        };
        // A default size wins over the default width.
        window = match self.default_size {
            Some(size) => window.default_size(size),
            None => window.default_width(self.default_width),
        };
        if let Some(size) = self.max_size {
            window = window.max_size(size);
        }
        let style = ctx.global_style();
        let (title, closable) = (self.title, self.closable);
        let mut close = false;
        let out = window
            .title_bar(false)
            .collapsible(false)
            .resizable(self.resizable)
            .min_width(self.min_width)
            .frame(Frame::window(&style).inner_margin(0))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                close = title_bar(ui, title, closable);
                Frame::new()
                    .inner_margin(Margin::same(MARGIN))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = GAP;
                        add(ui)
                    })
                    .inner
            });
        if close {
            *open = false;
        }
        out.and_then(|r| r.inner)
    }
}

/// The bar: the surface colour under the window's rounded top corners, the title on the left, a
/// close button on the right (when `closable`), a hairline below. Returns whether close was
/// clicked.
fn title_bar(ui: &mut Ui, title: &str, closable: bool) -> bool {
    let p = theme::palette(ui);
    // Hover only: a press on the bar falls through to the window, which moves it.
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), TITLE_H), Sense::hover());
    let r = ui.visuals().window_corner_radius;
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius { nw: r.nw, ne: r.ne, sw: 0, se: 0 }, p.surface);
    painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, p.border));
    let left = rect.left() + f32::from(MARGIN);
    let close = Rect::from_center_size(pos2(rect.right() - f32::from(MARGIN) / 2.0 - 10.0, rect.center().y), vec2(theme::ICON_BUTTON_W - 4.0, theme::CONTROL_H - 4.0));
    let font = FontId::new(TITLE_SIZE, crate::fonts::medium());
    let mut job = egui::text::LayoutJob::simple_singleline(title.to_owned(), font, p.text);
    let right = if closable { close.left() - 8.0 } else { rect.right() - f32::from(MARGIN) };
    job.wrap = egui::text::TextWrapping::truncate_at_width((right - left).max(1.0));
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    painter.galley(pos2(left, rect.center().y - galley.size().y / 2.0), galley, p.text);
    if !closable {
        return false;
    }
    let button = ui.put(close, egui::Button::new(egui::RichText::new("×").size(17.0).color(p.text_muted)).frame_when_inactive(false));
    button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Close {title}")));
    button.on_hover_text("Close").clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ctx: &egui::Context, events: Vec<egui::Event>, open: &mut bool) {
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0))), events, ..Default::default() };
        ctx.run_ui(raw, |ui| {
            ToolWindow::new("Find sequence", Id::unique("tool-window-test")).default_width(400.0).show(ui.ctx(), open, |ui| ui.label("body"));
        })
        .textures_delta
        .clear();
    }

    /// The bar is ours (one control row and a little, no egui title), and its × closes the window.
    #[test]
    fn title_bar_closes() {
        let ctx = egui::Context::default();
        let mut open = true;
        for _ in 0..3 {
            frame(&ctx, vec![], &mut open);
        }
        let rect = ctx.memory(|m| m.area_rect(Id::unique("tool-window-test"))).expect("shown");
        assert!(rect.width() >= 400.0 - 1.0, "{rect:?}");
        // The close button: right end of the bar.
        let p = pos2(rect.right() - f32::from(MARGIN) / 2.0 - 10.0, rect.top() + TITLE_H / 2.0);
        let press = |pressed| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        frame(&ctx, vec![egui::Event::PointerMoved(p)], &mut open);
        frame(&ctx, vec![press(true)], &mut open);
        frame(&ctx, vec![press(false)], &mut open);
        assert!(!open, "closed by its button");
    }
}
