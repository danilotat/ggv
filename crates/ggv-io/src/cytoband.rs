//! Chromosome bands (UCSC `cytoBand.txt`: contig, start, end, name, stain; 0-based, end
//! exclusive), for drawing a chromosome ideogram. Read from local files only: one the person
//! opens, or one kept beside the reference (`cytoBand.txt`, `hg38.cytoBand.txt`, optionally
//! gzipped). Nothing is downloaded.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// A band's Giemsa stain: how dark it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stain {
    /// Unstained (light).
    Neg,
    /// Stained, 25 to 100 (`gpos50`; a bare `gpos` is 100).
    Pos(u8),
    /// Centromere (`acen`).
    Centromere,
    /// Variable heterochromatin (`gvar`).
    Variable,
    /// Acrocentric stalk (`stalk`).
    Stalk,
    Other,
}

impl Stain {
    fn parse(s: &str) -> Self {
        match s {
            "gneg" => Self::Neg,
            "acen" => Self::Centromere,
            "gvar" => Self::Variable,
            "stalk" => Self::Stalk,
            _ => match s.strip_prefix("gpos") {
                Some("") => Self::Pos(100),
                Some(n) => n.parse().map_or(Self::Other, |n: u8| Self::Pos(n.min(100))),
                None => Self::Other,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Band {
    pub start: u64,
    pub end: u64,
    /// `p36.33`; may be empty (UCSC leaves unplaced contigs unnamed).
    pub name: String,
    pub stain: Stain,
}

/// Every contig's bands, each sorted by start.
#[derive(Debug, Default)]
pub struct Cytobands {
    by_contig: HashMap<String, Vec<Band>>,
}

impl Cytobands {
    /// Parses `cytoBand.txt` lines; `#` comments and blank lines are skipped. A line that isn't
    /// a band is an error naming it (a file that isn't a cytoband table at all fails early).
    pub fn parse(reader: impl BufRead) -> std::io::Result<Self> {
        let mut by_contig: HashMap<String, Vec<Band>> = HashMap::new();
        for (n, line) in reader.lines().enumerate() {
            let line = line?;
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, format!("line {}: not a cytoband line (contig, start, end, name, stain)", n + 1));
            if f.len() < 4 {
                return Err(bad());
            }
            let (Ok(start), Ok(end)) = (f[1].parse::<u64>(), f[2].parse::<u64>()) else { return Err(bad()) };
            if end < start {
                return Err(bad());
            }
            let stain = f.get(4).map_or(Stain::Other, |s| Stain::parse(s.trim()));
            by_contig.entry(f[0].to_owned()).or_default().push(Band { start, end, name: f[3].to_owned(), stain });
        }
        if by_contig.is_empty() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "no bands in the file"));
        }
        for bands in by_contig.values_mut() {
            bands.sort_by_key(|b| (b.start, b.end));
        }
        Ok(Self { by_contig })
    }

    /// Reads a file, gunzipping it when it is gzipped (by its magic bytes, not its name).
    pub fn read(path: &Path) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        if bytes.starts_with(&[0x1f, 0x8b]) {
            Self::parse(BufReader::new(flate2::read::MultiGzDecoder::new(&bytes[..])))
        } else {
            Self::parse(&bytes[..])
        }
    }

    /// The bands of `contig`, also under its other spelling (`chr7` ⇄ `7`, `chrM` ⇄ `MT`).
    pub fn bands(&self, contig: &str) -> Option<&[Band]> {
        if let Some(b) = self.by_contig.get(contig) {
            return Some(b);
        }
        ggv_model::contig_aliases(contig).iter().find_map(|a| self.by_contig.get(a)).map(Vec::as_slice)
    }
}

/// Whether a file name looks like a cytoband table: `cytoBand.txt`, `hg38.cytoBand.txt.gz`,
/// `cytoBandIdeo.txt` (case-insensitive).
pub fn is_cytoband_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let base = lower.rsplit(['/', '\\']).next().unwrap_or(&lower);
    let base = base.strip_suffix(".gz").unwrap_or(base);
    base.contains("cytoband") && (base.ends_with(".txt") || base.ends_with(".tsv"))
}

/// A cytoband file kept in the same folder as a local reference: `cytoBand.txt` or
/// `<name>.cytoBand.txt` (`hg38.2bit` → `hg38.cytoBand.txt`), optionally gzipped, any case.
/// `None` for a remote reference or when there is none.
pub fn beside(reference: &Path) -> Option<PathBuf> {
    if crate::remote::is_remote(reference) {
        return None;
    }
    let dir = reference.parent()?;
    let file = reference.file_name()?.to_string_lossy().to_ascii_lowercase();
    let stem = [".gz", ".bgz"].iter().fold(file.as_str(), |s, e| s.strip_suffix(e).unwrap_or(s));
    let stem = [".fa", ".fasta", ".fna", ".2bit"].iter().find_map(|e| stem.strip_suffix(e)).unwrap_or(stem);
    let wanted: Vec<String> = ["cytoband.txt".to_owned(), format!("{stem}.cytoband.txt")].into_iter().flat_map(|n| [format!("{n}.gz"), n]).collect();
    let mut found: Vec<(usize, PathBuf)> = std::fs::read_dir(if dir.as_os_str().is_empty() { Path::new(".") } else { dir })
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            // The reference's own name first, then the plain one; plain text before gzipped.
            let rank = wanted.iter().rev().position(|w| *w == name)?;
            Some((rank, e.path()))
        })
        .collect();
    found.sort_by_key(|(rank, _)| *rank);
    found.into_iter().next().map(|(_, p)| p).filter(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HG38_CHR1: &str = "#chrom\tchromStart\tchromEnd\tname\tgieStain\n\
        chr1\t0\t2300000\tp36.33\tgneg\n\
        chr1\t2300000\t5300000\tp36.32\tgpos25\n\
        chr1\t121700000\t123400000\tp11.1\tacen\n\
        chr1\t123400000\t125100000\tq11\tacen\n\
        chr1\t125100000\t143200000\tq12\tgvar\n\
        chr1\t143200000\t147500000\tq21.1\tgpos\n\
        chr1_KI270706v1_random\t0\t175055\t\tgneg\n";

    #[test]
    fn parses_ucsc_cytobands() {
        let c = Cytobands::parse(HG38_CHR1.as_bytes()).unwrap();
        let b = c.bands("chr1").unwrap();
        assert_eq!(b.len(), 6);
        assert_eq!(b[0], Band { start: 0, end: 2_300_000, name: "p36.33".into(), stain: Stain::Neg });
        assert_eq!(b.iter().map(|b| b.stain).collect::<Vec<_>>()[1..], [Stain::Pos(25), Stain::Centromere, Stain::Centromere, Stain::Variable, Stain::Pos(100)]);
        assert_eq!(c.bands("chr1_KI270706v1_random").unwrap()[0].name, "");
    }

    #[test]
    fn finds_bands_under_either_contig_spelling() {
        let c = Cytobands::parse(HG38_CHR1.as_bytes()).unwrap();
        assert_eq!(c.bands("1").map(<[Band]>::len), Some(6));
        assert!(c.bands("chr2").is_none());
    }

    #[test]
    fn rejects_files_that_arent_cytobands() {
        assert!(Cytobands::parse("chr1\tfoo\tbar\tbaz\n".as_bytes()).is_err());
        assert!(Cytobands::parse("##fileformat=VCFv4.2\n".as_bytes()).is_err(), "only comments: no bands");
        assert!(Cytobands::parse("chr1\t10\n".as_bytes()).is_err());
    }

    #[test]
    fn reads_gzipped_files() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("ggv-cyto-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cytoBand.txt.gz");
        let mut gz = flate2::write::GzEncoder::new(std::fs::File::create(&path).unwrap(), flate2::Compression::default());
        gz.write_all(HG38_CHR1.as_bytes()).unwrap();
        gz.finish().unwrap();
        assert_eq!(Cytobands::read(&path).unwrap().bands("chr1").map(<[Band]>::len), Some(6));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cytoband_names() {
        assert!(is_cytoband_name("cytoBand.txt"));
        assert!(is_cytoband_name("/data/hg38.cytoBand.txt.gz"));
        assert!(is_cytoband_name("cytoBandIdeo.txt"));
        assert!(!is_cytoband_name("cytoBand.bed"));
        assert!(!is_cytoband_name("genes.txt"));
    }

    #[test]
    fn finds_a_cytoband_file_beside_the_reference() {
        let dir = std::env::temp_dir().join(format!("ggv-cyto-beside-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let reference = dir.join("hg38.2bit");
        assert_eq!(beside(&reference), None);
        std::fs::write(dir.join("cytoBand.txt"), HG38_CHR1).unwrap();
        assert_eq!(beside(&reference), Some(dir.join("cytoBand.txt")));
        // The one named for the reference wins over the plain one.
        std::fs::write(dir.join("hg38.cytoBand.txt.gz"), b"").unwrap();
        assert_eq!(beside(&reference), Some(dir.join("hg38.cytoBand.txt.gz")));
        assert_eq!(beside(&dir.join("hg38.fa.gz")), Some(dir.join("hg38.cytoBand.txt.gz")));
        assert_eq!(beside(&dir.join("other.fa")), Some(dir.join("cytoBand.txt")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
