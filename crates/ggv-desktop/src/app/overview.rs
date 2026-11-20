//! The chromosome overview strip above the ruler: the whole contig as a bar (with its bands
//! and centromere when a cytoband table is at hand), the view as an outlined window on it.
//! Clicking the bar centres the view there; dragging the window pans. Both go into Back and
//! Forward like any other jump, and the locus box follows the view as it always does. One per
//! pane: each pane's canvas starts with its own.
//!
//! Bands come from a cytoband table the person opened (Open, or dropped on the window), else
//! one kept beside the reference (`cytoBand.txt`, `hg38.cytoBand.txt.gz`), else the built-in
//! genome's in the genome cache. A built-in genome downloads its table with it; one downloaded
//! before that (or data recognised as a built-in build) fetches it once, in the background,
//! the first time the strip is drawn. With none, the bar is plain.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Stroke, Vec2};
use ggv_io::cytoband::{Band, Cytobands, Stain};

use super::App;

/// The strip's height.
pub(super) const OVERVIEW_H: f32 = 24.0;
/// The view's window on the bar is never narrower than this (at base-pair zoom it would be a
/// fraction of a pixel).
const MIN_WINDOW_W: f32 = 6.0;
const BAR_H: f32 = 10.0;

/// Cytoband tables read so far (or that failed to read), shared by the panes.
static PARSED: Mutex<Vec<(PathBuf, Option<Arc<Cytobands>>)>> = Mutex::new(Vec::new());
/// A cytoband table the person opened: used before one found beside the reference.
static OPENED: Mutex<Option<PathBuf>> = Mutex::new(None);
/// Built-in genomes whose bands were fetched (or tried) in the background, once each.
static FETCHING: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
/// Bumped when a background fetch lands, so every pane looks for bands again.
static FETCHED: AtomicU64 = AtomicU64::new(0);

fn parsed(path: &PathBuf) -> Result<Arc<Cytobands>, String> {
    let mut cache = PARSED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, bands)) = cache.iter().find(|(p, _)| p == path) {
        return bands.clone().ok_or_else(|| "couldn't be read".to_owned());
    }
    let result = Cytobands::read(path).map(Arc::new);
    cache.push((path.clone(), result.as_ref().ok().cloned()));
    result.map_err(|e| e.to_string())
}

fn opened() -> Option<PathBuf> {
    OPENED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

#[derive(Default)]
pub(super) struct Overview {
    /// What the bands were last looked up for (the reference and an opened table), and the
    /// table found.
    looked_up: Option<(Option<PathBuf>, Option<PathBuf>, u64)>,
    bands: Option<Arc<Cytobands>>,
    drag: Option<Drag>,
    /// Esc ended a drag: the rest of it (until the button is let go) does nothing.
    cancelled: bool,
}

struct Drag {
    /// Bases from the view's centre to where the window was taken hold of.
    grab: f64,
    /// Where the view was (contig, centre, points per base), for Back and for Esc.
    from: (usize, f64, f64),
}

impl App {
    /// The strip at the top of a pane's canvas, and what is left below it for the ruler and
    /// tracks.
    pub(super) fn split_overview(full: Rect) -> (Rect, Rect) {
        let y = (full.top() + OVERVIEW_H).min(full.bottom());
        (Rect::from_min_max(full.min, Pos2::new(full.right(), y)), Rect::from_min_max(Pos2::new(full.left(), y), full.max))
    }

    /// Opens a cytoband table for the overview strip (every pane uses it).
    pub(super) fn open_cytobands(&mut self, path: PathBuf) {
        match parsed(&path) {
            Ok(_) => *OPENED.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(path),
            Err(e) => self.error = Some(format!("{}: not a cytoband table ({e})", crate::files::display_name(&path))),
        }
    }

    /// The bands for the overview, looked up again when the reference (or the opened table)
    /// changes: the opened table, else one beside the reference, else one in the built-in
    /// genome's folder.
    fn overview_bands(&mut self) -> Option<Arc<Cytobands>> {
        let key = (self.reference_path.clone(), opened(), FETCHED.load(Ordering::Relaxed));
        if self.overview.looked_up.as_ref() != Some(&key) {
            let genome_dir = || self.build().and_then(ggv_io::genomes::find).and_then(ggv_io::genomes::local).map(|l| l.reference);
            let found = key.1.clone().or_else(|| key.0.as_deref().and_then(ggv_io::cytoband::beside)).or_else(|| genome_dir().as_deref().and_then(ggv_io::cytoband::beside));
            self.overview.bands = found.and_then(|p| parsed(&p).ok());
            self.overview.looked_up = Some(key);
            if self.overview.bands.is_none() {
                self.fetch_bands_once();
            }
        }
        self.overview.bands.clone()
    }

    /// The open build's bands, when it is a built-in genome that has them and they aren't in
    /// the genome cache yet (a genome downloaded before GGV fetched them, or data recognised as
    /// that build): fetched in the background, once per run.
    fn fetch_bands_once(&self) {
        if cfg!(test) {
            return; // never the network from tests
        }
        let Some(g) = self.build().and_then(ggv_io::genomes::find).filter(|g| g.banded) else { return };
        if ggv_io::genomes::local(g).is_none_or(|l| l.cytobands.is_file()) {
            return;
        }
        {
            let mut tried = FETCHING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if tried.contains(&g.id) {
                return;
            }
            tried.push(g.id);
        }
        let ctx = self.egui_ctx.clone();
        std::thread::spawn(move || match ggv_io::genomes::fetch_cytobands(g, &mut |_, _, _| {}) {
            Ok(Some(_)) => {
                FETCHED.fetch_add(1, Ordering::Relaxed);
                ctx.request_repaint();
            }
            Ok(None) => {}
            Err(e) => eprintln!("ggv: no chromosome bands for {}: {e}", g.id),
        });
    }

    /// Where the bar is drawn: after the contig's name, to the strip's right edge.
    pub(super) fn overview_bar(&self, painter: &egui::Painter, strip: Rect) -> Rect {
        let name = self.contigs.get(self.contig).map_or("", |c| c.name.as_str());
        let label_w = painter.layout_no_wrap(name.to_owned(), FontId::proportional(11.0), Color32::WHITE).size().x;
        let left = (strip.left() + 6.0 + label_w + 8.0).min(strip.right() - 40.0);
        Rect::from_min_max(Pos2::new(left, strip.center().y - BAR_H / 2.0), Pos2::new((strip.right() - 8.0).max(left + 1.0), strip.center().y + BAR_H / 2.0))
    }

    /// The view's window on the bar, at least [`MIN_WINDOW_W`] wide and inside the bar.
    pub(super) fn overview_window(&self, bar: Rect) -> Rect {
        let (vs, ve) = self.visible();
        let len = self.contig_len().max(1.0);
        let x = |b: f64| bar.left() + (b / len).clamp(0.0, 1.0) as f32 * bar.width();
        let (mut x0, mut x1) = (x(vs), x(ve));
        if x1 - x0 < MIN_WINDOW_W {
            let mid = (x0 + x1) / 2.0;
            let half = MIN_WINDOW_W.min(bar.width()) / 2.0;
            let mid = mid.clamp(bar.left() + half, bar.right() - half);
            (x0, x1) = (mid - half, mid + half);
        }
        Rect::from_min_max(Pos2::new(x0, bar.top() - 3.0), Pos2::new(x1, bar.bottom() + 3.0))
    }

    fn overview_base_at(&self, bar: Rect, x: f32) -> f64 {
        f64::from(((x - bar.left()) / bar.width()).clamp(0.0, 1.0)) * self.contig_len()
    }

    /// Clicks and drags on the strip. A press on the window takes hold of it; a press elsewhere
    /// on the strip jumps there, and dragging on from it pans as well.
    pub(super) fn overview_input(&mut self, ui: &egui::Ui, response: &egui::Response, strip: Rect) {
        let bar = self.overview_bar(ui.painter(), strip);
        if self.overview.cancelled {
            if response.drag_stopped() || !response.dragged() {
                self.overview.cancelled = false;
            }
            return;
        }
        let window = self.overview_window(bar);
        let on_window = |p: Pos2| window.expand2(Vec2::new(3.0, 0.0)).x_range().contains(p.x);
        if response.drag_started()
            && let Some(p) = ui.input(|i| i.pointer.press_origin()).or(response.interact_pointer_pos())
        {
            let grab = if on_window(p) { self.overview_base_at(bar, p.x) - self.center } else { 0.0 };
            self.overview.drag = Some(Drag { grab, from: (self.contig, self.center, self.ppb) });
        }
        if let Some(d) = &self.overview.drag {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            let grab = d.grab;
            if let Some(p) = response.interact_pointer_pos() {
                self.center = self.overview_base_at(bar, p.x) - grab;
                self.clamp_view();
            }
            if response.drag_stopped() || !response.dragged() {
                let from = self.overview.drag.take().map(|d| d.from);
                if let Some(from) = from.filter(|f| (f.1 - self.center).abs() >= 1.0) {
                    self.remember(from);
                }
            }
            return;
        }
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            self.remember_place();
            self.center = self.overview_base_at(bar, p.x);
            self.clamp_view();
        }
        if let Some(p) = response.hover_pos() {
            ui.ctx().set_cursor_icon(if on_window(p) { egui::CursorIcon::Grab } else { egui::CursorIcon::PointingHand });
        }
    }

    pub(super) fn overview_dragging(&self) -> bool {
        self.overview.drag.is_some()
    }

    /// Esc during a drag on the strip: the view goes back to where the drag started. True if
    /// there was one.
    pub(super) fn cancel_overview_drag(&mut self) -> bool {
        let Some(d) = self.overview.drag.take() else { return false };
        (self.contig, self.center, self.ppb) = d.from;
        self.clamp_view();
        self.overview.cancelled = true;
        true
    }

    pub(super) fn draw_overview(&mut self, ui: &egui::Ui, response: &egui::Response, strip: Rect) {
        let bands = self.overview_bands();
        let painter = ui.painter_at(strip);
        let pal = self.pal();
        let theme = if self.style.dark { &crate::theme::DARK } else { &crate::theme::LIGHT };
        let Some(contig) = self.contigs.get(self.contig) else { return };
        let bar = self.overview_bar(&painter, strip);
        painter.text(Pos2::new(strip.left() + 6.0, strip.center().y), Align2::LEFT_CENTER, &contig.name, FontId::proportional(11.0), pal.muted);

        let len = self.contig_len().max(1.0);
        let x = |b: u64| bar.left() + (b as f64 / len).clamp(0.0, 1.0) as f32 * bar.width();
        let here = bands.as_ref().and_then(|c| c.bands(&contig.name)).filter(|b| !b.is_empty());
        let outline = Stroke::new(1.0, pal.rule_strong);
        match here {
            Some(bands) => {
                let mut arms: Vec<(f32, f32)> = Vec::new();
                let mut arm_start = bar.left();
                let mut centromere = 0;
                for b in bands {
                    let (x0, x1) = (x(b.start), x(b.end).max(x(b.start) + 0.5));
                    match b.stain {
                        Stain::Centromere => {
                            // The p side narrows to the centromere, the q side widens from it.
                            let q = b.name.starts_with('q') || (centromere > 0 && !b.name.starts_with('p'));
                            centromere += 1;
                            let (t, m, bt) = (bar.top(), bar.center().y, bar.bottom());
                            let points = if q { vec![Pos2::new(x0, m), Pos2::new(x1, t), Pos2::new(x1, bt)] } else { vec![Pos2::new(x0, t), Pos2::new(x1, m), Pos2::new(x0, bt)] };
                            painter.add(egui::Shape::convex_polygon(points, centromere_rgb(self.style.dark), Stroke::NONE));
                            if !q && x0 > arm_start {
                                arms.push((arm_start, x0));
                            }
                            arm_start = x1;
                        }
                        stain => {
                            let shrink = if stain == Stain::Stalk { BAR_H / 4.0 } else { 0.0 };
                            let r = Rect::from_min_max(Pos2::new(x0, bar.top() + shrink), Pos2::new(x1, bar.bottom() - shrink));
                            painter.rect_filled(r, 0.0, band_rgb(stain, self.style.dark));
                        }
                    }
                }
                arms.push((arm_start, bar.right()));
                for (a, b) in arms.into_iter().filter(|(a, b)| b > a) {
                    painter.rect_stroke(Rect::from_min_max(Pos2::new(a, bar.top()), Pos2::new(b, bar.bottom())), 2.0, outline, egui::StrokeKind::Inside);
                }
            }
            None => {
                painter.rect_filled(bar, 2.0, pal.rule);
                painter.rect_stroke(bar, 2.0, outline, egui::StrokeKind::Inside);
            }
        }

        let window = self.overview_window(bar);
        let hot = self.overview.drag.is_some() || response.hover_pos().is_some_and(|p| window.expand2(Vec2::new(3.0, 0.0)).x_range().contains(p.x));
        painter.rect_filled(window, 2.0, theme.accent.gamma_multiply(if hot { 0.3 } else { 0.18 }));
        painter.rect_stroke(window, 2.0, Stroke::new(if hot { 2.0 } else { 1.5 }, theme.accent), egui::StrokeKind::Middle);

        if let Some(p) = response.hover_pos()
            && self.overview.drag.is_none()
        {
            let base = self.overview_base_at(bar, p.x).floor() as u64;
            let band = here.and_then(|b| band_at(b, base)).filter(|b| !b.name.is_empty());
            let place = format!("{}:{}", contig.name, crate::util::commas(base + 1));
            let mut lines = vec![match band {
                Some(b) => format!("{place}  ({}{})", contig.name.strip_prefix("chr").unwrap_or(&contig.name), b.name),
                None => place,
            }];
            lines.push("Click to go there · drag the box to pan".to_owned());
            super::show_tooltip(response, lines);
        }
    }
}

/// The band containing `base`.
fn band_at(bands: &[Band], base: u64) -> Option<&Band> {
    let i = bands.partition_point(|b| b.end <= base);
    bands.get(i).filter(|b| b.start <= base)
}

/// Giemsa-stain shading: light for unstained bands, darker the more stained (lighter in the
/// dark theme, where the bar is dark).
fn band_rgb(stain: Stain, dark: bool) -> Color32 {
    let stained = |n: u8| f32::from(n.min(100)) / 100.0;
    let level = match stain {
        Stain::Neg | Stain::Other => 0.0,
        Stain::Pos(n) => stained(n),
        Stain::Stalk => 0.45,
        Stain::Variable => return if dark { Color32::from_rgb(70, 82, 110) } else { Color32::from_rgb(196, 206, 226) },
        Stain::Centromere => return centromere_rgb(dark),
    };
    if dark { Color32::from_gray((52.0 + level * 160.0) as u8) } else { Color32::from_gray((248.0 - level * 200.0) as u8) }
}

fn centromere_rgb(dark: bool) -> Color32 {
    if dark { Color32::from_rgb(210, 90, 80) } else { Color32::from_rgb(196, 64, 52) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_under_a_base() {
        let band = |start, end, name: &str| Band { start, end, name: name.into(), stain: Stain::Neg };
        let bands = [band(0, 100, "p2"), band(100, 250, "p1"), band(300, 400, "q1")];
        assert_eq!(band_at(&bands, 0).map(|b| b.name.as_str()), Some("p2"));
        assert_eq!(band_at(&bands, 100).map(|b| b.name.as_str()), Some("p1"));
        assert_eq!(band_at(&bands, 260), None, "a gap");
        assert_eq!(band_at(&bands, 400), None);
    }

    #[test]
    fn more_stain_is_darker() {
        let light = |s| band_rgb(s, false).r();
        assert!(light(Stain::Neg) > light(Stain::Pos(25)) && light(Stain::Pos(25)) > light(Stain::Pos(100)));
        let dark = |s| band_rgb(s, true).r();
        assert!(dark(Stain::Neg) < dark(Stain::Pos(100)), "inverted in the dark theme");
    }

    #[test]
    fn the_strip_sits_above_the_tracks() {
        let full = Rect::from_min_size(Pos2::new(10.0, 50.0), Vec2::new(800.0, 600.0));
        let (strip, rest) = App::split_overview(full);
        assert_eq!((strip.top(), strip.height(), rest.top(), rest.bottom()), (50.0, OVERVIEW_H, 50.0 + OVERVIEW_H, 650.0));
        assert_eq!((strip.width(), rest.width()), (800.0, 800.0));
    }
}
