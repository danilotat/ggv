//! A track's settings panel, opened from the gear in its header or "Track settings…" in the
//! right-click menu: the track's name and the colour of its header bar, display switches that don't belong at a position (squish, splice
//! junctions), copying the view, and managing the track (filter, move, collapse, close).
//!
//! Also the read details window ("Read details" in the right-click menu, or the palette for the
//! selected read): the read's summary and what to do with it (copy, go to its mate or parts).

use std::path::PathBuf;

use eframe::egui::{self, Pos2, Rect, Stroke, Vec2};

use super::{
    AlignedRead, App, Hit, Layout,
    actions::Action,
    arrange::{self, Key},
};
use crate::{theme, util::commas};

/// The read whose details are open: the track's file (track indices change as tracks move),
/// the read, and the base it was opened at.
#[derive(Clone, Debug)]
pub(super) struct ReadDetails {
    pub path: PathBuf,
    pub read: AlignedRead,
    pub base: u64,
}

/// The Okabe–Ito colours (distinguishable with colour-vision deficiencies), in their usual order.
pub(super) const OKABE_ITO: [(&str, [u8; 3]); 8] = [
    ("Orange", [0xE6, 0x9F, 0x00]),
    ("Sky blue", [0x56, 0xB4, 0xE9]),
    ("Bluish green", [0x00, 0x9E, 0x73]),
    ("Yellow", [0xF0, 0xE4, 0x42]),
    ("Blue", [0x00, 0x72, 0xB2]),
    ("Vermillion", [0xD5, 0x5E, 0x00]),
    ("Reddish purple", [0xCC, 0x79, 0xA7]),
    ("Black", [0x00, 0x00, 0x00]),
];

/// The settings window's name and colour fields as typed, for the track `key`: the name and hex
/// text are kept as typed (an unfinished hex code doesn't apply), and whether "Custom" is open.
#[derive(Clone, Debug)]
pub(super) struct LookDraft {
    key: Key,
    name: String,
    hex: String,
    custom: bool,
}

impl LookDraft {
    fn of(key: &Key, arrangement: &arrange::Arrangement) -> Self {
        let color = arrangement.colors.get(key).copied();
        Self {
            key: key.clone(),
            name: arrangement.names.get(key).cloned().unwrap_or_default(),
            hex: color.map(arrange::hex).unwrap_or_default(),
            custom: color.is_some_and(|c| !OKABE_ITO.iter().any(|(_, o)| *o == c)),
        }
    }
}

/// A colour swatch, ringed in the accent colour while chosen; `slash` marks the default colour.
fn swatch(ui: &mut egui::Ui, color: egui::Color32, chosen: bool, slash: bool) -> egui::Response {
    let p = theme::palette(ui);
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let inner = rect.shrink(3.0);
        painter.rect_filled(inner, 3.0, color);
        painter.rect_stroke(inner, 3.0, Stroke::new(1.0, p.border_strong), egui::StrokeKind::Inside);
        if slash {
            painter.line_segment([inner.left_bottom(), inner.right_top()], Stroke::new(1.2, p.text_muted));
        }
        if chosen {
            painter.rect_stroke(rect.shrink(0.5), 4.0, Stroke::new(2.0, p.accent), egui::StrokeKind::Inside);
        } else if r.hovered() {
            painter.rect_stroke(rect.shrink(0.5), 4.0, Stroke::new(1.0, p.border_strong), egui::StrokeKind::Inside);
        }
    }
    r
}

impl Layout {
    /// The gear (track settings), just left of the collapse button.
    pub(super) fn gear_button(section: Rect) -> Rect {
        Layout::collapse_button(section).translate(Vec2::new(-22.0, 0.0))
    }
}

/// Draws the gear in a header: a ring with six teeth.
pub(super) fn paint_gear(painter: &egui::Painter, rect: Rect, color: egui::Color32) {
    let c = rect.center();
    let stroke = Stroke::new(1.4, color);
    painter.circle_stroke(c, 3.6, stroke);
    for k in 0..6 {
        let a = k as f32 * std::f32::consts::TAU / 6.0;
        let d = Vec2::angled(a);
        painter.line_segment([c + d * 3.6, c + d * 6.2], Stroke::new(2.2, color));
    }
    painter.circle_filled(c, 1.3, color);
}

impl App {
    pub(super) fn open_track_settings(&mut self, hit: Hit) {
        self.track_settings = self.key_of(hit);
        self.look_draft = None;
    }

    /// The track the palette's "Track settings…" opens: the one last right-clicked or opened,
    /// else the first alignment track, else the first track.
    pub(super) fn settings_target(&self) -> Option<Hit> {
        let last = self.track_settings.as_ref().or(self.menu_track.as_ref()).and_then(|k| self.hit_of(k));
        last.or_else(|| (!self.tracks.is_empty()).then_some(Hit::Track(0))).or_else(|| self.track_keys().first().and_then(|k| self.hit_of(k)))
    }

    pub(super) fn track_settings_window(&mut self, ctx: &egui::Context) {
        if !self.is_active {
            return;
        }
        let Some(key) = self.track_settings.clone() else { return };
        let Some(hit) = self.hit_of(&key) else {
            self.track_settings = None; // the track was closed
            return;
        };
        let title = match hit {
            Hit::Track(i) => self.tracks[i].full_title(),
            Hit::Genes(i) => self.genes[i].path.display().to_string(),
            Hit::Variants(i) => self.variants[i].path.display().to_string(),
            Hit::Signal(i) => self.signals[i].path.display().to_string(),
        };
        // The header bar's colour; the default swatch shows the theme's.
        let theme_header = self.pal().header;
        let default_name = match hit {
            Hit::Track(i) => self.tracks[i].title(),
            Hit::Genes(i) => format!("Genes  ·  {}", self.genes[i].file_name()),
            Hit::Variants(i) => format!("Variants  ·  {}", self.variants[i].file_name()),
            Hit::Signal(i) => format!("Signal  ·  {}", self.signals[i].file_name()),
        };
        let mut draft = match self.look_draft.take() {
            Some(d) if d.key == key => d,
            _ => LookDraft::of(&key, &self.arrangement),
        };
        let mut color = self.arrangement.colors.get(&key).copied();
        let mut open = true;
        let mut close = false;
        let mut run = None;
        let mut style = self.style;
        let collapsed = self.arrangement.collapsed.contains(&key);
        let spot = self.window_pos(ctx, 320.0);
        super::tool_window::ToolWindow::new("Track settings", egui::Id::unique(("track-settings", self.reference_slot)))
            .placed(spot, self.pane_settled())
            .resizable(false)
            .default_width(300.0)
            .show(ctx, &mut open, |ui| {
                ui.set_width(300.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                let p = theme::palette(ui);
                ui.add(egui::Label::new(egui::RichText::new(&title).small().color(p.text_muted)).truncate()).on_hover_text(&title);
                theme::menu_rule(ui);
                ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label("Name");
                    let edit = ui.add(egui::TextEdit::singleline(&mut draft.name).hint_text(&default_name).desired_width(ui.available_width() - 8.0));
                    edit.on_hover_text("The name in the track's header; empty for the file's name");
                });
                {
                    let default = [theme_header.r(), theme_header.g(), theme_header.b()];
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        ui.label("Header color");
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(6.0);
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let [r, g, b] = default;
                        if swatch(ui, egui::Color32::from_rgb(r, g, b), color.is_none() && !draft.custom, true).on_hover_text("Default").clicked() {
                            color = None;
                            draft.custom = false;
                            draft.hex.clear();
                        }
                        for (name, c) in OKABE_ITO {
                            let chosen = color == Some(c) && !draft.custom;
                            if swatch(ui, egui::Color32::from_rgb(c[0], c[1], c[2]), chosen, false).on_hover_text(format!("{name}  {}", arrange::hex(c))).clicked() {
                                color = Some(c);
                                draft.custom = false;
                                draft.hex = arrange::hex(c);
                            }
                        }
                        ui.add_space(4.0);
                        if ui.selectable_label(draft.custom, "Custom").on_hover_text("A hex code or a color from the picker").clicked() {
                            draft.custom = !draft.custom;
                        }
                    });
                    if draft.custom {
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            let mut rgb = color.unwrap_or(default);
                            if egui::color_picker::color_edit_button_srgb(ui, &mut rgb).changed() {
                                color = Some(rgb);
                                draft.hex = arrange::hex(rgb);
                            }
                            let valid = draft.hex.trim().is_empty() || arrange::parse_hex(&draft.hex).is_some();
                            let mut edit = egui::TextEdit::singleline(&mut draft.hex).hint_text("#rrggbb").desired_width(90.0).font(egui::TextStyle::Monospace);
                            if !valid {
                                edit = edit.text_color(egui::Color32::from_rgb(0xD5, 0x5E, 0x00));
                            }
                            if ui.add(edit).changed()
                                && let Some(c) = arrange::parse_hex(&draft.hex)
                            {
                                color = Some(c);
                            }
                            if !valid {
                                ui.label(egui::RichText::new("not a hex color").small().color(p.text_muted));
                            }
                        });
                    }
                }
                ui.add_space(4.0);
                ui.spacing_mut().item_spacing = Vec2::new(ui.spacing().item_spacing.x, 0.0);
                let hint = |a: Action| a.keys().map(super::actions::keys_label);
                if let Hit::Track(i) = hit {
                    theme::menu_rule(ui);
                    theme::switch_row(ui, &mut style.squished, "Squish", hint(Action::ToggleSquish).as_deref());
                    theme::switch_row(ui, &mut style.junctions, "Splice junctions", None)
                        .on_hover_text("Splice junctions (RNA-seq) as arcs over the coverage, with their read counts: red + strand, blue −, grey unknown");
                    ui.label(egui::RichText::new("Display settings apply to every alignment track in this pane.").small().color(p.text_muted));
                    theme::menu_rule(ui);
                    if theme::action_row(ui, "Filter reads in this track…", None).clicked() {
                        self.open_filter_window(Some(i));
                    }
                }
                theme::menu_rule(ui);
                let fasta = if self.selection.is_some() { "Copy selection as FASTA" } else { "Copy view as FASTA" };
                if theme::action_row(ui, fasta, hint(Action::CopyFasta).as_deref()).clicked() {
                    run = Some(Action::CopyFasta);
                }
                if matches!(hit, Hit::Track(_)) && self.selection.is_some() && theme::action_row(ui, "Copy selected reads as SAM", hint(Action::CopySam).as_deref()).clicked() {
                    run = Some(Action::CopySam);
                }
                if theme::action_row(ui, "Copy view image", hint(Action::CopyImage).as_deref()).clicked() {
                    run = Some(Action::CopyImage);
                }
                theme::menu_rule(ui);
                if theme::action_row(ui, "Move track up", None).clicked() {
                    self.move_track(hit, true);
                }
                if theme::action_row(ui, "Move track down", None).clicked() {
                    self.move_track(hit, false);
                }
                if theme::action_row(ui, if collapsed { "Expand track" } else { "Collapse track" }, None).clicked() {
                    self.arrangement.toggle_collapsed(&key);
                }
                if theme::action_row(ui, "Close track", None).clicked() {
                    close = true;
                }
            });
        self.style = style;
        // The name and colour apply as they are typed and chosen.
        match draft.name.trim() {
            "" => self.arrangement.names.remove(&key),
            n => self.arrangement.names.insert(key.clone(), n.to_owned()),
        };
        match color {
            Some(c) => self.arrangement.colors.insert(key.clone(), c),
            None => self.arrangement.colors.remove(&key),
        };
        self.look_draft = Some(draft);
        if let Some(a) = run {
            self.run(a, ctx);
        }
        if close {
            self.close_hit(hit);
            self.ctx_menu = None;
            open = false;
        }
        if !open {
            self.track_settings = None;
            self.look_draft = None;
        }
    }

    pub(super) fn open_read_details(&mut self, track: usize, read: AlignedRead, base: u64) {
        let Some(t) = self.tracks.get(track) else { return };
        self.selected_read = Some(read.name.clone());
        self.read_details = Some(ReadDetails { path: t.loader.bam_path.clone(), read, base });
    }

    /// The palette's "Read details": the selected read (clicked, found or gone to), as loaded.
    pub(super) fn open_selected_read_details(&mut self) {
        let Some(name) = self.selected_read.clone() else {
            self.notify("Click a read first, then open its details");
            return;
        };
        let found = self.tracks.iter().enumerate().find_map(|(i, t)| {
            let (reads, _) = t.current.as_ref()?.view.reads.as_ref()?;
            reads.iter().find(|r| r.name == name).map(|r| (i, r.clone()))
        });
        match found {
            Some((i, r)) => {
                let base = match self.highlight {
                    Some((c, p)) if c == self.contig && (r.start..r.end).contains(&p) => p,
                    _ => r.start,
                };
                self.open_read_details(i, r, base);
            }
            None => self.notify(format!("{name} isn't loaded here")),
        }
    }

    pub(super) fn read_details_window(&mut self, ctx: &egui::Context) {
        if !self.is_active {
            return;
        }
        let Some(d) = self.read_details.clone() else { return };
        let Some(i) = self.tracks.iter().position(|t| t.loader.bam_path == d.path) else {
            self.read_details = None;
            return;
        };
        let read = &d.read;
        let lines = self.read_lines(i, read, d.base);
        let contigs: Vec<String> = self.tracks[i].contigs().iter().map(|c| c.name.clone()).collect();
        let mut open = true;
        let mut copy = None;
        let mut go: Option<(String, u64)> = None;
        let mut beside: Option<(String, u64)> = None;
        let spot = self.window_pos(ctx, 260.0);
        super::tool_window::ToolWindow::new("Read details", egui::Id::unique(("read-details", self.reference_slot)))
            .placed(spot, self.pane_settled())
            .default_width(420.0)
            .show(ctx, &mut open, |ui| {
                for l in &lines {
                    ui.add(egui::Label::new(egui::RichText::new(l).monospace()).wrap());
                }
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Copy name").clicked() {
                        ui.ctx().copy_text(read.name.clone());
                    }
                    if ui.button("Copy sequence").clicked() {
                        copy = Some(true);
                    }
                    if ui.button("Copy as SAM").clicked() {
                        copy = Some(false);
                    }
                });
                if let Some(m) = read.mate
                    && let Some(c) = contigs.get(m.contig_id.max(0) as usize)
                {
                    let at = (c.clone(), m.pos.max(0) as u64);
                    ui.horizontal(|ui| {
                        if ui.button(format!("Go to mate  {c}:{}", commas(at.1 + 1))).clicked() {
                            go = Some(at.clone());
                        }
                        if ui.button("View mate in split screen").on_hover_text("Open a pane beside this one at the mate, the read selected in both").clicked() {
                            beside = Some(at.clone());
                        }
                    });
                }
                for sa in &read.supplementary {
                    let label = format!("Go to split part  {}:{} ({}, MAPQ {})", sa.contig, commas(sa.pos + 1), if sa.reverse { "−" } else { "+" }, sa.mapq);
                    ui.horizontal(|ui| {
                        if ui.button(label).clicked() {
                            go = Some((sa.contig.clone(), sa.pos));
                        }
                        if ui.button("In split screen").on_hover_text("Open a pane beside this one at this part, the read selected in both").clicked() {
                            beside = Some((sa.contig.clone(), sa.pos));
                        }
                    });
                }
            });
        if let Some(seq) = copy {
            self.copy_read(ctx, i, read, seq);
        }
        if let Some((c, pos)) = beside {
            self.selected_read = Some(read.name.clone());
            self.view_in_split(&c, pos, Some(read.name.clone()));
        }
        if let Some((c, pos)) = go {
            // Through history: Alt+Left comes back.
            self.selected_read = Some(read.name.clone());
            self.jump_to(&c, pos);
        }
        if !open {
            self.read_details = None;
        }
    }
}

/// The track whose header gear `p` is on.
pub(super) fn gear_at(layout: &Layout, p: Pos2) -> Option<Key> {
    layout.order.iter().find(|(_, r)| Layout::gear_button(*r).contains(p)).map(|(k, _)| k.clone())
}

