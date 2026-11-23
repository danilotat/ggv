//! Where a [`super::VariantFile`]'s records come from: pasta's reader for a local file, or BGZF
//! over byte-range reads ([`crate::remote`]) for a URL, since pasta's reader opens paths only.
//! Index queries run the same way on both (HTSlib's `hts_itr_next`).

use std::io::{self, BufRead, Read};

use pasta::{
    bgzf,
    variant::{
        Header, Record, bcf,
        index::{QueryPlan, tabix::Tabix},
        io::{InputFormat, Query, Reader},
    },
};

use crate::remote::Source;

pub(super) enum Input {
    Local(Reader),
    Remote(Box<RemoteInput>),
}

/// An index query in progress, made by [`Input::scan`] for the same input.
pub(super) enum Scan {
    Local(Query),
    Remote(RemoteQuery),
}

impl Input {
    pub(super) fn scan(&self, plan: QueryPlan) -> Scan {
        match self {
            Self::Local(_) => Scan::Local(Query::new(plan)),
            Self::Remote(_) => Scan::Remote(RemoteQuery::new(plan)),
        }
    }

    /// The next VCF line of a tabix query.
    pub(super) fn next_line(&mut self, q: &mut Scan, tbx: &mut Tabix, line: &mut Vec<u8>) -> io::Result<bool> {
        match (self, q) {
            (Self::Local(r), Scan::Local(q)) => r.query_next_line(q, tbx, line),
            (Self::Remote(r), Scan::Remote(q)) => {
                let meta_char = tbx.conf.meta_char;
                q.next(&mut r.stream, |s| {
                    loop {
                        if !read_line(s, line)? {
                            return Ok(None);
                        }
                        if line.is_empty() || i32::from(line[0]) != meta_char {
                            break;
                        }
                    }
                    let (tid, intv) = tbx.line_interval(line)?;
                    Ok(Some((i64::from(tid), intv.beg, intv.end)))
                })
            }
            _ => Err(mismatched()),
        }
    }

    /// The next BCF record of an index query, checked against `header`.
    pub(super) fn next_bcf(&mut self, q: &mut Scan, header: &Header, rec: &mut Record) -> io::Result<bool> {
        match (self, q) {
            (Self::Local(r), Scan::Local(q)) => r.query_next_bcf(q, header, rec),
            (Self::Remote(r), Scan::Remote(q)) => q.next(&mut r.stream, |s| {
                if bcf::read_record_raw(s, rec)? == bcf::ReadOutcome::Eof {
                    return Ok(None);
                }
                if bcf::check_record(Some(header), rec).is_err() {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "BCF read error"));
                }
                Ok(Some((i64::from(rec.rid()), rec.pos(), rec.pos() + rec.rlen())))
            }),
            _ => Err(mismatched()),
        }
    }
}

fn mismatched() -> io::Error {
    io::Error::other("index query made for another input")
}

type Stream = bgzf::io::Reader<Source>;

/// A bgzipped VCF or a BCF at a URL.
pub(super) struct RemoteInput {
    stream: Stream,
    pub(super) format: InputFormat,
}

impl RemoteInput {
    /// Opens `source` (at its start) and tells VCF from BCF. Only BGZF can be read by index, so
    /// a plain or plain-gzip VCF is refused here rather than failing on its first query.
    pub(super) fn open(mut source: Source) -> io::Result<Self> {
        let mut magic = [0u8; 18];
        let n = read_up_to(&mut source, &mut magic)?;
        let bgzf = n == magic.len() && magic[..2] == [0x1f, 0x8b] && magic[3] & 4 != 0 && magic[12..16] == *b"BC\x02\x00";
        if !bgzf {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not BGZF-compressed: a VCF at a URL must be bgzipped (`bgzip`) and indexed, or a BCF with a CSI index",
            ));
        }
        std::io::Seek::rewind(&mut source)?;
        let mut stream = bgzf::io::Reader::new(source);
        let head = stream.fill_buf()?;
        let format = if head.len() >= 4 && head[3] <= 4 && head.starts_with(b"BCF\x02") {
            InputFormat::Bcf
        } else if head.starts_with(b"##fileformat=VCF") {
            InputFormat::Vcf
        } else {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "neither VCF (no ##fileformat=VCF line) nor BCF"));
        };
        Ok(Self { stream, format })
    }

    /// The header, read from the start of the file. A VCF's gains the contigs `tbx` names that
    /// it doesn't declare, as pasta's reader (and HTSlib) do.
    pub(super) fn read_header(&mut self, tbx: Option<&Tabix>) -> io::Result<Header> {
        let invalid = |e: pasta::variant::header::HeaderError| io::Error::new(io::ErrorKind::InvalidData, e.to_string());
        if self.format == InputFormat::Bcf {
            return bcf::read_header(&mut self.stream);
        }
        let mut txt = Vec::new();
        let mut line = Vec::new();
        let mut sample_line = false;
        while read_line(&mut self.stream, &mut line)? {
            if line.is_empty() {
                continue;
            }
            if line[0] != b'#' {
                break;
            }
            txt.extend_from_slice(&line);
            txt.push(b'\n');
            if line.get(1) != Some(&b'#') {
                sample_line = true;
                break;
            }
        }
        if !sample_line {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "the VCF header has no #CHROM line"));
        }
        let mut header = Header::parse(&txt).map_err(invalid)?;
        for name in tbx.map(Tabix::names).unwrap_or_default() {
            if header.contig_id(name).is_none() {
                let mut rec = pasta::variant::HeaderRecord::structured("contig");
                rec.push_field("ID", name, pasta::variant::Quoting::Bare);
                header.add_record(rec).map_err(invalid)?;
            }
        }
        Ok(header)
    }
}

/// One line without its line ending (`\n`, `\r\n`); `false` at the end.
fn read_line(s: &mut Stream, out: &mut Vec<u8>) -> io::Result<bool> {
    out.clear();
    if s.read_until(b'\n', out)? == 0 {
        return Ok(false);
    }
    if out.last() == Some(&b'\n') {
        out.pop();
    }
    if out.last() == Some(&b'\r') {
        out.pop();
    }
    Ok(true)
}

fn read_up_to<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// The progress of an index query over a [`RemoteInput`]: pasta's `Query` (HTSlib's
/// `hts_itr_t`), whose state pasta keeps private.
pub(super) struct RemoteQuery {
    plan: QueryPlan,
    /// The chunk being read; -1 before the first.
    i: isize,
    curr_off: u64,
    finished: bool,
}

impl RemoteQuery {
    fn new(plan: QueryPlan) -> Self {
        let finished = plan.chunks.is_empty();
        Self { plan, i: -1, curr_off: 0, finished }
    }

    /// Reads records with `read` (each giving its target and 0-based span) until one overlaps
    /// the region, seeking from chunk to chunk; `false` once past it.
    fn next<F>(&mut self, s: &mut Stream, mut read: F) -> io::Result<bool>
    where
        F: FnMut(&mut Stream) -> io::Result<Option<(i64, i64, i64)>>,
    {
        if self.finished {
            return Ok(false);
        }
        let chunks = &self.plan.chunks;
        loop {
            if self.curr_off == 0 || (self.i >= 0 && self.curr_off >= chunks[self.i as usize].1) {
                if self.i == chunks.len() as isize - 1 {
                    break;
                }
                let next = chunks[(self.i + 1) as usize];
                if self.i < 0 || chunks[self.i as usize].1 != next.0 {
                    s.seek(bgzf::VirtualPosition::from(next.0)).inspect_err(|_| self.finished = true)?;
                    self.curr_off = u64::from(s.virtual_position());
                }
                self.i += 1;
            }
            match read(s) {
                Ok(Some((tid, beg, end))) => {
                    self.curr_off = u64::from(s.virtual_position());
                    if self.plan.is_past(tid, beg) {
                        break;
                    }
                    if self.plan.overlaps(beg, end) {
                        return Ok(true);
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    self.finished = true;
                    return Err(e);
                }
            }
        }
        self.finished = true;
        Ok(false)
    }
}
