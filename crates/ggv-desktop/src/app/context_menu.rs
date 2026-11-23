//! The right-click menu on the tracks. Top to bottom: the clicked position (and the read under
//! the pointer), what to do at that position (sort, group, colour, read details), the display
//! switches, then utilities (copy, bookmark, track settings).
//!
//! It is drawn by hand rather than with egui's context menu so it can be driven from the
//! keyboard (arrows move, Right opens a submenu, Left closes it, Enter activates, Esc
//! dismisses), keep its switches open while they're toggled, and flip away from the window's
//! edges before it is first drawn (its size is known from its rows).
//!
//! Everything here is also an [`Action`] in the command palette.

use eframe::egui::{self, Align2, Color32, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};
use ggv_layout::{GroupBy, SortBy};

use super::{App, AlignedRead, Hit, actions::Action, group_label};
use crate::{
    session::{color_text, sort_name},
    theme,
    tracks::ColorBy,
    util::commas,
    variants::SiteColor,
};

const MENU_W: f32 = 300.0;
const SUB_W: f32 = 250.0;
const ROW_H: f32 = 26.0;
const RULE_H: f32 = 9.0;
const POS_H: f32 = 24.0;
const READ_H: f32 = 20.0;
/// The frame's margin (and its stroke) around the rows.
const MARGIN: f32 = 5.0;
/// The tallest a submenu grows before it scrolls (an INFO field per row can be many).
const SUB_MAX_H: f32 = 360.0;

/// The open menu: where it was opened and what was under the pointer.
#[derive(Clone, Debug)]
pub(super) struct ContextMenu {
    pub contig: usize,
    pub base: u64,
    pub read: Option<AlignedRead>,
    pub hit: Option<Hit>,
    /// The pointer when it was opened (the menu's corner, unless it flips).
    pub at: Pos2,
    focus: Option<usize>,
    sub: Option<Sub>,
    sub_focus: Option<usize>,
    /// Enter on the submenu's tag row: put the cursor in its field.
    focus_tag: bool,
    /// Where the menu and its submenu were drawn last frame, for clicks outside.
    drawn: Vec<Rect>,
}

impl ContextMenu {
    pub fn new(contig: usize, base: u64, read: Option<AlignedRead>, hit: Option<Hit>, at: Pos2) -> Self {
        Self { contig, base, read, hit, at, focus: None, sub: None, sub_focus: None, focus_tag: false, drawn: Vec::new() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Sub {
    Sort,
    Group,
    Color,
    /// A variant track's site colouring.
    Sites(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Toggle {
    SoftClips,
    LinkSplits,
    ViewPairs,
}

impl Toggle {
    fn label(self) -> &'static str {
        match self {
            Toggle::SoftClips => "Show soft-clipped bases",
            Toggle::LinkSplits => "Link split reads",
            Toggle::ViewPairs => "View as pairs",
        }
    }

    fn action(self) -> Action {
        match self {
            Toggle::SoftClips => Action::ToggleSoftClips,
            Toggle::LinkSplits => Action::ToggleLinkSplits,
            Toggle::ViewPairs => Action::ToggleViewPairs,
        }
    }
}

/// A row of the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Entry {
    SortBase,
    Open(Sub),
    ReadDetails,
    Toggle(Toggle),
    CopyPosition,
    Bookmark,
    TrackSettings,
    /// A variant track's switch: show only records whose FILTER is `PASS` (or `.`).
    PassOnly(usize),
}

impl Entry {
    /// The action the row runs, for its shortcut hint (an action whose keys do the same).
    fn action(self) -> Option<Action> {
        match self {
            Entry::Toggle(t) => Some(t.action()),
            Entry::Bookmark => Some(Action::AddBookmark),
            Entry::ReadDetails => Some(Action::ReadDetails),
            Entry::CopyPosition => Some(Action::CopyPosition),
            Entry::TrackSettings => Some(Action::TrackSettings),
            Entry::SortBase => Some(Action::Sort(SortBy::Base)),
            Entry::Open(_) | Entry::PassOnly(_) => None,
        }
    }
}

/// A row of a submenu.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Opt {
    Sort(Option<SortBy>),
    Group(GroupBy),
    Color(ColorBy),
    /// "Tag" with a field for its two letters (group or colour by any read tag).
    Tag,
    Sites(SiteColor),
}

const SORTS: [SortBy; 5] = [SortBy::Base, SortBy::Strand, SortBy::MappingQuality, SortBy::Start, SortBy::InsertSize];
const GROUPS: [GroupBy; 6] = [GroupBy::None, GroupBy::Strand, GroupBy::Haplotype, GroupBy::ReadGroup, GroupBy::Alleles, GroupBy::MateContig];
const COLORS: [ColorBy; 5] = [ColorBy::None, ColorBy::Strand, ColorBy::Haplotype, ColorBy::InsertSize, ColorBy::BaseMods];

/// `s` with its first letter capitalised ("MAPQ" stays).
fn sentence(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

/// A sort's name in the menu.
pub(super) fn sort_label(s: Option<SortBy>) -> String {
    s.map_or_else(|| "None".to_owned(), |by| sentence(sort_name(by)))
}

/// Where a box of `size` opened at `at` goes: below and right of it, flipped above or left of it
/// where it would cross `screen`'s edge, and kept on screen.
pub(super) fn place(at: Pos2, size: Vec2, screen: Rect) -> Pos2 {
    let x = if at.x + size.x > screen.right() { at.x - size.x } else { at.x };
    let y = if at.y + size.y > screen.bottom() { at.y - size.y } else { at.y };
    pos2(x.clamp(screen.left(), (screen.right() - size.x).max(screen.left())), y.clamp(screen.top(), (screen.bottom() - size.y).max(screen.top())))
}

/// Where a submenu of `size` goes beside its row `row` of the menu `menu`: to the right,
/// flipped to the left where it would cross `screen`'s edge, its first row level with `row`
/// and moved up where it would run off the bottom.
pub(super) fn place_sub(menu: Rect, row: Rect, size: Vec2, screen: Rect) -> Pos2 {
    let x = if menu.right() - 2.0 + size.x > screen.right() { menu.left() + 2.0 - size.x } else { menu.right() - 2.0 };
    let y = (row.top() - MARGIN).min(screen.bottom() - size.y);
    pos2(x.max(screen.left()), y.max(screen.top()))
}

impl App {
    /// The base the palette's position actions use: the outlined base, else the view's centre
    /// (as the toolbar's Reads menu does).
    pub(super) fn base_here(&self) -> u64 {
        match self.highlight {
            Some((c, p)) if c == self.contig => p,
            _ => self.center.max(0.0) as u64,
        }
    }

    /// The menu's rows, in groups divided by rules.
    pub(super) fn menu_entries(&self, m: &ContextMenu) -> Vec<Vec<Entry>> {
        let reads = matches!(m.hit, Some(Hit::Track(_))) || (m.hit.is_none() && !self.tracks.is_empty());
        let mut groups = Vec::new();
        if reads {
            let mut here = vec![Entry::SortBase, Entry::Open(Sub::Sort), Entry::Open(Sub::Group), Entry::Open(Sub::Color)];
            if m.read.is_some() {
                here.push(Entry::ReadDetails);
            }
            groups.push(here);
            groups.push([Toggle::SoftClips, Toggle::LinkSplits, Toggle::ViewPairs].map(Entry::Toggle).to_vec());
        }
        if let Some(Hit::Variants(i)) = m.hit
            && i < self.variants.len()
        {
            groups.push(vec![Entry::Open(Sub::Sites(i)), Entry::PassOnly(i)]);
        }
        let mut utils = vec![Entry::CopyPosition, Entry::Bookmark];
        if m.hit.is_some() {
            utils.push(Entry::TrackSettings);
        }
        groups.push(utils);
        groups
    }

    fn sub_opts(&self, m: &ContextMenu, sub: Sub) -> Vec<Opt> {
        match sub {
            Sub::Sort => SORTS.iter().map(|&s| Opt::Sort(Some(s))).chain([Opt::Sort(None)]).collect(),
            Sub::Group => GROUPS.iter().map(|&g| Opt::Group(g)).chain([Opt::Group(GroupBy::BaseAt(m.base)), Opt::Tag]).collect(),
            Sub::Color => COLORS.iter().map(|&c| Opt::Color(c)).chain([Opt::Tag]).collect(),
            Sub::Sites(i) => {
                let Some(v) = self.variants.get(i) else { return Vec::new() };
                std::iter::once(Opt::Sites(SiteColor::AlleleFrequency)).chain(v.info_fields.iter().map(|f| Opt::Sites(SiteColor::Info(f.id.clone())))).collect()
            }
        }
    }

    fn entry_label(&self, m: &ContextMenu, e: Entry) -> String {
        match e {
            Entry::SortBase => format!("Sort by base at {}", commas(m.base + 1)),
            Entry::Open(Sub::Sort) => "Sort reads".into(),
            Entry::Open(Sub::Group) => "Group reads".into(),
            Entry::Open(Sub::Color) => "Color by".into(),
            Entry::Open(Sub::Sites(_)) => "Color sites".into(),
            Entry::ReadDetails => "Read details".into(),
            Entry::Toggle(t) => t.label().into(),
            Entry::CopyPosition => "Copy position".into(),
            Entry::Bookmark => "Bookmark position".into(),
            Entry::TrackSettings => "Track settings…".into(),
            Entry::PassOnly(_) => "Hide variants without PASS".into(),
        }
    }

    /// A submenu's current value, shown muted before its chevron.
    fn sub_value(&self, sub: Sub) -> String {
        match sub {
            Sub::Sort => sort_label(self.style.sort.map(|(_, by)| by)),
            Sub::Group => sentence(&group_label(self.style.group)),
            Sub::Color => sentence(&color_text(self.style.color).replace("tag:", "tag ")),
            Sub::Sites(i) => self.variants.get(i).map_or_else(String::new, |v| sentence(v.color.name())),
        }
    }

    fn opt_label(&self, o: &Opt) -> String {
        match o {
            Opt::Sort(s) => sort_label(*s),
            Opt::Group(GroupBy::BaseAt(p)) => format!("Base at {}", commas(p + 1)),
            Opt::Group(g) => sentence(&group_label(*g)),
            Opt::Color(c) => sentence(crate::session::color_name(*c)),
            Opt::Tag => "Tag".into(),
            Opt::Sites(SiteColor::AlleleFrequency) => "Allele frequency (AF)".into(),
            Opt::Sites(SiteColor::Info(id)) => id.clone(),
        }
    }

    fn opt_active(&self, sub: Sub, o: &Opt) -> bool {
        match o {
            Opt::Sort(s) => self.style.sort.map(|(_, by)| by) == *s,
            Opt::Group(g) => self.style.group == *g,
            Opt::Color(c) => self.style.color == *c,
            Opt::Tag => match sub {
                Sub::Group => matches!(self.style.group, GroupBy::Tag(_)),
                Sub::Color => matches!(self.style.color, ColorBy::Tag(_)),
                _ => false,
            },
            Opt::Sites(c) => matches!(sub, Sub::Sites(i) if self.variants.get(i).is_some_and(|v| v.color == *c)),
        }
    }

    /// Runs a row. Returns whether the menu stays open (switches and submenus).
    fn activate(&mut self, m: &mut ContextMenu, e: Entry, ctx: &egui::Context) -> bool {
        match e {
            Entry::SortBase => self.style.sort = Some((m.base, SortBy::Base)),
            Entry::Open(sub) => {
                m.sub = Some(sub);
                m.sub_focus = Some(0);
                return true;
            }
            Entry::ReadDetails => {
                if let (Some(read), Some(Hit::Track(i))) = (&m.read, m.hit) {
                    self.open_read_details(i, read.clone(), m.base);
                }
            }
            Entry::Toggle(t) => {
                self.run(t.action(), ctx);
                return true;
            }
            Entry::CopyPosition => {
                let name = self.contigs.get(m.contig).map_or("", |c| c.name.as_str()).to_owned();
                ctx.copy_text(format!("{name}:{}", m.base + 1));
                self.notify(format!("Copied {name}:{}", commas(m.base + 1)));
            }
            Entry::Bookmark => {
                // Outlines the base too, so Ctrl+D (which bookmarks the outlined base) agrees.
                self.highlight = Some((m.contig, m.base));
                let name = self.contigs.get(m.contig).map_or("", |c| c.name.as_str()).to_owned();
                let i = crate::bookmarks::add(
                    &mut self.bookmarks.borrow_mut(),
                    crate::bookmarks::Bookmark { contig: name.clone(), start: m.base, end: m.base + 1, note: String::new() },
                );
                self.show_bookmarks = true;
                self.edit_bookmark = Some(i);
                self.notify(format!("Bookmarked {name}:{}; add a note in the Bookmarks window", commas(m.base + 1)));
            }
            Entry::TrackSettings => {
                if let Some(hit) = m.hit {
                    self.open_track_settings(hit);
                }
            }
            Entry::PassOnly(i) => {
                if let Some(v) = self.variants.get_mut(i) {
                    use crate::variants::Sites;
                    v.sites = if v.sites == Sites::Passing { Sites::All } else { Sites::Passing };
                }
                return true;
            }
        }
        false
    }

    /// Runs a submenu row. Returns whether the menu stays open (the tag row, until a tag is typed).
    fn choose(&mut self, m: &mut ContextMenu, sub: Sub, o: Opt) -> bool {
        match o {
            Opt::Sort(s) => self.style.sort = s.map(|by| (m.base, by)),
            Opt::Group(g) => self.style.group = g,
            Opt::Color(c) => self.style.color = c,
            Opt::Tag => match ggv_layout::parse_tag_name(&self.tag_input) {
                Some(t) if sub == Sub::Group => self.style.group = GroupBy::Tag(t),
                Some(t) => self.style.color = ColorBy::Tag(t),
                None => {
                    m.focus_tag = true;
                    return true;
                }
            },
            Opt::Sites(c) => {
                if let Sub::Sites(i) = sub
                    && let Some(v) = self.variants.get_mut(i)
                {
                    v.color = c;
                }
            }
        }
        false
    }

    /// Keys and clicks outside, before anything else reads them (the arrows would pan, Escape
    /// clear the selection). Runs ahead of the shortcuts.
    pub(super) fn context_menu_input(&mut self, ctx: &egui::Context) {
        let Some(mut m) = self.ctx_menu.take() else { return };
        // A press outside the menu dismisses it (the press still does what it does there).
        let outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.press_origin().is_some_and(|p| !m.drawn.iter().any(|r| r.contains(p))));
        if outside && !m.drawn.is_empty() {
            return;
        }
        if !self.is_active {
            self.ctx_menu = Some(m);
            return;
        }
        // Typing a tag: the field has the keys (Escape leaves it).
        if ctx.memory(|mem| mem.focused().is_some()) {
            self.ctx_menu = Some(m);
            return;
        }
        let pressed = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
        if pressed(Key::Escape) {
            return;
        }
        let entries: Vec<Entry> = self.menu_entries(&m).concat();
        let step = |at: Option<usize>, n: usize, down: bool| -> Option<usize> {
            (n > 0).then(|| match at {
                None => if down { 0 } else { n - 1 },
                Some(i) if down => (i + 1) % n,
                Some(i) => (i + n - 1) % n,
            })
        };
        for down in [true, false] {
            // Every press this frame (a held key repeats).
            let n = ctx.input_mut(|i| i.count_and_consume_key(Modifiers::NONE, if down { Key::ArrowDown } else { Key::ArrowUp }));
            for _ in 0..n {
                match m.sub {
                    Some(sub) => m.sub_focus = step(m.sub_focus, self.sub_opts(&m, sub).len(), down),
                    None => m.focus = step(m.focus, entries.len(), down),
                }
            }
        }
        if pressed(Key::ArrowRight)
            && m.sub.is_none()
            && let Some(Entry::Open(sub)) = m.focus.and_then(|i| entries.get(i).copied())
        {
            m.sub = Some(sub);
            m.sub_focus = Some(0);
        }
        if pressed(Key::ArrowLeft) && m.sub.is_some() {
            m.sub = None;
            m.sub_focus = None;
        }
        if pressed(Key::Enter) {
            let keep = match m.sub {
                Some(sub) => match m.sub_focus.and_then(|i| self.sub_opts(&m, sub).get(i).cloned()) {
                    Some(o) => self.choose(&mut m, sub, o),
                    None => true,
                },
                None => match m.focus.and_then(|i| entries.get(i).copied()) {
                    Some(e) => self.activate(&mut m, e, ctx),
                    None => true,
                },
            };
            if !keep {
                return;
            }
        }
        self.ctx_menu = Some(m);
    }

    /// Draws the open menu (and its submenu) over everything.
    pub(super) fn context_menu_ui(&mut self, ctx: &egui::Context) {
        let Some(mut m) = self.ctx_menu.take() else { return };
        if m.contig >= self.contigs.len() {
            return;
        }
        let groups = self.menu_entries(&m);
        let rows: usize = groups.iter().map(Vec::len).sum();
        let head = POS_H + if m.read.is_some() { READ_H } else { 0.0 };
        let h = head + rows as f32 * ROW_H + groups.len() as f32 * RULE_H + 2.0 * MARGIN;
        let screen = ctx.content_rect();
        let at = place(m.at, vec2(MENU_W, h), screen);
        let id = egui::Id::unique(("context-menu", self.reference_slot));
        let mut keep = true;
        let mut sub_row = None;
        let mut hovered_row = None;
        let frame = |ui: &egui::Ui| egui::Frame::popup(ui.style()).inner_margin(MARGIN - 1.0);
        let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(at).constrain(false).show(ctx, |ui| {
            frame(ui).show(ui, |ui| {
                ui.set_width(MENU_W - 2.0 * MARGIN);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                keep &= self.menu_header(ui, &m);
                let mut k = 0;
                for group in &groups {
                    theme::menu_rule(ui);
                    for &e in group {
                        let (r, rect) = self.menu_row(ui, &m, e, m.focus == Some(k));
                        if r.hovered() && ui.ctx().input(|i| i.pointer.is_moving() || i.pointer.any_pressed()) {
                            hovered_row = Some((k, e));
                        }
                        if let Entry::Open(sub) = e
                            && m.sub == Some(sub)
                        {
                            sub_row = Some(rect);
                        }
                        if r.clicked() {
                            keep &= self.activate(&mut m, e, ui.ctx());
                        }
                        k += 1;
                    }
                }
            });
        });
        // The pointer moves focus; resting on a submenu's row opens it, on another row closes it.
        if let Some((k, e)) = hovered_row {
            m.focus = Some(k);
            match e {
                Entry::Open(sub) if m.sub != Some(sub) => {
                    m.sub = Some(sub);
                    m.sub_focus = None;
                }
                Entry::Open(_) => {}
                _ => {
                    m.sub = None;
                    m.sub_focus = None;
                }
            }
        }
        let mut drawn = vec![area.response.rect];
        if keep
            && let Some(sub) = m.sub
            && let Some(row) = sub_row
        {
            let opts = self.sub_opts(&m, sub);
            let content = (opts.len() as f32 * ROW_H).min(SUB_MAX_H);
            let size = vec2(SUB_W, content + 2.0 * MARGIN);
            let pos = place_sub(area.response.rect, row, size, screen);
            let mut picked = None;
            let mut tag_rect = None;
            let sub_area = egui::Area::new(id.with("sub")).order(egui::Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
                frame(ui).show(ui, |ui| {
                    ui.set_width(SUB_W - 2.0 * MARGIN);
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    egui::ScrollArea::vertical().max_height(SUB_MAX_H).show(ui, |ui| {
                        for (k, o) in opts.iter().enumerate() {
                            let focused = m.sub_focus == Some(k);
                            let r = if *o == Opt::Tag {
                                let (r, field) = self.tag_row(ui, sub, focused, m.focus_tag);
                                tag_rect = Some(field);
                                r
                            } else {
                                self.opt_row(ui, sub, o, focused)
                            };
                            if focused && m.sub_focus.is_some() {
                                r.scroll_to_me(None);
                            }
                            if r.hovered() && ui.ctx().input(|i| i.pointer.is_moving()) {
                                m.sub_focus = Some(k);
                            }
                            if r.clicked() {
                                picked = Some(o.clone());
                            }
                        }
                    });
                });
            });
            m.focus_tag = false;
            // Enter in the tag field applies the tag.
            if tag_rect.is_some() && ctx.input(|i| i.key_pressed(Key::Enter)) && ggv_layout::parse_tag_name(&self.tag_input).is_some() && ctx.memory(|mem| mem.focused().is_none()) {
                picked = Some(Opt::Tag);
            }
            if let Some(o) = picked {
                keep &= self.choose(&mut m, sub, o);
            }
            drawn.push(sub_area.response.rect);
        }
        if keep {
            m.drawn = drawn;
            self.ctx_menu = Some(m);
        }
    }

    /// The position, and the read under the pointer with a copy button on hover. Returns false
    /// when the read name was copied (the menu closes).
    fn menu_header(&mut self, ui: &mut egui::Ui, m: &ContextMenu) -> bool {
        let p = theme::palette(ui);
        let name = self.contigs.get(m.contig).map_or("", |c| c.name.as_str());
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), POS_H), Sense::hover());
        ui.painter().text(
            pos2(rect.left() + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            format!("{name}:{}", commas(m.base + 1)),
            FontId::new(13.5, crate::fonts::medium()),
            p.text,
        );
        let Some(read) = &m.read else { return true };
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), READ_H), Sense::hover());
        let hovered = r.hovered() || ui.rect_contains_pointer(rect);
        let copy = Rect::from_min_max(pos2(rect.right() - 46.0, rect.top() + 1.0), pos2(rect.right() - 4.0, rect.bottom() - 1.0));
        let room = if hovered { copy.left() - 6.0 } else { rect.right() - 8.0 } - (rect.left() + 8.0);
        let font = FontId::monospace(11.5);
        let galley = truncated(ui, &read.name, font, p.text_muted, room);
        ui.painter().galley(pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0), galley, p.text_muted);
        if hovered {
            let b = ui.put(copy, egui::Button::new(egui::RichText::new("Copy").small()).frame_when_inactive(false)).on_hover_text("Copy the read name");
            if b.clicked() {
                ui.ctx().copy_text(read.name.clone());
                self.notify("Copied the read name");
                return false;
            }
        }
        if r.hovered() {
            r.on_hover_text(&read.name);
        }
        true
    }

    /// One row: label, then a switch, or the current value and a chevron, or a shortcut hint.
    fn menu_row(&mut self, ui: &mut egui::Ui, m: &ContextMenu, e: Entry, focused: bool) -> (egui::Response, Rect) {
        let p = theme::palette(ui);
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
        // Keys are the menu's own (see `context_menu_input`): no widget keeps focus, so Enter
        // isn't also a click on whatever was clicked last.
        if r.has_focus() {
            ui.memory_mut(|mem| mem.surrender_focus(r.id));
        }
        let label = self.entry_label(m, e);
        let open = matches!(e, Entry::Open(s) if m.sub == Some(s));
        let painter = ui.painter();
        if focused || open {
            painter.rect_filled(rect, theme::RADIUS, p.hover);
        }
        let y = rect.center().y;
        painter.text(pos2(rect.left() + 8.0, y), Align2::LEFT_CENTER, &label, FontId::proportional(13.0), p.text);
        let mut right = rect.right() - 8.0;
        match e {
            Entry::Toggle(_) | Entry::PassOnly(_) => {
                let on = match e {
                    Entry::Toggle(Toggle::SoftClips) => self.style.soft_clips,
                    Entry::Toggle(Toggle::LinkSplits) => self.style.link_splits,
                    Entry::Toggle(Toggle::ViewPairs) => self.style.view_pairs,
                    Entry::PassOnly(i) => self.variants.get(i).is_some_and(|v| v.sites == crate::variants::Sites::Passing),
                    _ => false,
                };
                let sw = Rect::from_center_size(pos2(right - theme::SWITCH.x / 2.0, y), theme::SWITCH);
                theme::paint_switch(painter, sw, on, p);
                right = sw.left() - 10.0;
                r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, true, on, &label));
            }
            Entry::Open(sub) => {
                let chevron = Rect::from_center_size(pos2(right - 4.0, y), vec2(8.0, 12.0));
                chevron_right(painter, chevron, p.text_muted);
                let value = self.sub_value(sub);
                let room = chevron.left() - 8.0 - (rect.left() + 8.0 + text_w(ui, &label, 13.0) + 16.0);
                let galley = truncated(ui, &value, FontId::proportional(12.0), p.text_muted, room.max(0.0));
                painter.galley(pos2(chevron.left() - 8.0 - galley.size().x, y - galley.size().y / 2.0), galley, p.text_muted);
                r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::MenuItem, true, format!("{label}: {}", self.sub_value(sub))));
                return (r, rect);
            }
            _ => r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::MenuItem, true, &label)),
        }
        if let Some(k) = e.action().and_then(Action::keys) {
            painter.text(pos2(right, y), Align2::RIGHT_CENTER, super::actions::keys_label(k), FontId::proportional(11.5), p.text_muted);
        }
        (r, rect)
    }

    /// A submenu row: a checkmark on the active choice, then its name.
    fn opt_row(&self, ui: &mut egui::Ui, sub: Sub, o: &Opt, focused: bool) -> egui::Response {
        let p = theme::palette(ui);
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
        if r.has_focus() {
            ui.memory_mut(|mem| mem.surrender_focus(r.id));
        }
        let active = self.opt_active(sub, o);
        let painter = ui.painter();
        if focused || r.hovered() {
            painter.rect_filled(rect, theme::RADIUS, p.hover);
        }
        if active {
            checkmark(painter, Rect::from_center_size(pos2(rect.left() + 14.0, rect.center().y), vec2(10.0, 10.0)), p.accent);
        }
        let label = self.opt_label(o);
        painter.text(pos2(rect.left() + 28.0, rect.center().y), Align2::LEFT_CENTER, &label, FontId::proportional(13.0), if active { p.accent_text } else { p.text });
        r.widget_info(|| egui::WidgetInfo::selected(egui::Role::MenuItemRadio, true, active, &label));
        if let (Opt::Sites(SiteColor::Info(id)), Sub::Sites(i)) = (o, sub)
            && let Some(f) = self.variants.get(i).and_then(|v| v.info_fields.iter().find(|f| &f.id == id))
        {
            return r.on_hover_text(&f.description);
        }
        r
    }

    /// The tag row: "Tag" and a two-letter field; clicking the row (or Enter in the field)
    /// applies a valid tag. Returns the row and the field.
    fn tag_row(&mut self, ui: &mut egui::Ui, sub: Sub, focused: bool, focus_field: bool) -> (egui::Response, Rect) {
        let p = theme::palette(ui);
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
        if r.has_focus() {
            ui.memory_mut(|mem| mem.surrender_focus(r.id));
        }
        let active = self.opt_active(sub, &Opt::Tag);
        if focused || r.hovered() {
            ui.painter().rect_filled(rect, theme::RADIUS, p.hover);
        }
        if active {
            checkmark(ui.painter(), Rect::from_center_size(pos2(rect.left() + 14.0, rect.center().y), vec2(10.0, 10.0)), p.accent);
        }
        ui.painter().text(pos2(rect.left() + 28.0, rect.center().y), Align2::LEFT_CENTER, "Tag", FontId::proportional(13.0), if active { p.accent_text } else { p.text });
        let field = Rect::from_min_size(pos2(rect.left() + 64.0, rect.top() + 3.0), vec2(44.0, ROW_H - 6.0));
        let edit = ui.put(field, egui::TextEdit::singleline(&mut self.tag_input).char_limit(2).hint_text("CB"));
        if focus_field {
            edit.request_focus();
        }
        ui.painter().text(
            pos2(rect.right() - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            "any read tag",
            FontId::proportional(11.5),
            p.text_muted,
        );
        (r.on_hover_text("Any read tag: CB (cell barcode), PS (phase set), UB, MI, an aligner's own..."), field)
    }
}

/// `text` laid out on one line, shortened with an ellipsis to fit `room`.
fn truncated(ui: &egui::Ui, text: &str, font: FontId, color: Color32, room: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(room.max(1.0));
    ui.fonts_mut(|f| f.layout_job(job))
}

fn text_w(ui: &egui::Ui, text: &str, size: f32) -> f32 {
    ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), FontId::proportional(size), Color32::WHITE).size().x)
}

fn chevron_right(painter: &egui::Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    painter.add(egui::Shape::line(vec![c + vec2(-2.0, -4.0), c + vec2(2.0, 0.0), c + vec2(-2.0, 4.0)], Stroke::new(1.5, color)));
}

fn checkmark(painter: &egui::Painter, rect: Rect, color: Color32) {
    let (l, b) = (rect.left(), rect.bottom());
    painter.add(egui::Shape::line(
        vec![pos2(l, rect.center().y), pos2(l + rect.width() * 0.38, b - 1.0), pos2(rect.right(), rect.top() + 1.0)],
        Stroke::new(1.8, color),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menus_flip_away_from_edges() {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0));
        let size = vec2(300.0, 400.0);
        assert_eq!(place(pos2(100.0, 100.0), size, screen), pos2(100.0, 100.0));
        // Near the right and bottom edges: left of and above the pointer.
        assert_eq!(place(pos2(900.0, 700.0), size, screen), pos2(600.0, 300.0));
        // Taller than the room either side: kept on screen.
        let p = place(pos2(500.0, 300.0), vec2(300.0, 790.0), screen);
        assert!(screen.contains_rect(Rect::from_min_size(p, vec2(300.0, 790.0))), "{p:?}");
        // A submenu flips to the left of a menu at the right edge.
        let menu = Rect::from_min_size(pos2(650.0, 100.0), vec2(300.0, 300.0));
        let row = Rect::from_min_size(pos2(650.0, 150.0), vec2(300.0, 26.0));
        let sub = place_sub(menu, row, vec2(250.0, 200.0), screen);
        assert!(sub.x + 250.0 <= menu.left() + 2.0, "{sub:?}");
        let low = Rect::from_min_size(pos2(100.0, 760.0), vec2(300.0, 26.0));
        let sub = place_sub(Rect::from_min_size(pos2(100.0, 500.0), vec2(300.0, 290.0)), low, vec2(250.0, 200.0), screen);
        assert!(sub.y + 200.0 <= 800.0);
    }

    fn key(k: Key) -> egui::Event {
        egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
    }

    /// One frame of the menu, as `windows` runs it, with `events`.
    fn frame(app: &mut App, ctx: &egui::Context, screen: Rect, events: Vec<egui::Event>) {
        let raw = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
        ctx.run_ui(raw, |ui| {
            let ctx = ui.ctx().clone();
            app.context_menu_input(&ctx);
            app.context_menu_ui(&ctx);
        })
        .textures_delta
        .clear();
    }

    /// Driven from the keyboard: switches stay open, submenus open and close with Right and
    /// Left, Enter chooses (and closes), Escape and a click outside dismiss; and the menu flips
    /// to stay on screen near the window's corner.
    #[test]
    fn keyboard_and_dismissal() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/hg002-revio");
        let bam = data.join("HG002.revio.slices.bam");
        if !bam.exists() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("ggv-context-menu-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("config")).unwrap();
        crate::store::TEST_CONFIG.with_borrow_mut(|c| *c = Some(dir.join("config")));
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, crate::app::Args { bams: vec![bam], locus: Some("chr20:58888838-58889038".into()), ..Default::default() }).unwrap();
        assert!(!app.contigs.is_empty());
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0));
        let open = |app: &mut App| app.ctx_menu = Some(ContextMenu::new(app.contig, 58_888_900, None, Some(Hit::Track(0)), pos2(1150.0, 760.0)));

        open(&mut app);
        frame(&mut app, &ctx, screen, vec![]);
        let rect = ctx.memory(|m| m.area_rect(egui::Id::unique(("context-menu", app.reference_slot)))).expect("shown");
        assert!(screen.contains_rect(rect), "on screen: {rect:?}");
        assert!(rect.right() <= 1151.0 && rect.bottom() <= 761.0, "flipped left of and above the pointer: {rect:?}");

        // Rows: sort at base, sort, group, color, then the three switches.
        let soft = app.style.soft_clips;
        frame(&mut app, &ctx, screen, vec![key(Key::ArrowDown); 5]);
        frame(&mut app, &ctx, screen, vec![key(Key::Enter)]);
        assert_ne!(app.style.soft_clips, soft, "Enter on the switch toggles it");
        assert!(app.ctx_menu.is_some(), "and the menu stays open");

        // Up to "Group reads", Right opens it, Left closes it, Right again, Down, Enter.
        frame(&mut app, &ctx, screen, vec![key(Key::ArrowUp), key(Key::ArrowUp)]);
        frame(&mut app, &ctx, screen, vec![key(Key::ArrowRight)]);
        assert_eq!(app.ctx_menu.as_ref().unwrap().sub, Some(Sub::Group));
        frame(&mut app, &ctx, screen, vec![key(Key::ArrowLeft)]);
        assert_eq!(app.ctx_menu.as_ref().unwrap().sub, None);
        frame(&mut app, &ctx, screen, vec![key(Key::ArrowRight)]);
        frame(&mut app, &ctx, screen, vec![]);
        let sub = ctx.memory(|m| m.area_rect(egui::Id::unique(("context-menu", app.reference_slot)).with("sub"))).expect("submenu shown");
        assert!(screen.contains_rect(sub) && sub.right() <= rect.left() + 3.0, "submenu flipped left: {sub:?}");
        frame(&mut app, &ctx, screen, vec![key(Key::ArrowDown), key(Key::Enter)]);
        assert_eq!(app.style.group, GroupBy::Strand);
        assert!(app.ctx_menu.is_none(), "choosing closes the menu");

        // "Sort by base at" runs at once and closes.
        open(&mut app);
        frame(&mut app, &ctx, screen, vec![]);
        frame(&mut app, &ctx, screen, vec![key(Key::ArrowDown), key(Key::Enter)]);
        assert_eq!(app.style.sort, Some((58_888_900, SortBy::Base)));
        assert!(app.ctx_menu.is_none());

        open(&mut app);
        frame(&mut app, &ctx, screen, vec![]);
        frame(&mut app, &ctx, screen, vec![key(Key::Escape)]);
        assert!(app.ctx_menu.is_none(), "Escape dismisses");

        open(&mut app);
        frame(&mut app, &ctx, screen, vec![]);
        let p = pos2(50.0, 50.0);
        frame(&mut app, &ctx, screen, vec![egui::Event::PointerMoved(p), egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        assert!(app.ctx_menu.is_none(), "a click outside dismisses");
    }

    #[test]
    fn every_entry_is_in_the_palette() {
        let all = Action::all();
        for e in [Entry::SortBase, Entry::ReadDetails, Entry::CopyPosition, Entry::Bookmark, Entry::TrackSettings]
            .into_iter()
            .chain([Toggle::SoftClips, Toggle::LinkSplits, Toggle::ViewPairs].map(Entry::Toggle))
        {
            assert!(all.contains(&e.action().unwrap()), "{e:?}");
        }
        for s in SORTS {
            assert!(all.contains(&Action::Sort(s)));
        }
        assert!(all.contains(&Action::ClearSort) && all.contains(&Action::GroupBaseHere));
        for g in GROUPS {
            assert!(all.contains(&Action::Group(g)));
        }
        for c in COLORS {
            assert!(all.contains(&Action::Color(c)));
        }
    }

    #[test]
    fn sentence_case() {
        assert_eq!(sentence("insert size"), "Insert size");
        assert_eq!(sentence("MAPQ"), "MAPQ");
        assert_eq!(sort_label(None), "None");
    }
}
