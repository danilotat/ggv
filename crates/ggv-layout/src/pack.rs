use serde::Serialize;

use crate::AlignedRead;

/// Reads assigned to rows, IGV-style: each read goes in the first row where it fits.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Packed {
    /// Indices into the read list, per row, in start order. Grouped packings have an empty
    /// row between groups.
    pub rows: Vec<Vec<usize>>,
    /// Reads that would have needed a row past `max_rows`.
    pub hidden_reads: usize,
    /// Groups in display order (empty when not grouped).
    pub groups: Vec<Group>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Group {
    pub label: String,
    /// The group's first row in [`Packed::rows`].
    pub first_row: usize,
    pub rows: usize,
    pub reads: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub enum GroupBy {
    #[default]
    None,
    Strand,
    /// `HP` tag (phased long reads).
    Haplotype,
    /// `RG` tag.
    ReadGroup,
    /// The base each read shows at this 0-based position: each alternate base, deletions, the
    /// reference, then reads not covering it. Phases reads at a het site without `HP` tags.
    BaseAt(u64),
    /// Haplotype by the alleles reads carry at the heterozygous sites in the loaded span
    /// (see [`crate::phase`]): phase A, phase B, then reads with no informative site. With
    /// several phase blocks the two bands hold every block's A and B, which the labels say:
    /// a band per block would be mostly empty, each block's reads lying under its own sites.
    Alleles,
    /// The value of this tag (`CB`, `PS`, `UB`...): the reads must be loaded with it
    /// (`ViewOptions::tag`). Reads without it come last.
    Tag([u8; 2]),
    /// The contig the mate maps to (IGV's "chromosome of mate"): the read's own contig first,
    /// then the others in header order, then reads whose mate is unmapped, then unpaired reads.
    /// Named from [`PackOptions::contig_names`].
    MateContig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum SortBy {
    /// Mismatching bases (A, C, G, T, N), then deletions, then reference matches.
    Base,
    /// Forward first.
    Strand,
    /// Highest mapping quality first.
    MappingQuality,
    /// Earliest start first.
    Start,
    /// Largest absolute insert size first.
    InsertSize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct PackOptions {
    pub max_rows: usize,
    pub min_gap: u64,
    pub group: GroupBy,
    /// Sort rows by the read covering this 0-based position.
    pub sort: Option<(u64, SortBy)>,
    /// Keep the parts of a split read (its primary and supplementary records, same name and
    /// mate) on one row, spanning from the first part to the last, grouped by the primary.
    pub link: bool,
    /// Keep the two mates of a pair (their primary records) on one row, spanning the insert
    /// between them, grouped by the leftmost (IGV's "view as pairs"). With `link`, a split
    /// mate's supplementary parts join them.
    pub pairs: bool,
    /// Soft clips are drawn: reads take their clipped bases' room too, so clips never cover
    /// a neighbour.
    pub clips: bool,
    /// The alignment file's contig names in header order ([`crate::ViewData::contig_names`]),
    /// which [`crate::Mate::contig_id`] indexes: the labels of [`GroupBy::MateContig`].
    #[serde(skip)]
    pub contig_names: Option<std::sync::Arc<[Box<str>]>>,
}

/// The reference span a read takes in a row: its aligned span, widened by its soft clips when
/// they are shown (drawn where they would align).
pub fn extent(r: &AlignedRead, clips: bool) -> (u64, u64) {
    if clips {
        (r.start.saturating_sub(u64::from(r.soft_clip_left)), r.end + u64::from(r.soft_clip_right))
    } else {
        (r.start, r.end)
    }
}

/// Packs reads (in coordinate order, as a BAM query returns them) into at most `max_rows`
/// rows, leaving at least `min_gap` bases between neighbours.
pub fn pack(reads: &[AlignedRead], max_rows: usize, min_gap: u64) -> Packed {
    pack_with(reads, &PackOptions { max_rows, min_gap, ..Default::default() })
}

/// Whether `a` and `b` are parts of one split read: same name, same mate (first / second).
pub fn same_template(a: &AlignedRead, b: &AlignedRead) -> bool {
    a.name == b.name && (a.flags & 0xC0) == (b.flags & 0xC0)
}

/// Whether `a` and `b` are records of the two mates of one pair: paired, same name, one the
/// first mate and the other the second.
pub fn same_pair(a: &AlignedRead, b: &AlignedRead) -> bool {
    a.flags & b.flags & 0x1 != 0 && a.name == b.name && (a.flags & 0xC0) != (b.flags & 0xC0)
}

/// Which unit a read joins, if any: its name and a kind (a split read's mate bits, or `PAIR`
/// for a whole pair). Mates join on their primary records only; secondary and supplementary
/// records of a pair stay alone unless split reads are linked too.
fn unit_key(r: &AlignedRead, link: bool, pairs: bool) -> Option<(&str, u16)> {
    const PAIR: u16 = 0xFFFF;
    let paired = r.flags & 0x1 != 0;
    let split = link && (!r.supplementary.is_empty() || r.flags & 0x800 != 0);
    if pairs && paired && (split || r.flags & 0x900 == 0) {
        Some((r.name.as_str(), PAIR))
    } else if split {
        Some((r.name.as_str(), r.flags & 0xC0))
    } else {
        None
    }
}

/// Units of reads to place together: each split read's parts (in start order, the primary
/// first among equals), every other read alone. Leaders (the unit's primary, else its first
/// part) decide the group.
fn units(reads: &[AlignedRead], order: &[usize], link: bool, pairs: bool) -> Vec<Vec<usize>> {
    let mut by_key: std::collections::HashMap<(&str, u16), usize> = std::collections::HashMap::new();
    let mut out: Vec<Vec<usize>> = Vec::new();
    for &i in order {
        let Some(key) = unit_key(&reads[i], link, pairs) else {
            out.push(vec![i]);
            continue;
        };
        match by_key.get(&key) {
            Some(&u) => out[u].push(i),
            None => {
                by_key.insert(key, out.len());
                out.push(vec![i]);
            }
        }
    }
    for u in &mut out {
        u.sort_by_key(|&i| (reads[i].start, reads[i].flags & 0x800 != 0));
    }
    out
}

fn leader(reads: &[AlignedRead], unit: &[usize]) -> usize {
    unit.iter().copied().find(|&i| reads[i].flags & 0x800 == 0).unwrap_or(unit[0])
}

/// First-fit packing of whole units (a unit spans its first part's start to its last part's
/// end, and its parts share a row).
fn first_fit_units(reads: &[AlignedRead], units: &[Vec<usize>], max_rows: usize, min_gap: u64, clips: bool) -> (Vec<Vec<usize>>, usize) {
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut ends: Vec<u64> = Vec::new();
    let mut hidden = 0;
    let mut placed: Vec<(u64, u64, &Vec<usize>)> = units
        .iter()
        .map(|u| (u.iter().map(|&i| extent(&reads[i], clips).0).min().unwrap_or(0), u.iter().map(|&i| extent(&reads[i], clips).1).max().unwrap_or(0), u))
        .collect();
    placed.sort_by_key(|p| (p.0, p.1));
    for (start, end, u) in placed {
        match ends.iter().position(|&e| e + min_gap <= start) {
            Some(row) => {
                ends[row] = end;
                rows[row].extend(u);
            }
            None if rows.len() < max_rows => {
                ends.push(end);
                rows.push(u.clone());
            }
            None => hidden += u.len(),
        }
    }
    (rows, hidden)
}

/// Packs with optional grouping and sorting. Each group is packed on its own, in its own band
/// of rows. Sorting reorders rows (not reads) by the read covering the sort position, as IGV
/// does: reads stay first-fit packed, and the rows through that position come out in order.
pub fn pack_with(reads: &[AlignedRead], opts: &PackOptions) -> Packed {
    pack_indices(reads, (0..reads.len()).collect(), opts)
}

/// [`pack_with`] for a subset of the reads (indices into `reads`), e.g. those passing filters.
pub fn pack_indices(reads: &[AlignedRead], mut order: Vec<usize>, opts: &PackOptions) -> Packed {
    order.sort_by_key(|&i| extent(&reads[i], opts.clips));

    let phasing = (opts.group == GroupBy::Alleles).then(|| crate::phase::phase(reads, &order));
    let key_of = |i: usize| match &phasing {
        Some(p) => GroupKey::Phase(match p.hap[i] {
            Some(true) => 0,
            Some(false) => 1,
            None => 2,
        }),
        None => group_key(&reads[i], opts.group),
    };
    // With linking, whole split reads go to their primary's group; with pairs, whole pairs
    // to their leftmost mate's.
    let linked = (opts.link || opts.pairs).then(|| units(reads, &order, opts.link, opts.pairs));
    let mut groups: Vec<(GroupKey, Vec<usize>)> = Vec::new();
    let mut unit_groups: Vec<(GroupKey, Vec<Vec<usize>>)> = Vec::new();
    match &linked {
        Some(us) => {
            for u in us {
                let key = key_of(leader(reads, u));
                match unit_groups.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, members)) => members.push(u.clone()),
                    None => unit_groups.push((key, vec![u.clone()])),
                }
            }
            unit_groups.sort_by(|a, b| a.0.cmp(&b.0));
            groups = unit_groups.iter().map(|(k, us)| (k.clone(), us.iter().flatten().copied().collect())).collect();
        }
        None => {
            for i in order {
                let key = key_of(i);
                match groups.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, members)) => members.push(i),
                    None => groups.push((key, vec![i])),
                }
            }
        }
    }
    groups.sort_by(|a, b| a.0.cmp(&b.0));

    let mut packed = Packed::default();
    let grouped = opts.group != GroupBy::None;
    for (gi, (key, members)) in groups.into_iter().enumerate() {
        if grouped && !packed.rows.is_empty() {
            packed.rows.push(Vec::new()); // a gap row between groups
        }
        let budget = opts.max_rows.saturating_sub(packed.rows.len());
        let (mut rows, hidden) = match &linked {
            Some(_) => first_fit_units(reads, &unit_groups[gi].1, budget, opts.min_gap, opts.clips),
            None => first_fit(reads, &members, budget, opts.min_gap, opts.clips),
        };
        if let Some((pos, by)) = opts.sort {
            sort_rows(reads, &mut rows, pos, by);
        }
        if grouped {
            let mut label = key.label(opts.contig_names.as_deref());
            if let (Some(p), GroupKey::Phase(rank)) = (&phasing, &key) {
                label = match (rank, p.sites.len()) {
                    (2, 0) => "no het sites found (they need a reference)".into(),
                    (2, _) => label,
                    (_, n) if p.blocks == 1 => format!("{label} · {n} het site{} · 1 block", if n == 1 { "" } else { "s" }),
                    // Said in the label: block 1's A and block 2's A are different haplotypes
                    // as often as not, and the band holds both.
                    (_, n) => format!("{label} · {n} het sites · {} blocks, A and B set per block", p.blocks),
                };
            }
            packed.groups.push(Group { label, first_row: packed.rows.len(), rows: rows.len(), reads: members.len() });
        }
        packed.hidden_reads += hidden;
        packed.rows.extend(rows);
    }
    packed
}

fn first_fit(reads: &[AlignedRead], members: &[usize], max_rows: usize, min_gap: u64, clips: bool) -> (Vec<Vec<usize>>, usize) {
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut ends: Vec<u64> = Vec::new();
    let mut hidden = 0;
    for &i in members {
        let (start, end) = extent(&reads[i], clips);
        match ends.iter().position(|&e| e + min_gap <= start) {
            Some(row) => {
                ends[row] = end;
                rows[row].push(i);
            }
            None if rows.len() < max_rows => {
                ends.push(end);
                rows.push(vec![i]);
            }
            None => hidden += 1,
        }
    }
    (rows, hidden)
}

/// Group order: strand +/−; haplotype 1, 2, …, then untagged; read groups by name, then none.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum GroupKey {
    All,
    Strand(bool),
    /// `(missing, value)`: the flag sorts untagged reads after every tagged group.
    Haplotype(bool, u8),
    ReadGroup(bool, Box<str>),
    /// `(tag, missing, numeric value, text)`: numbers sort as numbers.
    Tag([u8; 2], bool, i64, Box<str>),
    /// `(rank, base)`: 0 = an alternate base, 1 = deletion, 2 = reference, 3 = not covering.
    BaseAt(u8, u8),
    /// 0 = phase A, 1 = phase B, 2 = unassigned.
    Phase(u8),
    /// `(rank, contig id)`: 0 = mate on the read's contig, 1 = on another, 2 = mate unmapped,
    /// 3 = not paired.
    MateContig(u8, i32),
}

impl GroupKey {
    fn label(&self, contig_names: Option<&[Box<str>]>) -> String {
        match self {
            Self::MateContig(r @ (0 | 1), id) => {
                let name = usize::try_from(*id).ok().and_then(|i| contig_names?.get(i)).map_or_else(|| format!("contig #{id}"), |n| n.to_string());
                if *r == 0 { format!("mate on {name} (same)") } else { format!("mate on {name}") }
            }
            Self::MateContig(2, _) => "mate unmapped".into(),
            Self::MateContig(..) => "not paired".into(),
            Self::Tag(t, true, _, _) => format!("no {}", String::from_utf8_lossy(t)),
            Self::Tag(t, false, _, v) => format!("{}: {v}", String::from_utf8_lossy(t)),
            Self::All => String::new(),
            Self::Strand(false) => "+ strand".into(),
            Self::Strand(true) => "− strand".into(),
            Self::Haplotype(false, h) => format!("HP {h}"),
            Self::Haplotype(true, _) => "no HP".into(),
            Self::ReadGroup(false, rg) => format!("RG {rg}"),
            Self::ReadGroup(true, _) => "no RG".into(),
            Self::BaseAt(0, b) => format!("{}", *b as char),
            Self::BaseAt(1, _) => "deletion".into(),
            Self::BaseAt(2, b'?') => "base unknown".into(),
            Self::BaseAt(2, _) => "reference".into(),
            Self::BaseAt(..) => "not covering".into(),
            Self::Phase(0) => "allele phase A".into(),
            Self::Phase(1) => "allele phase B".into(),
            Self::Phase(_) => "no informative site".into(),
        }
    }
}

fn group_key(r: &AlignedRead, by: GroupBy) -> GroupKey {
    match by {
        GroupBy::None | GroupBy::Alleles => GroupKey::All,
        GroupBy::Strand => GroupKey::Strand(r.reverse),
        GroupBy::Haplotype => GroupKey::Haplotype(r.haplotype.is_none(), r.haplotype.unwrap_or(0)),
        GroupBy::ReadGroup => GroupKey::ReadGroup(r.read_group.is_none(), r.read_group.clone().unwrap_or_default()),
        GroupBy::MateContig => match r.mate {
            Some(m) if m.same_contig => GroupKey::MateContig(0, m.contig_id),
            Some(m) => GroupKey::MateContig(1, m.contig_id),
            None if r.flags & 0x1 != 0 => GroupKey::MateContig(2, 0),
            None => GroupKey::MateContig(3, 0),
        },
        GroupBy::Tag(t) => {
            let v = r.tag.clone().unwrap_or_default();
            GroupKey::Tag(t, r.tag.is_none(), v.parse().unwrap_or(i64::MAX), v)
        }
        GroupBy::BaseAt(pos) => {
            if !(r.start <= pos && pos < r.end) || r.ref_skips.iter().any(|d| (d.pos..d.pos + u64::from(d.len)).contains(&pos)) {
                GroupKey::BaseAt(3, 0)
            } else if let Some(m) = r.mismatches.iter().find(|m| m.pos == pos) {
                GroupKey::BaseAt(0, m.base)
            } else if r.deletions.iter().any(|d| (d.pos..d.pos + u64::from(d.len)).contains(&pos)) {
                GroupKey::BaseAt(1, 0)
            } else if r.bases.is_some() {
                // Loaded without a reference: the read's own base, as there is no "reference".
                r.base_at(pos).map_or(GroupKey::BaseAt(2, b'?'), |b| GroupKey::BaseAt(0, b))
            } else {
                GroupKey::BaseAt(2, 0)
            }
        }
    }
}

/// What a read shows at `pos`, for sorting by base.
fn base_rank(r: &AlignedRead, pos: u64) -> (u8, u8) {
    let base = r.mismatches.iter().find(|m| m.pos == pos).map(|m| m.base).or_else(|| r.base_at(pos));
    if let Some(base) = base {
        let b = match base {
            b'A' => 0,
            b'C' => 1,
            b'G' => 2,
            b'T' => 3,
            _ => 4,
        };
        return (0, b);
    }
    if r.deletions.iter().any(|d| (d.pos..d.pos + u64::from(d.len)).contains(&pos)) {
        return (1, 0);
    }
    (2, 0)
}

fn sort_rows(reads: &[AlignedRead], rows: &mut [Vec<usize>], pos: u64, by: SortBy) {
    // Rows without a read over `pos` go last, in their packed order (the sort is stable).
    let key = |row: &Vec<usize>| -> (u8, i64, i64) {
        let Some(r) = row.iter().map(|&i| &reads[i]).find(|r| r.start <= pos && pos < r.end) else {
            return (1, 0, 0);
        };
        match by {
            SortBy::Base => {
                let (a, b) = base_rank(r, pos);
                (0, i64::from(a), i64::from(b))
            }
            SortBy::Strand => (0, i64::from(r.reverse), 0),
            SortBy::MappingQuality => (0, -i64::from(r.mapq), 0),
            SortBy::Start => (0, r.start as i64, 0),
            SortBy::InsertSize => (0, -i64::from(r.template_length.unsigned_abs().min(i32::MAX as u32) as i32), 0),
        }
    };
    rows.sort_by_cached_key(key);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Indel, Mismatch};

    fn read(start: u64, end: u64) -> AlignedRead {
        AlignedRead {
            name: String::new(),
            start,
            end,
            reverse: false,
            mapq: 60,
            flags: 0,
            blocks: vec![],
            deletions: vec![],
            ref_skips: vec![],
            insertions: vec![],
            mismatches: vec![],
            soft_clip_left: 0,
            soft_clip_right: 0,
            clip_mismatches: vec![],
            haplotype: None,
            read_group: None,
            template_length: 0,
            mate: None,
            supplementary: vec![],
            mods: vec![],
            low_quality: vec![],
            tag: None,
            bases: None,
        }
    }

    /// Chromosome of mate: the read's own contig first, others in header order (named from
    /// the header), then mate unmapped, then unpaired.
    #[test]
    fn groups_by_the_mates_contig() {
        use crate::Mate;
        let mate = |same, id| Some(Mate { same_contig: same, contig_id: id, pos: 0, reverse: false });
        let mut reads: Vec<AlignedRead> = (0..6).map(|i| read(i * 20, i * 20 + 10)).collect();
        reads[0].mate = mate(false, 2);
        reads[1].mate = mate(true, 0);
        reads[2].flags = 0x1 | 0x8; // paired, mate unmapped
        reads[3].mate = mate(false, 1);
        reads[4].mate = mate(true, 0);
        // reads[5]: not paired
        let names: std::sync::Arc<[Box<str>]> = ["chr1", "chr5", "chrX"].map(Box::from).into();
        let p = pack_with(&reads, &PackOptions { contig_names: Some(names), ..opts(GroupBy::MateContig, None) });
        let groups: Vec<(&str, usize)> = p.groups.iter().map(|g| (g.label.as_str(), g.reads)).collect();
        assert_eq!(groups, [("mate on chr1 (same)", 2), ("mate on chr5", 1), ("mate on chrX", 1), ("mate unmapped", 1), ("not paired", 1)]);
        // Without the header's names the contig is still told apart.
        let p = pack_with(&reads, &opts(GroupBy::MateContig, None));
        assert_eq!(p.groups[1].label, "mate on contig #1");
    }

    #[test]
    fn first_fit_with_gap() {
        // 0..10 and 12..20 share a row (gap 2); 11..15 overlaps both and needs a second row.
        let reads = vec![read(0, 10), read(11, 15), read(12, 20), read(30, 40)];
        let p = pack(&reads, 10, 2);
        assert_eq!(p.rows, vec![vec![0, 2, 3], vec![1]]);
        assert_eq!(p.hidden_reads, 0);
    }

    #[test]
    fn shown_soft_clips_take_room_in_the_row() {
        // 0..10 clipped 5 on the right (drawn to 15), 30..40 clipped 10 on the left (from 20):
        // 12..18 fits beside the aligned spans but not beside the clips.
        let mut reads = vec![read(0, 10), read(12, 18), read(30, 40)];
        reads[0].soft_clip_right = 5;
        reads[2].soft_clip_left = 10;
        let o = |clips| PackOptions { clips, ..opts(GroupBy::None, None) };
        assert_eq!(pack_with(&reads, &o(false)).rows, vec![vec![0, 1, 2]]);
        assert_eq!(pack_with(&reads, &o(true)).rows, vec![vec![0, 2], vec![1]]);
        // Sticky packing too: a hint can't put a read under a neighbour's clip.
        let hints = row_hints(&reads, &pack_with(&reads, &o(false)));
        assert_eq!(pack_sticky(&reads, vec![0, 1, 2], &o(true), &hints).rows, vec![vec![0, 2], vec![1]]);
    }

    #[test]
    fn linked_split_reads_share_a_row_in_their_primary_group() {
        let split = crate::SplitAlignment { contig: "c".into(), pos: 0, reverse: false, mapq: 60, cigar: "10M".into() };
        let named = |name: &str, start, end, flags, hp| AlignedRead {
            name: name.into(),
            flags,
            haplotype: hp,
            supplementary: if flags & 0x800 != 0 || name == "s" { vec![split.clone()] } else { vec![] },
            ..read(start, end)
        };
        // "s": primary 0..10 (HP 1) and supplementary 50..60 (no HP); "x" sits between them,
        // and a mate pair ("m", first and second) must not be linked.
        let reads = vec![
            named("s", 0, 10, 0, Some(1)),
            named("x", 20, 30, 0, Some(1)),
            named("s", 50, 60, 0x800, None),
            named("m", 70, 80, 0x41, Some(1)),
            named("m", 90, 100, 0x81, Some(1)),
        ];
        let linked = pack_with(&reads, &PackOptions { link: true, ..opts(GroupBy::Haplotype, None) });
        // One group (HP 1): the supplementary part follows its primary.
        assert_eq!(linked.groups.iter().map(|g| (g.label.as_str(), g.reads)).collect::<Vec<_>>(), [("HP 1", 5)]);
        // "s" spans 0..60 on one row; "x" can't fit inside it, so it gets the next row.
        let row_of = |i| linked.rows.iter().position(|r| r.contains(&i)).unwrap();
        assert_eq!(row_of(0), row_of(2));
        assert_ne!(row_of(1), row_of(0));
        assert!(same_template(&reads[0], &reads[2]) && !same_template(&reads[3], &reads[4]));
        // Unlinked: the supplementary record is grouped on its own (no HP).
        let plain = pack_with(&reads, &opts(GroupBy::Haplotype, None));
        assert_eq!(plain.groups.iter().map(|g| (g.label.as_str(), g.reads)).collect::<Vec<_>>(), [("HP 1", 4), ("no HP", 1)]);
    }

    #[test]
    fn pairs_share_a_row_spanning_the_insert() {
        let named = |name: &str, start, end, flags, hp| AlignedRead { name: name.into(), flags, haplotype: hp, ..read(start, end) };
        // "p": mates at 0..10 (HP 1, first) and 50..60 (HP 2, second), with a supplementary
        // part of the second mate at 80..90 and a secondary record at 100..110. "x" sits in
        // the insert; "u" is unpaired; "q" has its mate outside the loaded reads.
        let reads = vec![
            named("p", 0, 10, 0x41, Some(1)),
            named("x", 20, 30, 0x41, Some(1)),
            named("p", 50, 60, 0x81, Some(2)),
            named("p", 80, 90, 0x881, None),
            named("p", 100, 110, 0x141, None),
            named("u", 120, 130, 0, Some(1)),
            named("q", 140, 150, 0x41, Some(1)),
        ];
        let row_of = |p: &Packed, i| p.rows.iter().position(|r| r.contains(&i)).unwrap();
        let pairs = pack_with(&reads, &PackOptions { pairs: true, ..opts(GroupBy::None, None) });
        assert_eq!(row_of(&pairs, 0), row_of(&pairs, 2), "mates on one row");
        assert_ne!(row_of(&pairs, 1), row_of(&pairs, 0), "the insert is taken");
        // Without linking, the supplementary and secondary records stay apart: here they fit
        // after the pair on its row, but they aren't part of it (x's row is first-fit too).
        assert_eq!(pairs.rows.iter().flatten().count(), reads.len());
        assert!(same_pair(&reads[0], &reads[2]) && !same_pair(&reads[0], &reads[1]) && !same_pair(&reads[2], &reads[3]));
        assert!(!same_pair(&reads[5], &reads[5]), "unpaired reads have no mate");

        // Grouped: the pair follows its leftmost mate (HP 1), though its other mate is HP 2.
        let grouped = pack_with(&reads, &PackOptions { pairs: true, ..opts(GroupBy::Haplotype, None) });
        let hp1 = &grouped.groups[0];
        assert_eq!(hp1.label, "HP 1");
        assert!((hp1.first_row..hp1.first_row + hp1.rows).contains(&row_of(&grouped, 2)));

        // With linking too, the supplementary part joins the pair; the secondary doesn't.
        let both = pack_with(&reads, &PackOptions { pairs: true, link: true, ..opts(GroupBy::Haplotype, None) });
        assert_eq!(row_of(&both, 0), row_of(&both, 3));
        assert_eq!(both.groups.iter().map(|g| (g.label.as_str(), g.reads)).collect::<Vec<_>>(), [("HP 1", 6), ("no HP", 1)]);

        // Off: mates pack apart (the second mate is HP 2).
        let plain = pack_with(&reads, &opts(GroupBy::Haplotype, None));
        assert_ne!(plain.groups.iter().position(|g| (g.first_row..g.first_row + g.rows).contains(&row_of(&plain, 0))),
                   plain.groups.iter().position(|g| (g.first_row..g.first_row + g.rows).contains(&row_of(&plain, 2))));
    }

    #[test]
    fn caps_rows() {
        let reads: Vec<_> = (0..5).map(|_| read(0, 10)).collect();
        let p = pack(&reads, 3, 2);
        assert_eq!(p.rows.len(), 3);
        assert_eq!(p.hidden_reads, 2);
    }

    fn opts(group: GroupBy, sort: Option<(u64, SortBy)>) -> PackOptions {
        PackOptions { max_rows: 100, min_gap: 2, group, sort, ..Default::default() }
    }

    #[test]
    fn sort_by_base_puts_mismatches_then_deletions_then_matches() {
        let mut reads: Vec<AlignedRead> = (0..4).map(|_| read(0, 100)).collect();
        // read 0 matches; read 1 has T at 50; read 2 deletes 50; read 3 has A at 50.
        reads[1].mismatches.push(Mismatch { pos: 50, base: b'T', quality: 30 });
        reads[2].deletions.push(Indel { pos: 49, len: 3 });
        reads[3].mismatches.push(Mismatch { pos: 50, base: b'A', quality: 30 });
        let p = pack_with(&reads, &opts(GroupBy::None, Some((50, SortBy::Base))));
        assert_eq!(p.rows, vec![vec![3], vec![1], vec![2], vec![0]]);
    }

    #[test]
    fn rows_not_covering_the_position_go_last() {
        let mut reads = vec![read(0, 10), read(0, 100), read(0, 100)];
        reads[2].mapq = 70;
        // Rows: [0 (0-10)], [1], [2]; only rows 1 and 2 cover 50.
        let p = pack_with(&reads, &opts(GroupBy::None, Some((50, SortBy::MappingQuality))));
        assert_eq!(p.rows.last().unwrap(), &vec![0]);
        assert_eq!(p.rows[0], vec![2], "higher MAPQ first");
    }

    #[test]
    fn grouping_by_base_at_a_position() {
        let mut reads: Vec<AlignedRead> = (0..6).map(|_| read(0, 100)).collect();
        reads[0].mismatches.push(Mismatch { pos: 50, base: b'T', quality: 30 });
        reads[1].mismatches.push(Mismatch { pos: 50, base: b'C', quality: 30 });
        reads[2].mismatches.push(Mismatch { pos: 50, base: b'T', quality: 30 });
        reads[3].deletions.push(Indel { pos: 48, len: 5 });
        reads[5] = read(60, 160); // doesn't reach 50
        let p = pack_with(&reads, &opts(GroupBy::BaseAt(50), None));
        let labels: Vec<_> = p.groups.iter().map(|g| (g.label.as_str(), g.reads)).collect();
        assert_eq!(labels, vec![("C", 1), ("T", 2), ("deletion", 1), ("reference", 1), ("not covering", 1)]);
    }

    #[test]
    fn grouping_by_base_without_a_reference_uses_the_read_bases() {
        // Loaded with no reference: no mismatches, the bases kept instead.
        let mut reads: Vec<AlignedRead> = (0..5).map(|_| read(0, 100)).collect();
        for (r, b) in reads.iter_mut().zip(*b"ACAN=") {
            r.blocks.push(crate::Block { ref_start: 0, query_start: 0, len: 100 });
            let mut bases = vec![b'G'; 100];
            bases[50] = b;
            r.bases = Some(bases.into());
        }
        let p = pack_with(&reads, &opts(GroupBy::BaseAt(50), None));
        let labels: Vec<_> = p.groups.iter().map(|g| (g.label.as_str(), g.reads)).collect();
        // A genuine N is its own group; `=` (a base the read doesn't store) is unknown.
        assert_eq!(labels, vec![("A", 2), ("C", 1), ("N", 1), ("base unknown", 1)]);
    }

    #[test]
    fn grouping_by_haplotype_bands_rows_with_gaps() {
        let mut reads: Vec<AlignedRead> = (0..5).map(|_| read(0, 100)).collect();
        reads[0].haplotype = Some(2);
        reads[1].haplotype = Some(1);
        reads[3].haplotype = Some(1);
        let p = pack_with(&reads, &opts(GroupBy::Haplotype, None));
        let labels: Vec<_> = p.groups.iter().map(|g| (g.label.as_str(), g.first_row, g.rows)).collect();
        assert_eq!(labels, vec![("HP 1", 0, 2), ("HP 2", 3, 1), ("no HP", 5, 2)]);
        assert!(p.rows[2].is_empty() && p.rows[4].is_empty(), "gap rows between groups");
        assert_eq!(p.rows.iter().flatten().count(), 5);
    }
}

/// Re-packs a loaded view's reads with grouping and sorting (views load ungrouped), keeping
/// the same row budget.
pub fn repack(view: &mut crate::ViewData, max_rows: usize, group: GroupBy, sort: Option<(u64, SortBy)>, link: bool, pairs: bool) {
    let contig_names = Some(view.contig_names.clone());
    if let Some((reads, packed)) = &mut view.reads {
        *packed = pack_with(reads, &PackOptions { max_rows, min_gap: 2, group, sort, link, pairs, clips: false, contig_names });
    }
}

/// Where reads sat in a previous packing, by identity (name, start, flags), so a new packing of
/// an overlapping span can keep them there.
#[derive(Clone, Debug, Default)]
pub struct RowHints(std::collections::HashMap<(u64, u64, u16), usize>);

fn read_key(r: &AlignedRead) -> (u64, u64, u16) {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    r.name.hash(&mut h);
    (h.finish(), r.start, r.flags)
}

/// The rows of `packed` (a packing of `reads`), to keep reads in place in the next one.
pub fn row_hints(reads: &[AlignedRead], packed: &Packed) -> RowHints {
    let mut m = std::collections::HashMap::with_capacity(reads.len());
    for (r, row) in packed.rows.iter().enumerate() {
        for &i in row {
            m.insert(read_key(&reads[i]), r);
        }
    }
    RowHints(m)
}

impl RowHints {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// [`pack_indices`] that keeps reads where `hints` had them: when a newly loaded span overlaps
/// the last (panning), reads already on screen stay in their rows and the new ones fit around
/// them, instead of the whole span repacking from its first read and reads jumping rows. Only
/// for plain packings (grouping, sorting, linking and pairs order rows themselves).
pub fn pack_sticky(reads: &[AlignedRead], mut order: Vec<usize>, opts: &PackOptions, hints: &RowHints) -> Packed {
    if hints.is_empty() || opts.group != GroupBy::None || opts.sort.is_some() || opts.link || opts.pairs {
        return pack_indices(reads, order, opts);
    }
    order.sort_by_key(|&i| extent(&reads[i], opts.clips));
    // Per row: occupied (start, end) in start order, and the reads there.
    let mut occupied: Vec<Vec<(u64, u64)>> = Vec::new();
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let gap = opts.min_gap;
    let fits = |occ: &Vec<(u64, u64)>, s: u64, e: u64| -> (bool, usize) {
        let p = occ.partition_point(|x| x.0 < s);
        let before = p == 0 || occ[p - 1].1 + gap <= s;
        let after = p == occ.len() || e + gap <= occ[p].0;
        (before && after, p)
    };
    let mut rest = Vec::new();
    for &i in &order {
        let r = &reads[i];
        let (start, end) = extent(r, opts.clips);
        match hints.0.get(&read_key(r)).copied().filter(|&row| row < opts.max_rows) {
            Some(row) => {
                if occupied.len() <= row {
                    occupied.resize_with(row + 1, Vec::new);
                    rows.resize_with(row + 1, Vec::new);
                }
                let (ok, p) = fits(&occupied[row], start, end);
                if ok {
                    occupied[row].insert(p, (start, end));
                    rows[row].push(i);
                } else {
                    rest.push(i);
                }
            }
            None => rest.push(i),
        }
    }
    let mut hidden = 0;
    for i in rest {
        let (start, end) = extent(&reads[i], opts.clips);
        let free = (0..occupied.len()).find_map(|row| {
            let (ok, p) = fits(&occupied[row], start, end);
            ok.then_some((row, p))
        });
        match free {
            Some((row, p)) => {
                occupied[row].insert(p, (start, end));
                rows[row].push(i);
            }
            None if occupied.len() < opts.max_rows => {
                occupied.push(vec![(start, end)]);
                rows.push(vec![i]);
            }
            None => hidden += 1,
        }
    }
    for row in &mut rows {
        row.sort_by_key(|&i| (reads[i].start, reads[i].end));
    }
    while rows.last().is_some_and(Vec::is_empty) {
        rows.pop();
    }
    Packed { rows, hidden_reads: hidden, groups: Vec::new() }
}
