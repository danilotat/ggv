//! A file read over HTTP gives exactly what the same file gives from disk: the hg002 fixture
//! served by a local range server (no internet needed), as BAM, FASTA and a CRAM made from
//! the BAM here.

use std::path::{Path, PathBuf};

use ggv_io::{BamFile, ReferenceFasta, remote};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/hg002-revio")
}

const REGIONS: [&str; 3] = ["chr20:58885000-58886000", "chr20:31150000-31200000", "chr20:58800000-58950000"];

/// Each record overlapping each region, as (name, flags, position, cigar length).
fn records(bam: &mut BamFile) -> Vec<(String, u16, Option<usize>, usize)> {
    let mut out = Vec::new();
    for r in REGIONS {
        let region = bam.resolve_str(r).unwrap();
        bam.for_each_record(&region, |rec| {
            let name = rec.name().map(|n| String::from_utf8_lossy(n.as_ref()).into_owned()).unwrap_or_default();
            let pos = rec.alignment_start().transpose()?.map(usize::from);
            out.push((name, rec.flags().bits(), pos, rec.cigar().len()));
            Ok(())
        })
        .unwrap();
    }
    out
}

/// A copy of the fixture's BAM as CRAM (with its `.crai`), in `dir`.
fn write_cram(dir: &Path) -> PathBuf {
    use ggv_io::pasta::{cram, fasta, sam::alignment::io::Write as _};
    let src = fixture();
    let mut bam = BamFile::open(src.join("HG002.revio.slices.bam")).unwrap();
    let header = bam.header().clone();
    let fa = fasta::io::indexed_reader::Builder::default().build_from_path(src.join("GRCh38.chr20.slices.fa.gz")).unwrap();
    let repo = fasta::Repository::new(fasta::repository::adapters::IndexedReader::new(fa));
    let dst = dir.join("HG002.revio.slices.cram");
    let mut writer = cram::io::writer::Builder::default().set_reference_sequence_repository(repo).build_from_path(&dst).unwrap();
    writer.write_header(&header).unwrap();
    let region = bam.resolve_str("chr20").unwrap();
    let mut recs = Vec::new();
    bam.for_each_record(&region, |r| {
        recs.push(r.clone());
        Ok(())
    })
    .unwrap();
    for r in &recs {
        writer.write_alignment_record(&header, r).unwrap();
    }
    writer.try_finish(&header).unwrap();
    drop(writer);
    ggv_io::build_index(&dst).unwrap();
    dst
}

#[test]
fn remote_files_read_the_same_as_local() {
    let dir = std::env::temp_dir().join(format!("ggv-remote-parity-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["HG002.revio.slices.bam", "HG002.revio.slices.bam.bai", "GRCh38.chr20.slices.fa.gz", "GRCh38.chr20.slices.fa.gz.fai", "GRCh38.chr20.slices.fa.gz.gzi"] {
        std::fs::copy(fixture().join(name), dir.join(name)).unwrap();
    }
    let cram = write_cram(&dir);
    let (base, _) = remote::test_server::serve(dir.clone());
    let url = |name: &str| PathBuf::from(format!("{base}/{name}"));

    // BAM: the same records, header and index-based density.
    let mut local = BamFile::open(dir.join("HG002.revio.slices.bam")).unwrap();
    let mut far = BamFile::open(url("HG002.revio.slices.bam")).unwrap();
    assert!(far.warning().is_none());
    let expected = records(&mut local);
    assert!(expected.len() > 400, "{}", expected.len());
    assert_eq!(records(&mut far), expected);
    assert_eq!(far.contigs(), local.contigs());
    assert_eq!(far.index_density("chr20"), local.index_density("chr20"));
    assert_eq!(records(&mut far.reopen().unwrap()), expected);
    assert_eq!(far.info().index, format!("{base}/HG002.revio.slices.bam.bai"));

    // A query string (as on a presigned URL) is kept for the request, dropped when shown, and
    // the index is looked for before it.
    let signed = PathBuf::from(format!("{base}/HG002.revio.slices.bam?X-Amz-Signature=secret"));
    let mut b = BamFile::open(&signed).unwrap();
    assert_eq!(records(&mut b), expected);
    assert!(!b.info().path.contains("secret") && !b.info().index.contains("secret"));

    // An explicit index.
    let mut b = BamFile::open_with_index(url("HG002.revio.slices.bam"), Some(dir.join("HG002.revio.slices.bam.bai")), None::<&Path>).unwrap();
    assert_eq!(records(&mut b), expected);

    // A missing file and a missing index are told apart, and the error names the URL.
    let e = BamFile::open(url("nope.bam")).err().unwrap().to_string();
    assert!(e.contains("nope.bam") && e.contains("not found"), "{e}");
    std::fs::copy(dir.join("HG002.revio.slices.bam"), dir.join("noindex.bam")).unwrap();
    let e = BamFile::open(url("noindex.bam")).err().unwrap();
    assert!(matches!(e, ggv_io::Error::MissingIndex { .. }), "{e}");
    assert!(e.to_string().contains("--index"), "{e}");

    // FASTA (bgzipped, with .fai and .gzi fetched remotely).
    let mut fa_local = ReferenceFasta::open(dir.join("GRCh38.chr20.slices.fa.gz")).unwrap();
    let mut fa_far = ReferenceFasta::open(url("GRCh38.chr20.slices.fa.gz")).unwrap();
    for r in REGIONS {
        let region = local.resolve_str(r).unwrap();
        assert_eq!(fa_far.fetch(&region).unwrap(), fa_local.fetch(&region).unwrap());
    }

    // CRAM with a remote reference: the same records as the BAM.
    let mut c_local = BamFile::open_with_reference(&cram, Some(dir.join("GRCh38.chr20.slices.fa.gz"))).unwrap();
    let mut c_far = BamFile::open_with_reference(url("HG002.revio.slices.cram"), Some(url("GRCh38.chr20.slices.fa.gz"))).unwrap();
    let from_cram = records(&mut c_local);
    assert_eq!(from_cram.len(), expected.len());
    assert_eq!(records(&mut c_far), from_cram);
    assert_eq!(records(&mut c_far.reopen().unwrap()), from_cram);
    let info = BamFile::header_info(url("HG002.revio.slices.cram")).unwrap();
    assert_eq!(info.contigs, local.contigs());

    let _ = std::fs::remove_dir_all(&dir);
}
