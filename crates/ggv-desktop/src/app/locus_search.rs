//! Gene-name suggestions under the locus box: a case-insensitive prefix index of the gene
//! names in the open annotations (rebuilt when one opens, finishes loading or closes), and up
//! to eight suggestions with each gene's locus. Arrows move, Enter or a click goes to the
//! gene with 10% of its length on either side. Text that isn't a gene name (a locus, a contig)
//! goes to the coordinate parser as before: Enter takes a suggestion only once one is chosen
//! with the arrows, or when the text is exactly that gene's name and not a locus.

use eframe::egui::{self, Align2, FontId, Key, Modifiers, Pos2, Rect, Sense, Vec2};
use ggv_model::Region;

use super::App;

/// At most this many suggestions.
const SHOWN: usize = 8;
const ROW_H: f32 = 26.0;

#[derive(Default)]
pub(super) struct LocusSearch {
    /// The gene tracks (id, loaded) the index was built from.
    built_for: Vec<(u64, bool)>,
    index: GeneIndex,
    /// The text the suggestions are for, and them.
    query: String,
    items: Vec<Suggestion>,
    selected: Option<usize>,
    /// The locus box, as of the last frame (its keys are taken before it sees them).
    edit_id: Option<egui::Id>,
    /// The suggestions were under the pointer last frame: a click on one takes the box's focus
    /// before it counts as a click, and the list must still be there for it.
    hovered: bool,
}

struct Suggestion {
    name: String,
    /// The gene's own extent, shown beside its name.
    extent: Region,
}

/// Gene names, lowercased with the name as written, sorted, one per (lowercased) name.
#[derive(Default)]
pub(super) struct GeneIndex(Vec<(Box<str>, Box<str>)>);

impl GeneIndex {
    pub(super) fn build<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut v: Vec<(Box<str>, Box<str>)> = names.into_iter().filter(|n| !n.is_empty()).map(|n| (n.to_lowercase().into(), n.into())).collect();
        v.sort_unstable();
        v.dedup_by(|a, b| a.0 == b.0);
        Self(v)
    }

    /// Names starting with `query` (any case): an exact match first, then shorter names, then
    /// alphabetically.
    pub(super) fn prefix(&self, query: &str, n: usize) -> Vec<&str> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let from = self.0.partition_point(|(k, _)| **k < *q);
        let mut hits: Vec<&(Box<str>, Box<str>)> = self.0[from..].iter().take_while(|(k, _)| k.starts_with(&q)).collect();
        hits.sort_by(|a, b| (*a.0 != *q, a.0.len(), &a.0).cmp(&(*b.0 != *q, b.0.len(), &b.0)));
        hits.into_iter().take(n).map(|(_, name)| &**name).collect()
    }
}

/// Text that could be the start of a gene name: one word, no `:` (a locus), not a bare number.
fn gene_like(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty() && !t.contains(':') && !t.contains(char::is_whitespace) && !t.chars().all(|c| c.is_ascii_digit() || c == ',')
}

impl App {
    /// Rebuilds the index when the set of loaded annotations changed.
    fn refresh_gene_index(&mut self) {
        let now: Vec<(u64, bool)> = self.genes.iter().map(|g| (g.id, g.annotation().is_some())).collect();
        if now == self.locus_search.built_for {
            return;
        }
        let names = self.genes.iter().filter_map(|g| g.annotation()).flat_map(|a| a.genes().iter().map(|s| s.name.as_str()));
        self.locus_search.index = GeneIndex::build(names);
        self.locus_search.built_for = now;
        // Suggestions from the old index may name genes that are gone.
        self.locus_search.query.clear();
        self.locus_search.items.clear();
        self.locus_search.selected = None;
    }

    /// Before the locus box: the arrows move through the suggestions (rather than the box's
    /// cursor) while it has focus.
    pub(super) fn locus_keys(&mut self, ui: &egui::Ui) {
        let s = &mut self.locus_search;
        let focused = s.edit_id.is_some_and(|id| ui.memory(|m| m.has_focus(id)));
        if !focused || s.items.is_empty() {
            return;
        }
        let (down, up) = ui.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::ArrowDown), i.consume_key(Modifiers::NONE, Key::ArrowUp)));
        let last = s.items.len() - 1;
        if down {
            s.selected = Some(s.selected.map_or(0, |i| (i + 1).min(last)));
        }
        if up {
            s.selected = s.selected.and_then(|i| i.checked_sub(1));
        }
    }

    /// Where Enter in the locus box goes when a suggestion is chosen (or the text is exactly a
    /// gene's name and no locus): that gene, padded.
    pub(super) fn locus_choice(&self, text: &str) -> Option<Region> {
        let s = &self.locus_search;
        let chosen = s.selected.and_then(|i| s.items.get(i)).or_else(|| {
            let exact = s.items.first().filter(|first| first.name.eq_ignore_ascii_case(text.trim()));
            exact.filter(|_| ggv_io::resolve_locus_str(&self.contigs, text).is_err())
        });
        chosen.map(|g| self.pad_gene(g.extent.clone()))
    }

    /// After the locus box: brings the suggestions up to date with its text and shows them
    /// below it. Returns the gene clicked, padded, to go to.
    pub(super) fn locus_suggestions(&mut self, ui: &egui::Ui, edit: &egui::Response) -> Option<Region> {
        self.locus_search.edit_id = Some(edit.id);
        if !edit.has_focus() && !self.locus_search.hovered {
            self.locus_search.items.clear();
            self.locus_search.query.clear();
            self.locus_search.selected = None;
            return None;
        }
        if edit.has_focus() && self.locus_search.query != self.locus_text {
            self.refresh_gene_index();
            let names: Vec<String> = if gene_like(&self.locus_text) { self.locus_search.index.prefix(&self.locus_text, SHOWN).into_iter().map(str::to_owned).collect() } else { Vec::new() };
            // Each with where it is (as a jump to it would find it); a name only some build
            // has is left out.
            let items = names.into_iter().filter_map(|name| Some(Suggestion { extent: self.gene_spans(&name).into_iter().next()?.1, name })).collect();
            self.locus_search.items = items;
            self.locus_search.query = self.locus_text.clone();
            self.locus_search.selected = None;
        }
        if self.locus_search.items.is_empty() {
            self.locus_search.hovered = false;
            return None;
        }

        let theme = crate::theme::palette(ui);
        let width = edit.rect.width().clamp(260.0, 460.0);
        let mut picked = None;
        let s = &self.locus_search;
        let area = egui::Area::new(egui::Id::unique("locus-suggestions"))
            .order(egui::Order::Foreground)
            .fixed_pos(edit.rect.left_bottom() + Vec2::new(0.0, 4.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).inner_margin(4).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (i, g) in s.items.iter().enumerate() {
                        let (row, response) = ui.allocate_exact_size(Vec2::new(width, ROW_H), Sense::click());
                        let fill = if s.selected == Some(i) {
                            Some(theme.accent_soft)
                        } else {
                            response.hovered().then_some(theme.hover)
                        };
                        if let Some(fill) = fill {
                            ui.painter().rect_filled(row, crate::theme::RADIUS, fill);
                        }
                        let inner = row.shrink2(Vec2::new(8.0, 0.0));
                        let locus = format!("{}:{}-{}", g.extent.contig, crate::util::commas(g.extent.start + 1), crate::util::commas(g.extent.end));
                        let locus = ui.painter().layout_no_wrap(locus, FontId::monospace(12.0), theme.text_muted);
                        let locus_x = inner.right() - locus.size().x;
                        let name_clip = Rect::from_min_max(inner.min, Pos2::new(locus_x - 12.0, inner.bottom()));
                        ui.painter().with_clip_rect(name_clip).text(
                            Pos2::new(inner.left(), inner.center().y),
                            Align2::LEFT_CENTER,
                            &g.name,
                            egui::TextStyle::Body.resolve(ui.style()),
                            theme.text,
                        );
                        ui.painter().galley(Pos2::new(locus_x, inner.center().y - locus.size().y / 2.0), locus, theme.text_muted);
                        if response.clicked() {
                            picked = Some(i);
                        }
                        response.on_hover_cursor(egui::CursorIcon::PointingHand);
                    }
                });
            });
        let over = ui.ctx().pointer_hover_pos().is_some_and(|p| area.response.rect.contains(p));
        self.locus_search.hovered = over;
        let region = picked.and_then(|i| self.locus_search.items.get(i)).map(|g| self.pad_gene(g.extent.clone()));
        if region.is_some() {
            self.locus_search.items.clear();
            self.locus_search.hovered = false;
            self.locus_search.selected = None;
        }
        region
    }

    /// After a jump from the box: no suggestions until the text changes again.
    pub(super) fn close_locus_suggestions(&mut self) {
        let s = &mut self.locus_search;
        s.items.clear();
        s.selected = None;
        s.hovered = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_matches_ignore_case_and_rank_exact_then_short() {
        let index = GeneIndex::build(["BRCA2", "BRCA1", "brca1", "BRCC3", "BRCA1P1", "TP53", "TP53BP1", ""]);
        assert_eq!(index.prefix("brca", 8), ["BRCA1", "BRCA2", "BRCA1P1"], "one per name, shortest first");
        assert_eq!(index.prefix("BRCA1", 8), ["BRCA1", "BRCA1P1"]);
        assert_eq!(index.prefix("tp53", 8), ["TP53", "TP53BP1"], "the exact name first");
        assert_eq!(index.prefix("brc", 2).len(), 2);
        assert!(index.prefix("zz", 8).is_empty());
        assert!(index.prefix("  ", 8).is_empty());
    }

    #[test]
    fn loci_and_numbers_are_not_gene_names() {
        assert!(gene_like("BRCA1") && gene_like("tp5") && gene_like("chr20"));
        assert!(!gene_like("chr20:1,000-2,000"));
        assert!(!gene_like("31,180,000"));
        assert!(!gene_like("BRCA1 BRCA2"));
        assert!(!gene_like(""));
    }
}
