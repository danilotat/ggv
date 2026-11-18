//! IGV's "batch port" links: web pages open data in a running IGV with
//! `http://localhost:60151/load?file=<url>&index=<url>&locus=<locus>&merge=true`. GGV answers
//! the same links; this module turns a request target (`/load?...`) into what to do. It does no
//! I/O, so the desktop app's listener and its tests share it.
//!
//! Supported: `/load` (`file`, `index`, `locus`, `genome`, `merge`; `file` and `index` may be
//! comma-separated lists, matched in order), `/goto` (`locus`) and `/echo`. `name` and `format`
//! are accepted and ignored. Values are percent-decoded, `+` as a space (as IGV does).

use std::path::Path;

pub const DEFAULT_PORT: u16 = 60151;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Open `files`, each with its index when one was given.
    Load {
        files: Vec<(String, Option<String>)>,
        /// Keep what is open (true, the default) or replace it.
        merge: bool,
        locus: Option<String>,
        /// A built-in genome id or alias (`hg38`), or the URL of a reference FASTA/2bit.
        genome: Option<String>,
    },
    Goto {
        locus: String,
    },
    /// "Is anything listening?": answered without touching the window.
    Echo,
}

/// Parses a request target such as `/load?file=...&index=...`.
pub fn parse(target: &str) -> Result<Request, String> {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let params = query_pairs(query);
    let get = |key: &str| params.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.trim()).filter(|v| !v.is_empty());
    let command = path.trim_matches('/').to_ascii_lowercase();
    match command.as_str() {
        "echo" => Ok(Request::Echo),
        "goto" => {
            let locus = get("locus").ok_or("give the locus: /goto?locus=chr1:100-200")?;
            Ok(Request::Goto { locus: locus.to_owned() })
        }
        // IGV also takes the parameters on the root path.
        "load" | "" => {
            let list = |v: Option<&str>| v.map(|v| v.split(',').map(str::trim).map(str::to_owned).collect::<Vec<_>>()).unwrap_or_default();
            let urls = list(get("file").or_else(|| get("url")));
            let indexes = list(get("index"));
            let locus = get("locus").map(str::to_owned);
            let genome = get("genome").map(str::to_owned);
            if urls.iter().any(String::is_empty) {
                return Err("`file` has an empty entry".into());
            }
            if urls.is_empty() && genome.is_none() {
                return match locus {
                    Some(locus) if command.is_empty() => Ok(Request::Goto { locus }),
                    _ => Err("give the file to open: /load?file=<url>".into()),
                };
            }
            if !indexes.is_empty() && indexes.len() != urls.len() {
                return Err(format!("{} files but {} indexes: give one index per file, in the same order (or none)", urls.len(), indexes.len()));
            }
            let files = urls.into_iter().enumerate().map(|(i, f)| (f, indexes.get(i).filter(|x| !x.is_empty()).cloned())).collect();
            let merge = !get("merge").is_some_and(|m| m.eq_ignore_ascii_case("false"));
            Ok(Request::Load { files, merge, locus, genome })
        }
        other => Err(format!("unknown command `{other}`: GGV answers /load, /goto and /echo")),
    }
}

impl Request {
    /// What a web page may ask for: data at public URLs only. A local path would let any page
    /// open files from this computer, and a private address would let it make GGV fetch from
    /// the local network; both are refused. A genome may also be a built-in name.
    pub fn check(&self) -> Result<(), String> {
        let Request::Load { files, genome, .. } = self else { return Ok(()) };
        let urls = files.iter().flat_map(|(f, i)| std::iter::once(f).chain(i));
        let genome_url = genome.as_ref().filter(|g| crate::genomes::find(g).is_none());
        for u in urls.chain(genome_url) {
            if !crate::remote::is_remote(Path::new(u)) {
                return Err(format!("{u}: links open files at http(s)://, s3:// or gs:// addresses only, not files on this computer (use File > Open for those)"));
            }
            crate::remote::check_public(u).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

/// `a=1&b=2` as decoded pairs; empty pieces (`&&`, a trailing `&`) are skipped.
fn query_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (decode(k), decode(v))
        })
        .collect()
}

/// Percent-decoding, `+` as a space; a `%` not followed by two hex digits is kept as is.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match (hex(b[i + 1]), hex(b[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push(h << 4 | l);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_42basepairs_link() {
        let r = parse("/load?merge=true&file=https://42basepairs.com/download/r2/genomics-data/alignments_HG002.bam&index=https://42basepairs.com/download/r2/genomics-data/alignments_HG002.bam.bai&").unwrap();
        assert_eq!(
            r,
            Request::Load {
                files: vec![(
                    "https://42basepairs.com/download/r2/genomics-data/alignments_HG002.bam".into(),
                    Some("https://42basepairs.com/download/r2/genomics-data/alignments_HG002.bam.bai".into())
                )],
                merge: true,
                locus: None,
                genome: None,
            }
        );
    }

    #[test]
    fn encoded_lists_locus_genome_and_merge() {
        let r = parse("/load?file=https%3A%2F%2Fx.org%2Fa.bam%2Chttps%3A%2F%2Fx.org%2Fb.cram&index=https://x.org/a.bai,https://x.org/b.crai&locus=chr7%3A55%2C019%2C017-55%2C211%2C628&genome=hg38&merge=FALSE&name=two+samples&format=bam").unwrap();
        let Request::Load { files, merge, locus, genome } = r else { panic!() };
        assert_eq!(files, vec![("https://x.org/a.bam".into(), Some("https://x.org/a.bai".into())), ("https://x.org/b.cram".into(), Some("https://x.org/b.crai".into()))]);
        assert!(!merge);
        assert_eq!(locus.as_deref(), Some("chr7:55,019,017-55,211,628"));
        assert_eq!(genome.as_deref(), Some("hg38"));
        // Merge unless told not to; no index is fine.
        let Request::Load { files, merge, .. } = parse("/load?file=https://x.org/a.bam").unwrap() else { panic!() };
        assert!(merge);
        assert_eq!(files[0].1, None);
    }

    #[test]
    fn goto_echo_and_bad_requests() {
        assert_eq!(parse("/goto?locus=BRCA1").unwrap(), Request::Goto { locus: "BRCA1".into() });
        assert_eq!(parse("/?locus=chr1:100").unwrap(), Request::Goto { locus: "chr1:100".into() });
        assert_eq!(parse("/echo").unwrap(), Request::Echo);
        assert!(parse("/goto").is_err());
        assert!(parse("/load").is_err());
        assert!(parse("/load?file=a.bam,b.bam&index=a.bai").unwrap_err().contains("2 files but 1 indexes"));
        assert!(parse("/snapshot?file=x.png").unwrap_err().contains("unknown command"));
    }

    #[test]
    fn decoding() {
        assert_eq!(decode("a%20b+c%2Cd"), "a b c,d");
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("%zz%4"), "%zz%4");
        assert_eq!(decode("%C3%A9"), "é");
    }

    #[test]
    fn pages_may_open_public_urls_only() {
        let load = |f: &str| parse(&format!("/load?file={f}")).unwrap();
        assert!(load("/home/me/secret.bam").check().unwrap_err().contains("not files on this computer"));
        assert!(load("file:///etc/passwd.bam").check().is_err());
        assert!(load("http://127.0.0.1:8080/a.bam").check().unwrap_err().contains("private or local"));
        assert!(load("http://192.168.1.10/a.bam").check().is_err());
        assert!(load("http://[::1]/a.bam").check().is_err());
        assert!(parse("/load?file=https://1.1.1.1/a.bam&index=http://10.0.0.1/a.bai").unwrap().check().is_err(), "the index too");
        assert!(load("https://1.1.1.1/a.bam").check().is_ok());
        assert!(parse("/load?genome=hg38").unwrap().check().is_ok(), "a built-in genome");
        assert!(parse("/load?genome=/tmp/ref.fa").unwrap().check().is_err());
        assert!(Request::Goto { locus: "x".into() }.check().is_ok());
    }
}
