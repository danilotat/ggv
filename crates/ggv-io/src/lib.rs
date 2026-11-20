//! File access for GGV, built on [pasta](https://github.com/DylanCosto/pasta).
//!
//! Indexed BAM/CRAM, FASTA/2bit, VCF/BCF and signal tracks. BAM, CRAM and FASTA can also be
//! read from `http(s)://`, `s3://` and `gs://` URLs by byte range ([`remote`]), behind the same
//! types, so callers don't change.

pub mod assembly;
pub mod attach;
mod bam_file;
mod error;
pub mod catalog;
pub mod cytoband;
pub mod demo;
pub mod folders;
pub mod genomes;
pub mod igv_link;
mod indexing;
pub mod places;
pub mod remote;
mod reference;
pub mod signal;
pub mod twobit;
mod variants;

pub use bam_file::{BamFile, BamInfo, ReadGroup};
pub use error::{Error, Result};
pub use indexing::{build_index, index_kind};
pub use reference::ReferenceFasta;
pub use variants::{GenotypeClass, InfoField, SampleCall, Variant, VariantFile, breakend_mate, is_breakend};

/// Re-exported so downstream crates use the exact pasta version ggv-io was built against.
pub use pasta;

use ggv_model::{Contig, Locus, Region, contig_aliases};

/// Where alignment files' indexes are when they aren't next to them, set by the front ends
/// (`--index`, the MCP `index` argument, the app's Open URL dialog).
static INDEX_FOR: std::sync::Mutex<Vec<(std::path::PathBuf, std::path::PathBuf)>> = std::sync::Mutex::new(Vec::new());

/// Opens `data`'s index from `index` from now on (in this process), wherever it is opened.
pub fn set_index_for(data: &std::path::Path, index: &std::path::Path) {
    let mut map = INDEX_FOR.lock().unwrap_or_else(|e| e.into_inner());
    map.retain(|(d, _)| d != data);
    map.push((data.to_owned(), index.to_owned()));
}

/// The index set for `data` with [`set_index_for`].
pub fn index_for(data: &std::path::Path) -> Option<std::path::PathBuf> {
    INDEX_FOR.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(d, _)| d == data).map(|(_, i)| i.clone())
}

/// Parses and resolves locus text against a list of contigs. A string that is exactly a contig
/// name wins over reading its tail as coordinates (`HLA-A*01:01:01:01` is a contig, not
/// position 1 of `HLA-A*01:01:01`).
pub fn resolve_locus_str(contigs: &[Contig], text: &str) -> Result<Region> {
    let text = text.trim();
    let locus = if contigs.iter().any(|c| c.name == text) {
        Locus::whole(text)
    } else {
        text.parse().map_err(|cause| Error::InvalidLocus { text: text.to_owned(), cause })?
    };
    resolve_locus(contigs, &locus)
}

/// Resolves a user-written locus against a list of contigs: finds the contig (trying `chr`
/// aliases), fills in open ends, and clamps the end to the contig length.
pub fn resolve_locus(contigs: &[Contig], locus: &Locus) -> Result<Region> {
    let contig = contigs
        .iter()
        .find(|c| c.name == locus.contig)
        .or_else(|| {
            contig_aliases(&locus.contig)
                .into_iter()
                .find_map(|alias| contigs.iter().find(|c| c.name == alias))
        })
        .ok_or_else(|| Error::UnknownContig {
            name: locus.contig.clone(),
            examples: contigs.iter().take(5).map(|c| c.name.clone()).collect(),
            file: None,
        })?;

    let start = locus.start.map_or(0, |s| s - 1);
    let end = locus.end.map_or(contig.length, |e| e.min(contig.length));

    if start >= contig.length {
        return Err(Error::OutOfBounds {
            locus: locus.to_string(),
            contig: contig.name.clone(),
            length: contig.length,
        });
    }

    Ok(Region { contig: contig.name.clone(), start, end })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contigs() -> Vec<Contig> {
        vec![
            Contig { name: "chr1".into(), length: 1000 },
            Contig { name: "chrM".into(), length: 16569 },
        ]
    }

    #[test]
    fn resolves_aliases_and_clamps() {
        let r = resolve_locus(&contigs(), &"1:990-2000".parse().unwrap()).unwrap();
        assert_eq!(r, Region { contig: "chr1".into(), start: 989, end: 1000 });

        let r = resolve_locus(&contigs(), &"MT".parse().unwrap()).unwrap();
        assert_eq!(r, Region { contig: "chrM".into(), start: 0, end: 16569 });
    }

    #[test]
    fn whole_string_contig_wins() {
        let mut cs = contigs();
        cs.push(Contig { name: "HLA-A*01:01:01:01".into(), length: 3503 });
        let r = resolve_locus_str(&cs, "HLA-A*01:01:01:01").unwrap();
        assert_eq!(r, Region { contig: "HLA-A*01:01:01:01".into(), start: 0, end: 3503 });
        let r = resolve_locus_str(&cs, "HLA-A*01:01:01:01:10-20").unwrap();
        assert_eq!(r, Region { contig: "HLA-A*01:01:01:01".into(), start: 9, end: 20 });
        let r = resolve_locus_str(&cs, "chr1:1,000").unwrap();
        assert_eq!(r, Region { contig: "chr1".into(), start: 999, end: 1000 });
    }

    #[test]
    fn unknown_and_out_of_bounds() {
        assert!(matches!(
            resolve_locus(&contigs(), &"chr2".parse().unwrap()),
            Err(Error::UnknownContig { .. })
        ));
        assert!(matches!(
            resolve_locus(&contigs(), &"chr1:1001".parse().unwrap()),
            Err(Error::OutOfBounds { .. })
        ));
    }
}
