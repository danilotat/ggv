//! A variant's details, as IGV shows them on click: the site, ClinVar's summary when the record
//! is ClinVar's (significance, review stars, condition, gene, molecular consequence), what it
//! does to the genes open here, the genotypes, and every INFO field with the header's
//! description on its key's hover. Values are shown exactly as the file writes them (the record
//! is read again as text). The window opens beside the variant clicked and stays until closed;
//! clicking another variant replaces it.

use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
};

use eframe::egui;
use ggv_io::{InfoField, RawRecord, Variant};

use super::App;
use crate::variants::Significance;

pub struct VariantPopup {
    pub file: String,
    /// The file's full path (on the file name's hover).
    pub path: PathBuf,
    pub contig: String,
    pub var: Variant,
    pub fields: Arc<Vec<InfoField>>,
    /// The header's FORMAT fields, for the genotype columns' hovers.
    pub format_fields: Arc<Vec<InfoField>>,
    pub samples: Vec<String>,
    /// The record as the file writes it, read on the track's loader thread: what the window
    /// shows (values exactly as written), where the parsed `var` would reformat some.
    pub(super) raw: Raw,
    /// Where on screen the variant was clicked, when it was: the window opens beside it.
    clicked: Option<egui::Pos2>,
    /// "In the genes open here", and what it was computed from (see `popup_consequences`).
    pub(super) consequences: Option<(ConsequenceKey, Vec<ConsequenceRow>)>,
}

pub(super) enum Raw {
    Reading(mpsc::Receiver<Option<RawRecord>>),
    /// `None`: the record wasn't found again (the file changed); the parsed one is shown.
    Read(Option<RawRecord>),
}

/// One line of "In the genes open here": what the variant does to one transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ConsequenceRow {
    pub gene: String,
    pub transcript: String,
    /// `c.` and, when there is one, `p.`, two spaces apart.
    pub hgvs: String,
    pub effect: String,
}

impl ConsequenceRow {
    /// The line as "Copy as text" writes it.
    pub fn line(&self) -> String {
        format!("{} {}  {}  {}", self.gene, self.transcript, self.hgvs, self.effect)
    }
}

/// What a variant's consequences depend on besides the variant: the annotations loaded, one
/// transcript per gene or all, and the reference.
type ConsequenceKey = (usize, bool, Option<std::path::PathBuf>);

/// ClinVar's review status as gold stars (0-4), as ClinVar shows it.
pub fn review_stars(status: &str) -> u8 {
    match status {
        "practice_guideline" => 4,
        "reviewed_by_expert_panel" => 3,
        "criteria_provided,_multiple_submitters,_no_conflicts" => 2,
        s if s.starts_with("criteria_provided") => 1,
        _ => 0,
    }
}

/// ClinVar's text, readable: underscores as spaces, `|` between values.
pub fn readable(v: &str) -> String {
    v.replace('_', " ").replace('|', " | ")
}

/// The ClinVar summary of a record, as (label, value) lines; empty when it isn't ClinVar's.
pub fn clinvar_lines(var: &Variant) -> Vec<(&'static str, String)> {
    let Some(sig) = var.info_value("CLNSIG") else { return Vec::new() };
    let mut out = vec![("Significance", readable(sig))];
    if let Some(s) = var.info_value("CLNREVSTAT") {
        let n = review_stars(s);
        // As text: the default fonts have no star glyphs (they drew as empty boxes).
        out.push(("Review", format!("{n} of 4 stars  ({})", readable(s))));
    }
    if let Some(c) = var.info_value("CLNDN") {
        out.push(("Condition", readable(c)));
    }
    if let Some(g) = var.info_value("GENEINFO") {
        // `BRCA1:672|NBR2:10230`: symbols with their Gene IDs.
        let genes: Vec<&str> = g.split('|').map(|x| x.split(':').next().unwrap_or(x)).collect();
        out.push(("Gene", genes.join(", ")));
    }
    if let Some(mc) = var.info_value("MC") {
        // `SO:0001583|missense_variant,SO:0001819|synonymous_variant`.
        let terms: Vec<String> = mc.split(',').map(|x| x.split('|').nth(1).unwrap_or(x).replace('_', " ")).collect();
        out.push(("Consequence", terms.join(", ")));
    }
    if let Some(h) = var.info_value("CLNHGVS") {
        out.push(("HGVS", h.to_owned()));
    }
    if var.id != "." {
        out.push(("Variation ID", var.id.clone()));
    }
    out
}

impl App {
    /// Shows `var` from variant track `i` until closed or replaced.
    pub(super) fn open_variant_popup(&mut self, i: usize, var: Variant) {
        let Some(t) = self.variants.get(i) else { return };
        let contig = self.contigs.get(self.contig).map(|c| c.name.clone()).unwrap_or_default();
        let raw = Raw::Reading(t.read_raw(&contig, &var));
        self.variant_popup = Some(VariantPopup {
            file: t.file_name(),
            path: t.path.clone(),
            contig,
            var,
            fields: t.info_fields.clone(),
            format_fields: t.format_fields.clone(),
            samples: t.samples.clone(),
            raw,
            clicked: None,
            consequences: None,
        });
    }

    pub(super) fn variant_window_id(&self) -> egui::Id {
        // One per pane (two panes' windows don't collide).
        egui::Id::unique(("variant details", self.reference_slot))
    }

    /// Shows `var` for a click on it at `at`: the window is placed again beside it each time.
    pub(super) fn open_variant_popup_at(&mut self, i: usize, var: Variant, at: egui::Pos2) {
        self.open_variant_popup(i, var);
        let id = self.variant_window_id();
        let Some(p) = &mut self.variant_popup else { return };
        p.clicked = Some(at);
        super::inspector::place_again(&self.egui_ctx, id);
    }

    /// What the window's variant does to the genes open here: at its own contig and position,
    /// wherever the view has gone since (computed again only when genes or the reference
    /// change, not each frame).
    pub(super) fn popup_consequences(&mut self) -> Vec<ConsequenceRow> {
        let Some(p) = &self.variant_popup else { return Vec::new() };
        let key = (self.genes.iter().filter(|g| g.annotation().is_some()).count(), self.all_transcripts, self.reference_path.clone());
        if let Some((k, rows)) = &p.consequences
            && *k == key
        {
            return rows.clone();
        }
        let var = &p.var;
        let rows: Vec<ConsequenceRow> = var
            .alts
            .iter()
            .filter(|a| !a.starts_with('<') && *a != "*")
            .flat_map(|a| self.consequences(&p.contig, var.start, &var.reference, a).into_iter().take(6))
            .map(|c| ConsequenceRow {
                gene: c.gene,
                transcript: c.transcript,
                hgvs: format!("{}{}", c.hgvs_c, c.hgvs_p.map(|p| format!("  {p}")).unwrap_or_default()),
                effect: c.effect,
            })
            .collect();
        if let Some(p) = &mut self.variant_popup {
            p.consequences = Some((key, rows.clone()));
        }
        rows
    }

    pub(super) fn variant_window(&mut self, ctx: &egui::Context) {
        let consequences = self.popup_consequences();
        if let Some(p) = &mut self.variant_popup
            && let Raw::Reading(rx) = &p.raw
        {
            match rx.try_recv() {
                Ok(raw) => p.raw = Raw::Read(raw),
                Err(mpsc::TryRecvError::Disconnected) => p.raw = Raw::Read(None),
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let Some(p) = &self.variant_popup else { return };
        let var = &p.var;
        let text = self.variant_text(p, &consequences.iter().map(ConsequenceRow::line).collect::<Vec<_>>());
        let id = self.variant_window_id();
        let screen = ctx.content_rect();
        let size = ctx.memory(|m| m.area_rect(id)).map_or(egui::vec2(WIDTH, 420.0), |r| r.size());
        let spot = match p.clicked {
            Some(at) => beside_point(at, size, screen),
            // Opened some other way: at the pane's top right.
            None => {
                let pane = self.pane_rect(ctx);
                super::inspector::beside(pane, pane.left(), size.y, screen)
            }
        };
        let raw = match &p.raw {
            Raw::Read(Some(r)) => Some(r.clone()),
            Raw::Read(None) => Some(parsed_record(p)),
            Raw::Reading(_) => None,
        };
        // As wide as its widest table (up to most of the screen), so columns don't scroll.
        let style = ctx.global_style();
        let tables = ctx.fonts_mut(|f| tables_width(f, &style, p, raw.as_ref(), &consequences));
        let inner = 2.0 * f32::from(super::tool_window::MARGIN) + 12.0;
        let min_width = (tables + inner).max(super::tool_window::MIN_WIDTH).min(screen.width() * 0.6);
        let mut open = true;
        let mut copy = false;
        super::tool_window::ToolWindow::new("Variant details", id)
            .placed(spot, self.pane_settled())
            .default_width(WIDTH.max(min_width))
            .min_width(min_width)
            .show(ctx, &mut open, |ui| {
                super::inspector::visible_scroll_bars(ui);
                header(ui, p, raw.as_ref());
                let body_h = (screen.height() * 0.6).clamp(160.0, 560.0);
                egui::ScrollArea::vertical().id_salt("variant-body").max_height(body_h).auto_shrink([false, true]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = super::tool_window::GAP;
                    let clinvar = clinvar_lines(var);
                    if !clinvar.is_empty() {
                        ui.separator();
                        let sig = var.info_value("CLNSIG").map(Significance::of).unwrap_or(Significance::Other);
                        let [r, g, b] = sig.rgb();
                        ui.label(egui::RichText::new("ClinVar").strong().color(egui::Color32::from_rgb(r, g, b)));
                        egui::Grid::new("clinvar").num_columns(2).striped(true).show(ui, |ui| {
                            for (k, v) in &clinvar {
                                ui.label(egui::RichText::new(*k).weak());
                                ui.add(egui::Label::new(v).wrap());
                                ui.end_row();
                            }
                        });
                    }
                    if !consequences.is_empty() {
                        ui.separator();
                        ui.label(egui::RichText::new("In the genes open here").strong());
                        gene_grid(ui, &consequences);
                    }
                    match &raw {
                        Some(raw) => {
                            if !raw.format.is_empty() && !raw.samples.is_empty() {
                                ui.separator();
                                ui.label(egui::RichText::new("Samples").strong());
                                genotype_table(ui, p, raw);
                            }
                            // A record with no INFO (`.`) has no section, rather than a heading
                            // over nothing.
                            if raw.info_pairs().next().is_some() {
                                ui.separator();
                                ui.label(egui::RichText::new("INFO").strong());
                                info_rows(ui, p, raw);
                            }
                        }
                        None => {
                            ui.separator();
                            ui.label(egui::RichText::new("Reading the record…").weak());
                        }
                    }
                });
                ui.separator();
                if ui.button("Copy as text").clicked() {
                    copy = true;
                }
            });
        if copy {
            ctx.copy_text(text);
        }
        if !open {
            self.variant_popup = None;
        }
    }

    /// The window's contents as plain text, for copying.
    pub(super) fn variant_text(&self, p: &VariantPopup, consequences: &[String]) -> String {
        let var = &p.var;
        let mut out = vec![format!("{}:{} {} {} > {}  ({})", p.contig, var.start + 1, var.id, super::allele(&var.reference), super::alleles(&var.alts.join(",")), p.file)];
        out.extend(clinvar_lines(var).into_iter().map(|(k, v)| format!("{k}: {v}")));
        out.extend(consequences.iter().cloned());
        out.extend(var.info_pairs().map(|(k, v)| {
            let desc = p.fields.iter().find(|f| f.id == k).map(|f| f.description.as_str()).unwrap_or("");
            if desc.is_empty() { format!("{k}={v}") } else { format!("{k}={v}  ({desc})") }
        }));
        out.join("\n")
    }
}

/// The window's width when it first opens.
const WIDTH: f32 = 460.0;
/// Samples shown in the genotype table; a note counts the rest.
const SAMPLES_SHOWN: usize = 20;
/// Gap between table columns, and between the window and the variant beside it.
const COLUMN_GAP: f32 = 16.0;
/// Alleles longer than this (REF and ALT together) scroll in a box of a few lines.
const LONG_ALLELES: usize = 60;

/// Where the window goes for a click at `at`: beside it, its top near the click, on the right
/// unless there is no room there (then the left), so the variant's column stays visible.
pub(super) fn beside_point(at: egui::Pos2, size: egui::Vec2, screen: egui::Rect) -> (egui::Pos2, egui::Align2) {
    const MARGIN: f32 = 8.0;
    let y = (at.y - 24.0).min(screen.bottom() - MARGIN - size.y).max(screen.top() + MARGIN);
    let right_room = screen.right() - MARGIN - (at.x + COLUMN_GAP);
    let left_room = (at.x - COLUMN_GAP) - (screen.left() + MARGIN);
    if right_room >= size.x || right_room >= left_room {
        (egui::pos2(at.x + COLUMN_GAP, y), egui::Align2::LEFT_TOP)
    } else {
        (egui::pos2(at.x - COLUMN_GAP, y), egui::Align2::RIGHT_TOP)
    }
}

/// `text` shortened in the middle with an ellipsis to fit `width` in `font`.
fn middle_truncated(ui: &egui::Ui, text: &str, font: &egui::FontId, width: f32) -> String {
    let w = |s: &str| ui.painter().layout_no_wrap(s.to_owned(), font.clone(), egui::Color32::WHITE).size().x;
    if w(text) <= width {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let cut = |keep: usize| {
        let (a, b) = (keep.div_ceil(2), keep / 2);
        format!("{}…{}", chars[..a].iter().collect::<String>(), chars[chars.len() - b..].iter().collect::<String>())
    };
    // The most characters that fit.
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if w(&cut(mid)) <= width { lo = mid } else { hi = mid - 1 }
    }
    cut(lo)
}

/// A header's Description for `key`, when it has one.
fn description<'a>(fields: &'a [InfoField], key: &str) -> Option<&'a str> {
    fields.iter().find(|f| f.id == key).map(|f| f.description.as_str()).filter(|d| !d.is_empty())
}

/// The record as parsed, for when it can't be read again as text: what the window showed before.
fn parsed_record(p: &VariantPopup) -> RawRecord {
    let var = &p.var;
    let keys: Vec<&str> = ["GT", "DP", "GQ", "AD"]
        .into_iter()
        .filter(|k| match *k {
            "GT" => !var.samples.is_empty(),
            "DP" => var.samples.iter().any(|s| s.dp.is_some()),
            "GQ" => var.samples.iter().any(|s| s.gq.is_some()),
            _ => var.samples.iter().any(|s| !s.ad.is_empty()),
        })
        .collect();
    let opt = |v: Option<i32>| v.map_or_else(|| ".".to_owned(), |v| v.to_string());
    RawRecord {
        chrom: p.contig.clone(),
        pos: (var.start + 1).to_string(),
        qual: var.qual.map_or_else(|| ".".to_owned(), |q| format!("{q:.1}")),
        filter: if var.filters.is_empty() { ".".to_owned() } else { var.filters.join(",") },
        info: var.info.clone(),
        samples: var
            .samples
            .iter()
            .map(|s| {
                keys.iter()
                    .map(|k| match *k {
                        "GT" => s.gt.clone(),
                        "DP" => opt(s.dp),
                        "GQ" => opt(s.gq),
                        _ => s.ad_text().unwrap_or_else(|| ".".to_owned()),
                    })
                    .collect()
            })
            .collect(),
        format: keys.into_iter().map(str::to_owned).collect(),
    }
}

/// The alleles and type; the position and file; ID, QUAL and FILTER.
fn header(ui: &mut egui::Ui, p: &VariantPopup, raw: Option<&RawRecord>) {
    let var = &p.var;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        let alleles = format!("{} > {}", var.reference, var.alts.join(","));
        let title = |ui: &mut egui::Ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.add(egui::Label::new(egui::RichText::new(&alleles).size(15.0)).wrap());
                ui.label(egui::RichText::new(var.kind()).weak().size(15.0));
            });
        };
        if alleles.chars().count() > LONG_ALLELES {
            egui::ScrollArea::vertical().id_salt("variant-alleles").max_height(64.0).auto_shrink([false, true]).show(ui, title);
        } else {
            title(ui);
        }

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let pos = match raw {
                Some(r) => format!("{}:{}", r.chrom, r.pos),
                None => format!("{}:{}", p.contig, var.start + 1),
            };
            ui.label(pos);
            let font = egui::TextStyle::Body.resolve(ui.style());
            let shown = middle_truncated(ui, &p.file, &font, ui.available_width());
            ui.add(egui::Label::new(egui::RichText::new(shown).weak()).truncate()).on_hover_text(p.path.display().to_string());
        });

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let pending = "…";
            let pairs = [
                ("ID", var.id.as_str()),
                ("QUAL", raw.map_or(pending, |r| r.qual.as_str())),
                ("FILTER", raw.map_or(pending, |r| r.filter.as_str())),
            ];
            for (k, (label, value)) in pairs.into_iter().enumerate() {
                if k > 0 {
                    ui.add_space(COLUMN_GAP - 4.0);
                }
                ui.label(egui::RichText::new(label).weak());
                ui.label(value);
            }
        });
    });
}

/// How wide the gene grid and genotype table are drawn, the wider of the two.
fn tables_width(fonts: &mut egui::epaint::FontsView<'_>, style: &egui::Style, p: &VariantPopup, raw: Option<&RawRecord>, genes: &[ConsequenceRow]) -> f32 {
    let body = egui::TextStyle::Body.resolve(style);
    let mut w = |s: &str| fonts.layout_no_wrap(s.to_owned(), body.clone(), egui::Color32::WHITE).size().x;
    // Columns side by side, `COLUMN_GAP` apart: each as wide as its widest cell, and at least
    // as wide as egui's grids make a column.
    let min_col = style.spacing.interact_size.x;
    let mut columns = |cols: Vec<Vec<&str>>| -> f32 {
        let n = cols.len() as f32;
        cols.iter().map(|c| c.iter().map(|s| w(s)).fold(min_col, f32::max)).sum::<f32>() + COLUMN_GAP * (n - 1.0).max(0.0)
    };
    let gene_w = if genes.is_empty() {
        0.0
    } else {
        columns(vec![
            genes.iter().map(|r| r.gene.as_str()).collect(),
            genes.iter().map(|r| r.transcript.as_str()).collect(),
            genes.iter().map(|r| r.hgvs.as_str()).collect(),
            genes.iter().map(|r| r.effect.as_str()).collect(),
        ])
    };
    let gt_w = raw.filter(|r| !r.format.is_empty()).map_or(0.0, |raw| {
        let rows: Vec<(&String, &Vec<String>)> = p.samples.iter().zip(&raw.samples).take(SAMPLES_SHOWN).collect();
        let mut cols = vec![rows.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>()];
        for (k, key) in raw.format.iter().enumerate() {
            let mut c = vec![key.as_str()];
            c.extend(rows.iter().map(|(_, v)| v.get(k).map_or("", String::as_str)));
            cols.push(c);
        }
        columns(cols)
    });
    gene_w.max(gt_w)
}

/// Gene, transcript, HGVS and consequence in aligned columns.
fn gene_grid(ui: &mut egui::Ui, rows: &[ConsequenceRow]) {
    egui::Grid::new("variant-genes").num_columns(4).spacing([COLUMN_GAP, 4.0]).show(ui, |ui| {
        for r in rows {
            ui.label(&r.gene);
            ui.label(&r.transcript);
            ui.label(&r.hgvs);
            ui.label(&r.effect);
            ui.end_row();
        }
    });
}

/// Samples by FORMAT keys, values as written; a key's hover is its header
/// Description.
fn genotype_table(ui: &mut egui::Ui, p: &VariantPopup, raw: &RawRecord) {
    egui::ScrollArea::horizontal().id_salt("variant-genotypes").auto_shrink([false, true]).show(ui, |ui| {
        egui::Grid::new("variant-genotype-table").num_columns(raw.format.len() + 1).spacing([COLUMN_GAP, 4.0]).show(ui, |ui| {
            ui.label("");
            for key in &raw.format {
                let r = ui.label(egui::RichText::new(key).weak());
                if let Some(d) = description(&p.format_fields, key) {
                    r.on_hover_text(d);
                }
            }
            ui.end_row();
            for (name, values) in p.samples.iter().zip(&raw.samples).take(SAMPLES_SHOWN) {
                ui.label(name);
                for k in 0..raw.format.len() {
                    ui.label(values.get(k).map_or("", String::as_str));
                }
                ui.end_row();
            }
        });
    });
    if raw.samples.len() > SAMPLES_SHOWN {
        ui.label(egui::RichText::new(format!("and {} more samples", raw.samples.len() - SAMPLES_SHOWN)).weak());
    }
}

/// One line per INFO field in file order: the key (weak, its header Description on hover) in a
/// column as wide as the longest key, the value beside it, wrapping within its column.
fn info_rows(ui: &mut egui::Ui, p: &VariantPopup, raw: &RawRecord) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let key_w = raw.info_pairs().map(|(k, _)| ui.painter().layout_no_wrap(k.to_owned(), font.clone(), egui::Color32::WHITE).size().x).fold(0.0, f32::max).ceil();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        for (k, v) in raw.info_pairs() {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = COLUMN_GAP;
                let key = ui
                    .allocate_ui_with_layout(egui::vec2(key_w, 0.0), egui::Layout::left_to_right(egui::Align::Min), |ui| {
                        ui.set_min_width(key_w);
                        ui.add(egui::Label::new(egui::RichText::new(k).weak()).extend())
                    })
                    .inner;
                if let Some(d) = description(&p.fields, k) {
                    key.on_hover_text(d);
                }
                let value_w = ui.available_width();
                ui.vertical(|ui| {
                    ui.set_max_width(value_w);
                    if v.is_empty() {
                        ui.label(egui::RichText::new("(flag)").weak());
                    } else {
                        ui.add(egui::Label::new(v).wrap());
                    }
                });
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clinvar(info: &str) -> Variant {
        Variant {
            start: 43_094_463,
            end: 43_094_464,
            id: "55601".into(),
            reference: "C".into(),
            alts: vec!["T".into()],
            qual: None,
            filters: vec![],
            info: info.into(),
            af: None,
            samples: vec![],
            ..Default::default()
        }
    }

    /// Beside the click: on the right while it fits there, else on the left; never over it.
    #[test]
    fn opens_beside_the_click() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let size = egui::vec2(460.0, 400.0);
        let (at, align) = beside_point(egui::pos2(300.0, 200.0), size, screen);
        assert_eq!((at.x, align), (300.0 + COLUMN_GAP, egui::Align2::LEFT_TOP));
        let (at, align) = beside_point(egui::pos2(1200.0, 200.0), size, screen);
        assert_eq!((at.x, align), (1200.0 - COLUMN_GAP, egui::Align2::RIGHT_TOP));
        // Low on the screen: kept on it.
        let (at, _) = beside_point(egui::pos2(300.0, 880.0), size, screen);
        assert!(at.y + size.y <= screen.bottom());
    }

    #[test]
    fn file_names_shorten_in_the_middle() {
        let ctx = egui::Context::default();
        let mut out = (String::new(), String::new());
        ctx.run_ui(Default::default(), |ui| {
            let font = egui::TextStyle::Body.resolve(ui.style());
            let name = "HG002.revio.deepvariant.phased.vcf.gz";
            out = (middle_truncated(ui, name, &font, 1000.0), middle_truncated(ui, name, &font, 120.0));
        })
        .textures_delta
        .clear();
        assert_eq!(out.0, "HG002.revio.deepvariant.phased.vcf.gz");
        assert!(out.1.starts_with("HG00") && out.1.ends_with(".gz") && out.1.contains('…'), "{}", out.1);
    }

    #[test]
    fn clinvar_summary() {
        let v = clinvar(
            "ALLELEID=70069;CLNDISDB=MedGen:C2676676;CLNDN=Hereditary_breast_ovarian_cancer_syndrome|not_provided;CLNHGVS=NC_000017.11:g.43094464C>T;\
             CLNREVSTAT=reviewed_by_expert_panel;CLNSIG=Pathogenic;GENEINFO=BRCA1:672;MC=SO:0001587|nonsense",
        );
        let lines = clinvar_lines(&v);
        assert_eq!(lines[0], ("Significance", "Pathogenic".to_owned()));
        assert_eq!(lines[1], ("Review", "3 of 4 stars  (reviewed by expert panel)".to_owned()));
        assert_eq!(lines[2], ("Condition", "Hereditary breast ovarian cancer syndrome | not provided".to_owned()));
        assert_eq!(lines[3], ("Gene", "BRCA1".to_owned()));
        assert_eq!(lines[4], ("Consequence", "nonsense".to_owned()));
        assert!(clinvar_lines(&clinvar("DP=30")).is_empty(), "not a ClinVar record");
        assert_eq!(review_stars("criteria_provided,_single_submitter"), 1);
        assert_eq!(review_stars("criteria_provided,_conflicting_classifications"), 1);
        assert_eq!(review_stars("no_assertion_criteria_provided"), 0);
    }
}
