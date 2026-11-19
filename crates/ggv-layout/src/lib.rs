//! Turns alignments into drawable data, independent of how it is drawn.
//!
//! [`load_view`] reads a region in one pass and returns everything a renderer needs: the
//! reference, decoded reads packed into rows, and coverage computed from those same reads.
//! Regions too wide for reads get binned depth from a lighter pass that keeps no reads.

mod coverage;
pub mod classify;
pub mod expr;
mod pack;
pub mod phase;
mod read;

pub use coverage::{Coverage, CoverageBin, CoverageColumn, coverage_from_reads};
use coverage::ColumnAccumulator;
pub use pack::{Group, GroupBy, PackOptions, Packed, SortBy, pack, pack_indices, pack_with, extent, repack, same_template, pack_sticky, row_hints, RowHints};
pub use ggv_query::ReadPredicate;
pub use read::{AlignedRead, Block, Indel, Mate, Mismatch, ModCall, SplitAlignment, decode, decode_parts, decode_with, parse_sa, parse_tag_name, read_bases, tag_text};

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use ggv_io::{BamFile, pasta::pileup::Read};
use ggv_model::Region;
use ggv_query::{PileupOptions, ReadCounts};

#[derive(Clone, Debug)]
pub struct ViewOptions {
    pub filters: PileupOptions,
    /// Number of bins for coverage of regions too wide to load reads.
    pub bins: usize,
    /// Reads are loaded only for regions up to this many bases (IGV's "visibility window").
    /// Default 100 kb.
    pub max_read_window: u64,
    /// Binned coverage is computed only for regions up to this many bases. Default 5 Mb.
    pub max_coverage_window: u64,
    /// ... and only when the index places at most this many (compressed) bytes of alignments
    /// in the region; denser regions get depth estimated from the index, so a wide view of
    /// deep data (or of a remote file) reads a bounded amount. Default: no limit.
    pub max_coverage_bytes: f64,
    /// Rows beyond this are dropped and counted in [`Packed::hidden_reads`]. Default 100.
    pub max_rows: usize,
    /// Minimum gap between reads in one row, in bases. Default 2.
    pub min_gap: u64,
    /// A tag whose value each read keeps (`AlignedRead::tag`), to group or colour by.
    pub tag: Option<[u8; 2]>,
    /// Whether reads keep their base-modification calls (`AlignedRead::mods`). Default true.
    pub mods: bool,
    /// Spans at least twice this long are decoded on several threads. Default [`READ_CHUNK`].
    pub read_chunk: u64,
    /// Keep at most this many reads per window of start positions (IGV's downsampling). Every
    /// read is still read and counted: coverage and read counts are those of all the reads,
    /// only the reads kept to draw are fewer. Default: off (every read kept).
    pub downsample: Option<Downsample>,
    /// Set (from another thread) to stop a load early; it then returns an error of kind
    /// `Interrupted`. Checked at every record.
    pub cancel: Option<Arc<AtomicBool>>,
}

/// How reads are sampled when a region is deeper than can usefully be drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Downsample {
    /// Bases per window of read start positions. IGV: 50.
    pub window: u64,
    /// Reads kept per window. IGV: 100.
    pub count: usize,
}

impl Default for Downsample {
    fn default() -> Self {
        Self { window: 50, count: 100 }
    }
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            filters: PileupOptions::default(),
            bins: 1200,
            max_read_window: 100_000,
            max_coverage_window: 5_000_000,
            max_coverage_bytes: f64::INFINITY,
            max_rows: 100,
            min_gap: 2,
            tag: None,
            mods: true,
            read_chunk: READ_CHUNK,
            downsample: None,
            cancel: None,
        }
    }
}

/// Everything needed to draw one region of one BAM.
#[derive(Clone, Debug)]
pub struct ViewData {
    pub region: Region,
    /// Uppercase reference bases for exactly `region`, if a reference was given.
    pub reference: Option<Vec<u8>>,
    pub coverage: Coverage,
    /// `None` when the region is wider than `max_read_window`.
    pub reads: Option<(Vec<AlignedRead>, Packed)>,
    pub read_counts: ReadCounts,
    /// A label for the track, e.g. the sample name or file name.
    pub label: String,
    /// Where reads were left out by [`ViewOptions::downsample`]: merged `[start, end)` windows
    /// of read start positions, in order. Empty when every read is kept.
    pub downsampled: Vec<(u64, u64)>,
    /// Reads that passed the filters but were left out by downsampling (counted in coverage
    /// and `read_counts`, not in `reads`).
    pub reads_left_out: u64,
}

/// The fixed fields of an encoded BAM record (without `block_size`) needed for depth.
struct RawFields<'a> {
    pos: i64,
    mapq: u8,
    flags: u16,
    cigar: &'a [u8],
}

impl RawFields<'_> {
    fn cigar(&self) -> impl Iterator<Item = u32> + '_ {
        self.cigar.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// Reads the fields depth needs from an encoded record, or `None` when the record is short or
/// its CIGAR is the `kSmN` placeholder for one stored in the `CG` tag.
fn raw_fields(raw: &[u8]) -> Option<RawFields<'_>> {
    let u16_at = |o: usize| u16::from_le_bytes([raw[o], raw[o + 1]]);
    if raw.len() < 32 {
        return None;
    }
    let pos = i64::from(i32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]));
    let name_len = usize::from(raw[8]);
    let n_cigar = usize::from(u16_at(12));
    let start = 32 + name_len;
    let cigar = raw.get(start..start + 4 * n_cigar)?;
    let fields = RawFields { pos, mapq: raw[9], flags: u16_at(14), cigar };
    // kSmN: two ops, a soft clip of the whole read then a reference skip.
    if n_cigar == 2 {
        let mut ops = fields.cigar();
        let (first, second) = (ops.next()?, ops.next()?);
        let l_seq = u32::from_le_bytes([raw[16], raw[17], raw[18], raw[19]]);
        if first & 0xf == 4 && first >> 4 == l_seq && second & 0xf == 3 {
            return None;
        }
    }
    Some(fields)
}

/// Spans at least twice this long are decoded on several threads.
pub const READ_CHUNK: u64 = 250_000;

pub fn load_view(
    bam: &mut BamFile,
    region: &Region,
    reference: Option<Vec<u8>>,
    label: String,
    opts: &ViewOptions,
) -> ggv_query::Result<ViewData> {
    let mut counts = ReadCounts::default();
    let len = region.len();

    if len <= opts.max_read_window {
        let ref_slice = reference.as_deref().map(|r| (r, region.start));
        // Decoding (every base against the reference) is most of a load: wide spans are cut
        // into chunks decoded on several threads, each keeping the reads that start in it, and
        // joined in order, so the reads and counts are the same as one pass.
        let sampling = opts.downsample.filter(|d| d.window > 0 && d.count > 0);
        let cancel = opts.cancel.as_deref();
        let parts = ggv_query::chunks::run_sized(bam, region, opts.read_chunk, |b, chunk, count_from| {
            let mut sampler = Sampler::new(sampling);
            let mut counts = ReadCounts::default();
            // Sampling: coverage is counted here from every read, before any is left out.
            // A read running past the chunk is counted up to its end and the rest kept aside,
            // added once the chunks are joined (each chunk counts only its own bases).
            let mut cov = sampling.map(|_| ColumnAccumulator::new(chunk));
            let mut spill = Vec::new();
            b.for_each_record(chunk, |record| {
                check_cancel(cancel)?;
                let read = Read::new(record.as_ref().to_vec())?;
                if count_from > 0 && read.position() < count_from as i64 {
                    return Ok(()); // an earlier chunk has it
                }
                if counts.admit_in(&read, &opts.filters, &region.contig, ref_slice) && read.position() >= 0 {
                    // A read running past the region is decoded against the reference over all
                    // of it, so its mismatches (drawn, and seen by a `mismatches` filter) are whole.
                    let whole = ref_slice.and(opts.filters.reference.as_ref()).and_then(|r| r.beyond(&read, &region.contig, ref_slice));
                    let slice = whole.as_ref().map(|(s, from)| (s.as_slice(), *from)).or(ref_slice);
                    let mut decoded = decode_parts(&read, slice, opts.filters.min_base_quality, opts.mods);
                    if let Some(t) = opts.tag {
                        decoded.tag = tag_text(&read, t);
                    }
                    if ref_slice.is_none() {
                        decoded.bases = Some(read_bases(&read));
                    }
                    if let Some(cov) = &mut cov {
                        cov.add(&decoded);
                        if decoded.end > chunk.end && chunk.end < region.end {
                            spill.push(decoded.clone());
                        }
                    }
                    sampler.push(decoded);
                }
                Ok(())
            })?;
            let (reads, left_out, windows) = sampler.finish();
            Ok(Part { reads, counts, cov, spill, chunk_end: chunk.end, left_out, windows })
        })?;
        let mut reads = Vec::with_capacity(parts.iter().map(|p| p.reads.len()).sum());
        let mut downsampled: Vec<(u64, u64)> = Vec::new();
        let mut reads_left_out = 0;
        let mut whole_cov = sampling.map(|_| ColumnAccumulator::new(region));
        for p in parts {
            if let Some(acc) = &mut whole_cov {
                if let Some(c) = &p.cov {
                    acc.absorb(c);
                }
                for r in &p.spill {
                    acc.add_within(r, p.chunk_end, region.end);
                }
            }
            reads.extend(p.reads);
            counts.merge(&p.counts);
            reads_left_out += p.left_out;
            for w in p.windows {
                match downsampled.last_mut() {
                    Some(last) if last.1 >= w.0 => last.1 = last.1.max(w.1),
                    _ => downsampled.push(w),
                }
            }
        }
        let columns = match whole_cov {
            Some(acc) => acc.finish(reference.as_deref()),
            None => coverage_from_reads(&reads, region, reference.as_deref()),
        };
        let coverage = Coverage::PerBase(columns);
        let packed = pack(&reads, opts.max_rows, opts.min_gap);
        return Ok(ViewData {
            region: region.clone(),
            reference,
            coverage,
            reads: Some((reads, packed)),
            read_counts: counts,
            label,
            downsampled,
            reads_left_out,
        });
    }

    let countable = len <= opts.max_coverage_window;
    let too_dense = countable && opts.max_coverage_bytes.is_finite() && bam.index_bytes(region).is_some_and(|b| b > opts.max_coverage_bytes);
    let estimated = if !countable || too_dense {
        estimate_depth(bam, region, &opts.filters, opts.bins.clamp(1, 64))?.map(|(window, depth)| Coverage::Estimated(resample(&depth, window, region, opts.bins)))
    } else {
        None
    };
    let coverage = match estimated {
        Some(c) => c,
        None if countable => {
            let (depth, c) = binned_depth(bam, region, &opts.filters, opts.cancel.as_deref())?;
            counts = c;
            Coverage::Binned(depth.into_bins(opts.bins))
        }
        None => Coverage::Unavailable,
    };

    Ok(ViewData { region: region.clone(), reference, coverage, reads: None, read_counts: counts, label, downsampled: Vec::new(), reads_left_out: 0 })
}

/// One chunk of a read-level load.
struct Part {
    reads: Vec<AlignedRead>,
    counts: ReadCounts,
    /// Coverage of every admitted read over the chunk (when sampling).
    cov: Option<ColumnAccumulator>,
    /// Reads running past the chunk's end, for the coverage beyond it.
    spill: Vec<AlignedRead>,
    chunk_end: u64,
    left_out: u64,
    windows: Vec<(u64, u64)>,
}

/// The error a cancelled load stops with.
fn check_cancel(cancel: Option<&AtomicBool>) -> std::io::Result<()> {
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
        return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "load cancelled"));
    }
    Ok(())
}

/// Keeps at most `count` reads per window of start positions, as records stream past in
/// coordinate order, so memory stays bounded however deep the region.
///
/// The reads kept in a window are the `count` with the smallest hash of their name: the same
/// reads on every load of the same place (no flicker on reload), and mates (one name) tend to
/// be kept or left out together, as IGV keeps pairs.
struct Sampler {
    rule: Option<Downsample>,
    window: Option<u64>,
    heap: std::collections::BinaryHeap<Sampled>,
    /// Whether the current window has left a read out.
    full: bool,
    seq: u64,
    out: Vec<AlignedRead>,
    left_out: u64,
    windows: Vec<(u64, u64)>,
}

struct Sampled {
    key: (u64, u64),
    read: AlignedRead,
}

impl PartialEq for Sampled {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
impl Eq for Sampled {}
impl PartialOrd for Sampled {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Sampled {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key.cmp(&other.key)
    }
}

/// A well-mixed 64-bit hash of a read name (FNV-1a, then a SplitMix64 finish).
fn name_hash(name: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^ (h >> 31)
}

impl Sampler {
    fn new(rule: Option<Downsample>) -> Self {
        Self { rule, window: None, heap: Default::default(), full: false, seq: 0, out: Vec::new(), left_out: 0, windows: Vec::new() }
    }

    fn push(&mut self, read: AlignedRead) {
        let Some(rule) = self.rule else {
            self.out.push(read);
            return;
        };
        let w = read.start / rule.window;
        if self.window != Some(w) {
            self.flush();
            self.window = Some(w);
        }
        self.seq += 1;
        self.heap.push(Sampled { key: (name_hash(&read.name), self.seq), read });
        if self.heap.len() > rule.count {
            self.heap.pop();
            self.left_out += 1;
            self.full = true;
        }
    }

    /// Moves the current window's reads to the output, in the order they came (by position).
    fn flush(&mut self) {
        let (Some(rule), Some(w)) = (self.rule, self.window) else { return };
        let mut kept: Vec<Sampled> = std::mem::take(&mut self.heap).into_vec();
        kept.sort_by_key(|s| s.key.1);
        self.out.extend(kept.into_iter().map(|s| s.read));
        if std::mem::take(&mut self.full) {
            let span = (w * rule.window, (w + 1) * rule.window);
            match self.windows.last_mut() {
                Some(last) if last.1 == span.0 => last.1 = span.1,
                _ => self.windows.push(span),
            }
        }
    }

    fn finish(mut self) -> (Vec<AlignedRead>, u64, Vec<(u64, u64)>) {
        self.flush();
        (self.out, self.left_out, self.windows)
    }
}

/// Depth over `region` from every read that passes `filters`, without keeping reads: in one
/// pass, or for a long region in chunks on several threads, added up to the same depth and the
/// same read counts (see [`ggv_query::chunks`]).
fn binned_depth(bam: &mut BamFile, region: &Region, filters: &PileupOptions, cancel: Option<&AtomicBool>) -> ggv_query::Result<(coverage::DepthAccumulator, ReadCounts)> {
    let parts = ggv_query::chunks::run(bam, region, |b, chunk, count_from| depth_pass(b, chunk, filters, count_from, cancel))?;
    let mut depth = coverage::DepthAccumulator::new(region);
    let mut counts = ReadCounts::default();
    for (d, c) in parts {
        depth.absorb(&d);
        counts.merge(&c);
    }
    Ok((depth, counts))
}

/// One pass of [`binned_depth`]; reads starting before `count_from` add depth but aren't counted.
fn depth_pass(bam: &mut BamFile, region: &Region, filters: &PileupOptions, count_from: u64, cancel: Option<&AtomicBool>) -> ggv_query::Result<(coverage::DepthAccumulator, ReadCounts)> {
    let mut depth = coverage::DepthAccumulator::new(region);
    let mut counts = ReadCounts::default();
    let fast = filters.read_filter.is_none();
    bam.for_each_record(region, |record| {
        check_cancel(cancel)?;
        let raw = record.as_ref();
        let mut scratch = ReadCounts::default();
        match raw_fields(raw).filter(|_| fast) {
            // Fast path: filter and count depth from the raw record.
            Some(f) => {
                let tally = if f.pos.max(0) as u64 >= count_from { &mut counts } else { &mut scratch };
                if tally.admit_fields(f.flags, f.mapq, filters) && f.pos >= 0 {
                    depth.add_cigar(f.pos as u64, f.cigar());
                }
            }
            // CIGAR stored in the CG tag (ultra-long reads), or a read test to apply:
            // decode properly.
            None => {
                let read = Read::new(raw.to_vec())?;
                let tally = if read.position().max(0) as u64 >= count_from { &mut counts } else { &mut scratch };
                if tally.admit(&read, filters, &region.contig) && read.position() >= 0 {
                    depth.add(&decode(&read, None));
                }
            }
        }
        Ok(())
    })?;
    Ok((depth, counts))
}

/// Depth per index window over `region`, estimated from the index's read density and scaled
/// to depth by counting sample windows exactly (with `filters` applied).
///
/// Bytes of alignment data per base differ along a chromosome: repeats full of MAPQ-0 reads,
/// duplicates and long tags all add bytes that a filter then drops. So the region is cut into
/// up to `segments` parts and each is calibrated on its own samples: the part's windows are split
/// by density into strata holding a third of its data each, and each stratum is calibrated on
/// runs of its own windows, from its middle outwards, until they hold its share of
/// [`CALIBRATION_BUDGET`] of alignment data. Sizing by data, not bases, keeps the sample big enough to be steady at low
/// depth (the index places reads only to the nearest compressed block, so a few windows are
/// noisy) and cheap at high depth. Parts whose sample is empty use the region-wide ratio. Returns the window size and one value per window of the contig (zero
/// outside `region`), or `None` without an index estimate or reads to calibrate with.
/// About this many bytes of alignment data (by the index) are counted to calibrate an estimate
/// in all, shared by its parts (each gets at least [`CALIBRATION_MIN_BYTES`] and at most
/// [`CALIBRATION_BYTES`]).
pub const CALIBRATION_BUDGET: f64 = 8.0 * 1024.0 * 1024.0;
pub const CALIBRATION_BYTES: f64 = 512.0 * 1024.0;
pub const CALIBRATION_MIN_BYTES: f64 = 128.0 * 1024.0;
/// Each part is calibrated in this many density strata, each a third of its data.
const STRATA: usize = 3;

pub fn estimate_depth(bam: &mut BamFile, region: &Region, filters: &PileupOptions, segments: usize) -> ggv_query::Result<Option<(u64, Vec<f64>)>> {
    let Some((window, bytes)) = bam.index_density(&region.contig) else { return Ok(None) };
    let contig_len = bam.contigs().iter().find(|c| c.name == region.contig).map_or(u64::MAX, |c| c.length);
    let w0 = (region.start / window) as usize;
    let w1 = (region.end.div_ceil(window) as usize).min(bytes.len());
    if w0 >= w1 {
        return Ok(None);
    }
    let parts = segments.clamp(1, 64).min(w1 - w0);
    let per_part = (CALIBRATION_BUDGET / parts as f64).clamp(CALIBRATION_MIN_BYTES, CALIBRATION_BYTES);
    let mut counts = ReadCounts::default();
    // Bases (clipped to the run) and bytes of a run of windows, counted exactly.
    let mut count_run = |run: std::ops::Range<usize>| -> ggv_query::Result<(f64, f64)> {
        let win = Region { contig: region.contig.clone(), start: run.start as u64 * window, end: (run.end as u64 * window).min(contig_len) };
        if win.start >= win.end {
            return Ok((0.0, 0.0));
        }
        let mut depth = coverage::DepthAccumulator::new(&win);
        let fast = filters.read_filter.is_none();
        bam.for_each_record(&win, |record| {
            let raw = record.as_ref();
            match raw_fields(raw).filter(|_| fast) {
                Some(f) => {
                    if counts.admit_fields(f.flags, f.mapq, filters) && f.pos >= 0 {
                        depth.add_cigar(f.pos as u64, f.cigar());
                    }
                }
                None => {
                    let read = Read::new(raw.to_vec())?;
                    if counts.admit(&read, filters, &win.contig) && read.position() >= 0 {
                        depth.add(&decode(&read, None));
                    }
                }
            }
            Ok(())
        })?;
        let bases = depth.into_bins(1).first().map_or(0.0, |b| b.mean) * win.len() as f64;
        Ok((bytes[run].iter().sum::<f64>(), bases))
    };
    // Per window: bases per byte, from the sample of its part and density stratum (NaN: none).
    let mut per_byte = vec![f64::NAN; bytes.len()];
    let (mut all_bytes, mut all_bases) = (0.0, 0.0);
    for k in 0..parts {
        let (a, b) = (w0 + (w1 - w0) * k / parts, w0 + (w1 - w0) * (k + 1) / parts);
        let mut order: Vec<usize> = (a..b).filter(|&w| bytes[w] > 0.0).collect();
        if order.is_empty() {
            continue;
        }
        order.sort_by(|&x, &y| bytes[x].total_cmp(&bytes[y]));
        // Strata by share of the part's data: the densest windows, which carry most of the
        // estimate, often hold a different mix (collapsed repeats, duplicates, unmapped mates
        // placed there) than the typical window, so each third of the data is calibrated on
        // its own sample (a single calibration once overestimated depth by far).
        let mass: f64 = order.iter().map(|&w| bytes[w]).sum();
        let strata = STRATA.min(order.len());
        let mut stratum = vec![usize::MAX; b - a];
        let mut cum = 0.0;
        for &w in &order {
            // The stratum holding this window's middle byte.
            stratum[w - a] = (((cum + bytes[w] / 2.0) / mass * strata as f64) as usize).min(strata - 1);
            cum += bytes[w];
        }
        let budget = per_part / strata as f64;
        for s in 0..strata {
            let members: Vec<usize> = order.iter().copied().filter(|&w| stratum[w - a] == s).collect();
            if members.is_empty() {
                continue;
            }
            // Start at the stratum's middle (by data) and take runs of adjacent windows of the
            // same stratum, nearest in density first, until the sample holds its share.
            let half = members.iter().map(|&w| bytes[w]).sum::<f64>() / 2.0;
            let mut acc = 0.0;
            let seed = members.iter().position(|&w| {
                acc += bytes[w];
                acc >= half
            }).unwrap_or(0);
            let mut near: Vec<usize> = (0..members.len()).collect();
            near.sort_by_key(|&i| i.abs_diff(seed));
            let mut used = vec![false; b - a];
            let (mut sb, mut sd) = (0.0, 0.0);
            for i in near {
                if sb >= budget {
                    break;
                }
                let w = members[i];
                if used[w - a] {
                    continue;
                }
                let (mut lo, mut hi) = (w, w + 1);
                let mut held = bytes[w];
                used[w - a] = true;
                let same = |x: usize| stratum[x - a] == s;
                while sb + held < budget {
                    if hi < b && same(hi) && !used[hi - a] {
                        used[hi - a] = true;
                        held += bytes[hi];
                        hi += 1;
                    } else if lo > a && same(lo - 1) && !used[lo - 1 - a] {
                        lo -= 1;
                        used[lo - a] = true;
                        held += bytes[lo];
                    } else {
                        break;
                    }
                }
                let (rb, rd) = count_run(lo..hi)?;
                sb += rb;
                sd += rd;
            }
            all_bytes += sb;
            all_bases += sd;
            if sb > 0.0 {
                for &w in &members {
                    per_byte[w] = sd / sb;
                }
            }
        }
    }
    if all_bytes <= 0.0 {
        return Ok(None);
    }
    let fallback = all_bases / all_bytes;
    let mut out = vec![0.0; bytes.len()];
    for w in w0..w1 {
        let r = if per_byte[w].is_nan() { fallback } else { per_byte[w] };
        out[w] = bytes[w] * r / window as f64;
    }
    Ok(Some((window, out)))
}

/// Averages per-window values (window `w` covers `[w * window, (w + 1) * window)`) into `bins`
/// equal bins over `region`.
fn resample(values: &[f64], window: u64, region: &Region, bins: usize) -> Vec<CoverageBin> {
    let bins = bins.max(1);
    let len = region.len().max(1) as f64;
    (0..bins)
        .map(|i| {
            let s = region.start as f64 + len * i as f64 / bins as f64;
            let e = region.start as f64 + len * (i + 1) as f64 / bins as f64;
            let (a, b) = ((s / window as f64).floor() as usize, ((e / window as f64).ceil() as usize).max((s / window as f64).floor() as usize + 1));
            // Weight each window by how much of it falls in the bin.
            let (mut sum, mut wsum) = (0.0, 0.0);
            for w in a..b.min(values.len()) {
                let ws = (w as u64 * window) as f64;
                let overlap = (e.min(ws + window as f64) - s.max(ws)).max(0.0);
                sum += values[w] * overlap;
                wsum += overlap;
            }
            let mean = if wsum > 0.0 { sum / wsum } else { 0.0 };
            CoverageBin { mean, max: mean.round() as u32 }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ggv_model::Region;

    /// An encoded record with the given CIGAR (`(len, op)` pairs) and sequence length.
    fn record(pos: i32, cigar: &[(u32, u32)], l_seq: u32) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend(0i32.to_le_bytes());
        v.extend(pos.to_le_bytes());
        v.push(3); // name length incl. NUL
        v.push(60);
        v.extend(0u16.to_le_bytes());
        v.extend((cigar.len() as u16).to_le_bytes());
        v.extend(0u16.to_le_bytes()); // flags
        v.extend(l_seq.to_le_bytes());
        v.extend((-1i32).to_le_bytes());
        v.extend((-1i32).to_le_bytes());
        v.extend(0i32.to_le_bytes());
        v.extend(b"r1\0");
        for (len, op) in cigar {
            v.extend((len << 4 | op).to_le_bytes());
        }
        v.extend(std::iter::repeat_n(0x11u8, l_seq.div_ceil(2) as usize));
        v.extend(std::iter::repeat_n(30u8, l_seq as usize));
        v
    }

    #[test]
    fn raw_cigar_depth_matches_decoded_depth() {
        let region = Region { contig: "c".into(), start: 90, end: 140 };
        // 2S 4M 1I 3M 2D 3M 5N 4= 1X 3S, starting before the region and running past a skip.
        let cigar = [(2, 4), (4, 0), (1, 1), (3, 0), (2, 2), (3, 0), (5, 3), (4, 7), (1, 8), (3, 4)];
        let l_seq = 2 + 4 + 1 + 3 + 3 + 4 + 1 + 3;
        for pos in [85, 100, 130] {
            let raw = record(pos, &cigar, l_seq);
            let mut fast = coverage::DepthAccumulator::new(&region);
            let f = raw_fields(&raw).unwrap();
            fast.add_cigar(f.pos as u64, f.cigar());
            let mut slow = coverage::DepthAccumulator::new(&region);
            slow.add(&decode(&Read::new(raw).unwrap(), None));
            assert_eq!(fast.into_bins(50), slow.into_bins(50), "read at {pos}");
        }
    }

    #[test]
    fn cg_placeholder_falls_back() {
        // kSmN: whole read soft-clipped, then a reference skip: the real CIGAR is in CG.
        let raw = record(100, &[(10, 4), (50, 3)], 10);
        assert!(raw_fields(&raw).is_none());
        assert!(raw_fields(&record(100, &[(10, 0), (50, 3)], 10)).is_some());
    }
}
