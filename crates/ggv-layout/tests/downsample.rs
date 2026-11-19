//! Downsampling keeps fewer reads to draw but changes no number: coverage and read counts are
//! those of every read, on one thread or split across several (tests/data/hg002-revio).

use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use ggv_io::{BamFile, ReferenceFasta};
use ggv_layout::{Coverage, Downsample, ViewOptions, load_view};

fn data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/hg002-revio").join(name)
}

fn open() -> BamFile {
    BamFile::open_with_reference(data("HG002.revio.slices.bam"), Some(data("GRCh38.chr20.slices.fa.gz"))).unwrap()
}

const LOCUS: &str = "chr20:58,800,000-58,950,000";

fn per_base(c: &Coverage) -> &[ggv_layout::CoverageColumn] {
    let Coverage::PerBase(c) = c else { panic!("expected per-base coverage") };
    c
}

#[test]
fn sampled_loads_keep_every_number() {
    let mut bam = open();
    let region = bam.resolve_str(LOCUS).unwrap();
    let reference = ReferenceFasta::open(data("GRCh38.chr20.slices.fa.gz")).unwrap().fetch(&region).unwrap();
    let base = ViewOptions { max_read_window: 1_000_000, max_rows: 0, ..Default::default() };
    let all = load_view(&mut bam, &region, Some(reference.clone()), String::new(), &base).unwrap();
    // HiFi reads are ~15 kb at ~30x, so only ~10 start per 5 kb: a quota of 3 there samples.
    let rule = Downsample { window: 5_000, count: 3 };

    // One pass, and chunks small enough to split the region over several threads (reads
    // crossing a chunk's end add their coverage past it once the chunks are joined).
    for read_chunk in [u64::MAX / 4, 20_000] {
        let opts = ViewOptions { downsample: Some(rule), read_chunk, ..base.clone() };
        let sampled = load_view(&mut bam, &region, Some(reference.clone()), String::new(), &opts).unwrap();

        assert_eq!(per_base(&sampled.coverage), per_base(&all.coverage), "chunk {read_chunk}: coverage must count every read");
        assert_eq!(format!("{:?}", sampled.read_counts), format!("{:?}", all.read_counts), "chunk {read_chunk}: read counts");

        let (all_reads, _) = all.reads.as_ref().unwrap();
        let (kept, _) = sampled.reads.as_ref().unwrap();
        assert!(sampled.reads_left_out > 0, "chunk {read_chunk}: 3 reads per 5 kb must leave some out");
        assert_eq!(kept.len() as u64 + sampled.reads_left_out, all_reads.len() as u64, "chunk {read_chunk}: kept + left out = all");
        assert!(kept.windows(2).all(|w| w[0].start <= w[1].start), "chunk {read_chunk}: kept reads stay in position order");
        // At most `count` per window; a window cut by a chunk boundary may hold one quota per side.
        let mut per_window = std::collections::HashMap::<u64, usize>::new();
        for r in kept {
            *per_window.entry(r.start / rule.window).or_default() += 1;
        }
        let limit = if read_chunk < region.len() { 2 * rule.count } else { rule.count };
        assert!(per_window.values().all(|&n| n <= limit), "chunk {read_chunk}: a window kept more than its quota");
        // Every window that lost reads is marked, and the marks are sorted and disjoint.
        assert!(!sampled.downsampled.is_empty());
        assert!(sampled.downsampled.windows(2).all(|w| w[0].1 < w[1].0), "marks are merged and in order");
    }
}

#[test]
fn sampling_is_the_same_on_every_load() {
    let mut bam = open();
    let region = bam.resolve_str(LOCUS).unwrap();
    let opts = ViewOptions { downsample: Some(Downsample { window: 5_000, count: 3 }), max_read_window: 1_000_000, max_rows: 0, ..Default::default() };
    let names = |bam: &mut BamFile| {
        let v = load_view(bam, &region, None, String::new(), &opts).unwrap();
        v.reads.unwrap().0.into_iter().map(|r| (r.name, r.start, r.flags)).collect::<Vec<_>>()
    };
    let first = names(&mut bam);
    assert_eq!(first, names(&mut bam));
}

#[test]
fn without_sampling_nothing_is_left_out() {
    let mut bam = open();
    let region = bam.resolve_str(LOCUS).unwrap();
    let v = load_view(&mut bam, &region, None, String::new(), &ViewOptions { max_read_window: 1_000_000, max_rows: 0, ..Default::default() }).unwrap();
    assert!(v.reads.is_some());
    assert_eq!(v.reads_left_out, 0);
    assert!(v.downsampled.is_empty());
}

#[test]
fn a_cancelled_load_stops_with_interrupted() {
    let mut bam = open();
    let region = bam.resolve_str(LOCUS).unwrap();
    let opts = ViewOptions { cancel: Some(Arc::new(AtomicBool::new(true))), max_read_window: 1_000_000, max_rows: 0, ..Default::default() };
    let err = load_view(&mut bam, &region, None, String::new(), &opts).unwrap_err();
    assert!(err.to_string().contains("cancelled"), "{err}");
    // Coverage-only (wide) loads too.
    let wide = bam.resolve_str("chr20:58,000,000-59,000,000").unwrap();
    let opts = ViewOptions { max_read_window: 0, ..opts };
    assert!(load_view(&mut bam, &wide, None, String::new(), &opts).is_err());
}
