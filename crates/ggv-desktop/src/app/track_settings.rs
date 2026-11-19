//! A track's settings panel, opened from the gear in its header or "Track settings…" in the
//! right-click menu: display switches that don't belong at a position (squish, splice
//! junctions), copying the view, and managing the track (filter, move, collapse, close).
//!
//! Also the read details window ("Read details" in the right-click menu, or the palette for the
//! selected read): the read's summary and what to do with it (copy, go to its mate or parts).

use std::path::PathBuf;

use eframe::egui::{self, Pos2, Rect, Stroke, Vec2};

use super::{AlignedRead, App, Hit, Layout, actions::Action, arrange::Key};
use crate::{theme, util::commas};

/// The read whose details are open: the track's file (track indices change as tracks move),
/// the read, and the base it was opened at.
#[derive(Clone, Debug)]
pub(super) struct ReadDetails {
    pub path: PathBuf,
    pub read: AlignedRead,
    pub base: u64,
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
                    && ui.button(format!("Go to mate  {c}:{}", commas(m.pos.max(0) as u64 + 1))).clicked()
                {
                    go = Some((c.clone(), m.pos.max(0) as u64));
                }
                for sa in &read.supplementary {
                    let label = format!("Go to split part  {}:{} ({}, MAPQ {})", sa.contig, commas(sa.pos + 1), if sa.reverse { "−" } else { "+" }, sa.mapq);
                    if ui.button(label).clicked() {
                        go = Some((sa.contig.clone(), sa.pos));
                    }
                }
            });
        if let Some(seq) = copy {
            self.copy_read(ctx, i, read, seq);
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

