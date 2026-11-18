//! Files read over HTTP(S) by byte range, so an indexed BAM or CRAM in a bucket can be viewed
//! without downloading it (as IGV does). Only the bytes a query needs are fetched.
//!
//! A location is remote when its text starts with `http://`, `https://`, `s3://` or `gs://`.
//! It stays in a `Path`/`PathBuf` like a local file, so the front ends need no separate type.
//! `s3://bucket/key` becomes the bucket's public HTTPS address (`AWS_REGION` picks the regional
//! endpoint, `AWS_ENDPOINT_URL` an S3-compatible server). `gs://bucket/key` becomes
//! `https://storage.googleapis.com/bucket/key`. Presigned URLs work as given. Private buckets
//! with signed requests are not supported yet.
//!
//! [`RemoteFile`] reads in aligned blocks. Every handle on one URL (the loader reopens a file
//! for each thread) shares one block cache, capped at `GGV_REMOTE_CACHE_MB` (default 64) per
//! file, and the cache is kept only in memory. Indexes are fetched whole and kept on disk
//! under the cache folder, keyed by URL and ETag, so opening the same file again is fast.
//! Every request after the first carries `If-Match` with the ETag seen on open, so a file
//! replaced on the server while open is an error instead of a mix of two versions.

use std::{
    collections::HashMap,
    io::{self, Read, Seek, SeekFrom},
    net::{IpAddr, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// Bytes per cached block (and the smallest request).
const BLOCK: u64 = 256 * 1024;
/// Blocks fetched in one request when reads run forward.
const READ_AHEAD: u64 = 4;
const RETRIES: u32 = 3;
const MAX_REDIRECTS: u32 = 5;

/// Whether `path` names a remote file (an `http(s)://`, `s3://` or `gs://` URL).
pub fn is_remote(path: &Path) -> bool {
    path.to_str().is_some_and(|s| scheme_len(s).is_some())
}

/// The length of a supported URL scheme prefix (`https://` is 8).
fn scheme_len(s: &str) -> Option<usize> {
    ["https://", "http://", "s3://", "gs://"]
        .iter()
        .find(|p| s.len() > p.len() && s[..p.len()].eq_ignore_ascii_case(p))
        .map(|p| p.len())
}

/// `url` without its query string or fragment, which can hold a presigned URL's signature:
/// what errors, logs and tool results show.
pub fn redact(url: &str) -> &str {
    url.find(['?', '#']).map_or(url, |i| &url[..i])
}

/// A path for showing to people: the URL without its query for a remote file.
pub fn display(path: &Path) -> String {
    match path.to_str() {
        Some(s) if scheme_len(s).is_some() => redact(s).to_owned(),
        _ => path.display().to_string(),
    }
}

/// The file's name: the last part of a URL's path (without the query), or a local file name.
pub fn file_name(path: &Path) -> String {
    match path.to_str() {
        Some(s) if scheme_len(s).is_some() => {
            let base = redact(s).trim_end_matches('/');
            base.rsplit('/').next().unwrap_or(base).to_owned()
        }
        _ => path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned()),
    }
}

/// `path` with `.ext` added to its name: `x.bam` -> `x.bam.bai`. For a URL the extension goes
/// before the query string, so a presigned URL keeps its signature (which is only valid for
/// the file it was made for, but some portals sign a folder).
pub fn with_added_extension(path: &Path, ext: &str) -> PathBuf {
    match path.to_str() {
        Some(s) if scheme_len(s).is_some() => {
            let (base, rest) = s.split_at(s.find(['?', '#']).unwrap_or(s.len()));
            PathBuf::from(format!("{base}.{ext}{rest}"))
        }
        _ => path.with_added_extension(ext),
    }
}

/// `path` with its last extension replaced: `x.cram` -> `x.crai`.
pub fn with_extension(path: &Path, ext: &str) -> PathBuf {
    match path.to_str() {
        Some(s) if scheme_len(s).is_some() => {
            let (base, rest) = s.split_at(s.find(['?', '#']).unwrap_or(s.len()));
            let name_start = base.rfind('/').map_or(0, |i| i + 1);
            let stem_end = base[name_start..].rfind('.').filter(|&i| i > 0).map_or(base.len(), |i| name_start + i);
            PathBuf::from(format!("{}.{ext}{rest}", &base[..stem_end]))
        }
        _ => path.with_extension(ext),
    }
}

/// The HTTP(S) URL a remote location is fetched from.
pub fn http_url(location: &str) -> io::Result<String> {
    let lower = location.get(..5).unwrap_or("").to_ascii_lowercase();
    if lower.starts_with("http") {
        return Ok(location.to_owned());
    }
    let n = scheme_len(location).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("not a URL: {location}")))?;
    let rest = &location[n..];
    let (bucket, key) = rest.split_once('/').filter(|(b, k)| !b.is_empty() && !k.is_empty()).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, format!("{}: expected {}bucket/key", redact(location), &location[..n]))
    })?;
    let key = encode_key(key);
    if lower.starts_with("gs") {
        return Ok(format!("https://storage.googleapis.com/{bucket}/{key}"));
    }
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    if let Some(endpoint) = env("AWS_ENDPOINT_URL_S3").or_else(|| env("AWS_ENDPOINT_URL")) {
        return Ok(format!("{}/{bucket}/{key}", endpoint.trim_end_matches('/')));
    }
    let region = env("AWS_REGION").or_else(|| env("AWS_DEFAULT_REGION"));
    // A bucket name with dots doesn't match the TLS certificate as a host name: path style.
    Ok(match (bucket.contains('.'), region) {
        (true, Some(r)) => format!("https://s3.{r}.amazonaws.com/{bucket}/{key}"),
        (true, None) => format!("https://s3.amazonaws.com/{bucket}/{key}"),
        (false, Some(r)) => format!("https://{bucket}.s3.{r}.amazonaws.com/{key}"),
        (false, None) => format!("https://{bucket}.s3.amazonaws.com/{key}"),
    })
}

/// Percent-encodes an object key for a URL path (`/` kept).
fn encode_key(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for b in key.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

static ALLOW_PRIVATE: AtomicBool = AtomicBool::new(true);

/// Whether URLs may point at loopback, private or link-local addresses. On by default; the
/// MCP server turns it off (unless asked) so an agent can't make GGV read internal services
/// such as a cloud metadata endpoint.
pub fn set_allow_private_hosts(allow: bool) {
    ALLOW_PRIVATE.store(allow, Ordering::Relaxed);
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast()
                // 100.64.0.0/10, carrier-grade NAT
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64)
        }
        IpAddr::V6(v6) => {
            let seg = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || (seg[0] & 0xfe00) == 0xfc00 // unique local
                || (seg[0] & 0xffc0) == 0xfe80 // link local
                || v6.to_ipv4_mapped().is_some_and(|v4| is_private(IpAddr::V4(v4)))
        }
    }
}

/// Refuses a URL whose host resolves to a private address, when those are not allowed.
fn check_host(url: &str) -> io::Result<()> {
    check_host_with(url, ALLOW_PRIVATE.load(Ordering::Relaxed))
}

fn check_host_with(url: &str, allow_private: bool) -> io::Result<()> {
    if allow_private {
        return Ok(());
    }
    refuse_private(url, "start the MCP server with --allow-private-urls to allow them")
}

/// Refuses a URL whose host resolves to a loopback, private or link-local address, whatever
/// the process-wide setting: for requests from web pages (IGV links), which must not make GGV
/// read from this computer or its network.
pub fn check_public(url: &str) -> io::Result<()> {
    refuse_private(url, "links from web pages may only open data on the public internet")
}

fn refuse_private(url: &str, why: &str) -> io::Result<()> {
    let after = &url[scheme_len(url).unwrap_or(0)..];
    let authority = after.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let default_port = if url.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("https")) { 443 } else { 80 };
    let target = if host_port.starts_with('[') || host_port.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) {
        host_port.to_owned()
    } else {
        format!("{host_port}:{default_port}")
    };
    let addrs = target.to_socket_addrs().map_err(|e| io::Error::new(e.kind(), format!("couldn't resolve {host_port}: {e}")))?;
    for a in addrs {
        if is_private(a.ip()) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{host_port} is a private or local address; URLs that point at them are refused here ({why})")));
        }
    }
    Ok(())
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0) // followed here, so each hop's host is checked
            .user_agent(concat!("ggv/", env!("CARGO_PKG_VERSION")))
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_recv_body(Some(Duration::from_secs(120)))
            .build();
        ureq::Agent::new_with_config(config)
    })
}

/// A response's status, headers we use, and body.
struct Response {
    status: u16,
    content_range: Option<String>,
    etag: Option<String>,
    body: Vec<u8>,
}

/// One GET with optional `Range` and `If-Match`, following redirects (each host checked).
fn get(url: &str, range: Option<(u64, u64)>, etag: Option<&str>, limit: u64) -> io::Result<Response> {
    let mut url = url.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        check_host(&url)?;
        let mut req = agent().get(&url).header("Accept-Encoding", "identity");
        if let Some((start, end)) = range {
            req = req.header("Range", &format!("bytes={start}-{}", end - 1));
        }
        if let Some(tag) = etag {
            req = req.header("If-Match", tag);
        }
        let mut resp = req.call().map_err(http_error)?;
        let status = resp.status().as_u16();
        let header = |name: &str| resp.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
        if (300..400).contains(&status) && status != 304 {
            let Some(loc) = header("location") else { return Err(io::Error::other(format!("HTTP {status} without a Location"))) };
            url = resolve_redirect(&url, &loc);
            continue;
        }
        let (content_range, etag) = (header("content-range"), header("etag"));
        let body = if (200..300).contains(&status) {
            resp.body_mut().with_config().limit(limit).read_to_vec().map_err(http_error)?
        } else {
            Vec::new()
        };
        return Ok(Response { status, content_range, etag, body });
    }
    Err(io::Error::other(format!("more than {MAX_REDIRECTS} redirects")))
}

fn resolve_redirect(from: &str, loc: &str) -> String {
    if scheme_len(loc).is_some() {
        return loc.to_owned();
    }
    let n = scheme_len(from).unwrap_or(0);
    let host_end = from[n..].find('/').map_or(from.len(), |i| n + i);
    if loc.starts_with('/') {
        format!("{}{loc}", &from[..host_end])
    } else {
        let base = redact(from);
        let dir = base.rfind('/').filter(|&i| i >= host_end).map_or(base, |i| &base[..=i]);
        format!("{dir}{loc}")
    }
}

fn http_error(e: ureq::Error) -> io::Error {
    match e {
        ureq::Error::Io(e) => e,
        ureq::Error::Timeout(t) => io::Error::new(io::ErrorKind::TimedOut, format!("timed out ({t})")),
        other => io::Error::other(other.to_string()),
    }
}

/// An HTTP error status as an `io::Error` with a kind callers can test (`NotFound` for 404,
/// which index lookups skip, as they do a missing local file).
fn status_error(status: u16) -> io::Error {
    match status {
        404 | 410 => io::Error::new(io::ErrorKind::NotFound, format!("not found (HTTP {status})")),
        401 | 403 => io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("access denied (HTTP {status}): GGV reads public URLs and presigned URLs; signed requests to private buckets are not supported yet"),
        ),
        412 => io::Error::other("the file changed on the server since it was opened (ETag mismatch); open it again"),
        416 => io::Error::new(io::ErrorKind::UnexpectedEof, "range past the end of the file (HTTP 416)"),
        s => io::Error::other(format!("HTTP {s}")),
    }
}

fn retryable(e: &io::Error) -> bool {
    !matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied | io::ErrorKind::InvalidInput | io::ErrorKind::UnexpectedEof)
        && !e.to_string().contains("changed on the server")
}

fn with_retries<T>(mut f: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut attempt = 0;
    loop {
        match f() {
            Err(e) if attempt + 1 < RETRIES && retryable(&e) => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(250 << attempt));
            }
            other => return other,
        }
    }
}

/// The total length from `Content-Range: bytes 0-0/12345`.
fn total_of(content_range: Option<&str>) -> Option<u64> {
    content_range?.rsplit_once('/')?.1.trim().parse().ok()
}

/// What one open URL shares between its handles.
struct Shared {
    /// What is shown (no query string).
    shown: String,
    /// What is fetched.
    url: String,
    len: u64,
    etag: Option<String>,
    cache: Mutex<Cache>,
}

#[derive(Default)]
struct Cache {
    blocks: HashMap<u64, (Arc<Vec<u8>>, u64)>,
    tick: u64,
    capacity: usize,
}

impl Cache {
    fn get(&mut self, block: u64) -> Option<Arc<Vec<u8>>> {
        self.tick += 1;
        let tick = self.tick;
        self.blocks.get_mut(&block).map(|(data, used)| {
            *used = tick;
            data.clone()
        })
    }

    fn put(&mut self, block: u64, data: Arc<Vec<u8>>) {
        self.tick += 1;
        self.blocks.insert(block, (data, self.tick));
        while self.blocks.len() > self.capacity.max(1) {
            let Some(&oldest) = self.blocks.iter().min_by_key(|(_, (_, used))| *used).map(|(k, _)| k) else { break };
            self.blocks.remove(&oldest);
        }
    }
}

fn cache_blocks() -> usize {
    let mb = crate::folders::env_var_os("REMOTE_CACHE_MB").and_then(|v| v.to_str()?.parse::<u64>().ok()).unwrap_or(64);
    ((mb * 1024 * 1024) / BLOCK).max(4) as usize
}

impl Shared {
    fn open(location: &str) -> io::Result<Arc<Self>> {
        // Handles on the same URL share their cache while any of them is open.
        static OPEN: OnceLock<Mutex<HashMap<String, Weak<Shared>>>> = OnceLock::new();
        let open = OPEN.get_or_init(Default::default);
        if let Some(s) = open.lock().unwrap_or_else(|e| e.into_inner()).get(location).and_then(Weak::upgrade) {
            return Ok(s);
        }
        let url = http_url(location)?;
        // A one-byte range rather than HEAD: a presigned URL is signed for GET only.
        let resp = with_retries(|| get(&url, Some((0, 1)), None, 16))?;
        let len = match resp.status {
            206 => total_of(resp.content_range.as_deref())
                .ok_or_else(|| io::Error::other("the server's range reply has no total length (Content-Range)"))?,
            200 => return Err(io::Error::other("the server doesn't support range requests (HTTP 200 to a Range request), so the file can't be read in parts")),
            416 => 0,
            s => return Err(status_error(s)),
        };
        let shared = Arc::new(Self {
            shown: redact(location).to_owned(),
            url,
            len,
            etag: resp.etag.filter(|t| !t.starts_with("W/")),
            cache: Mutex::new(Cache { capacity: cache_blocks(), ..Default::default() }),
        });
        open.lock().unwrap_or_else(|e| e.into_inner()).insert(location.to_owned(), Arc::downgrade(&shared));
        Ok(shared)
    }

    /// Bytes `start..end` (within the file).
    fn fetch(&self, start: u64, end: u64) -> io::Result<Vec<u8>> {
        with_retries(|| {
            let resp = get(&self.url, Some((start, end)), self.etag.as_deref(), end - start + 1)?;
            match resp.status {
                206 if resp.body.len() as u64 == end - start => Ok(resp.body),
                206 => Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("asked for {} bytes at {start}, got {}", end - start, resp.body.len()))),
                200 => Err(io::Error::other("the server stopped honouring range requests (HTTP 200)")),
                s => Err(status_error(s)),
            }
        })
        .map_err(|e| io::Error::new(e.kind(), format!("reading bytes {start}-{end}: {e}")))
    }

    /// Block `b` (and, when `ahead` > 1, the blocks after it, cached too).
    fn block(&self, b: u64, ahead: u64) -> io::Result<Arc<Vec<u8>>> {
        if let Some(data) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(b) {
            return Ok(data);
        }
        let blocks = self.len.div_ceil(BLOCK);
        // Don't fetch past a block that is already cached.
        let mut n = 1;
        {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            while n < ahead && b + n < blocks && !cache.blocks.contains_key(&(b + n)) {
                n += 1;
            }
        }
        let start = b * BLOCK;
        let end = ((b + n) * BLOCK).min(self.len);
        let bytes = self.fetch(start, end)?;
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let mut first = None;
        for (i, chunk) in bytes.chunks(BLOCK as usize).enumerate() {
            let data = Arc::new(chunk.to_vec());
            if i == 0 {
                first = Some(data.clone());
            }
            cache.put(b + i as u64, data);
        }
        first.ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "empty range"))
    }
}

/// A remote file read by byte range: `Read + Seek` like a `File`.
pub struct RemoteFile {
    shared: Arc<Shared>,
    pos: u64,
    /// The block read last, to detect forward reads and fetch ahead.
    last_block: Option<u64>,
}

impl RemoteFile {
    /// Opens `location` (`https://`, `s3://`, ...): one small request for its length and ETag.
    pub fn open(location: &str) -> io::Result<Self> {
        Ok(Self { shared: Shared::open(location)?, pos: 0, last_block: None })
    }

    pub fn len(&self) -> u64 {
        self.shared.len
    }

    pub fn is_empty(&self) -> bool {
        self.shared.len == 0
    }

    /// The URL without its query string.
    pub fn shown(&self) -> &str {
        &self.shared.shown
    }

    /// The whole file. For indexes, which are read in full.
    pub fn read_all(&self) -> io::Result<Vec<u8>> {
        if self.shared.len == 0 {
            return Ok(Vec::new());
        }
        self.shared.fetch(0, self.shared.len)
    }
}

impl Read for RemoteFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.pos >= self.shared.len {
            return Ok(0);
        }
        let b = self.pos / BLOCK;
        let ahead = if self.last_block.is_some_and(|l| l + 1 == b || l == b) { READ_AHEAD } else { 1 };
        let data = self.shared.block(b, ahead)?;
        self.last_block = Some(b);
        let off = (self.pos - b * BLOCK) as usize;
        let n = buf.len().min(data.len().saturating_sub(off));
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short block"));
        }
        buf[..n].copy_from_slice(&data[off..off + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for RemoteFile {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.shared.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = pos.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before the start"))?;
        Ok(self.pos)
    }
}

/// A local file or a remote one, behind the same `Read + Seek`.
pub enum Source {
    File(std::fs::File),
    Remote(RemoteFile),
}

impl Source {
    /// Opens a local path or a URL.
    pub fn open(path: &Path) -> io::Result<Self> {
        match path.to_str() {
            Some(s) if scheme_len(s).is_some() => RemoteFile::open(s).map(Self::Remote),
            _ => std::fs::File::open(path).map(Self::File),
        }
    }

    pub fn len(&self) -> io::Result<u64> {
        match self {
            Self::File(f) => Ok(f.metadata()?.len()),
            Self::Remote(r) => Ok(r.len()),
        }
    }
}

impl Read for Source {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::File(f) => f.read(buf),
            Self::Remote(r) => r.read(buf),
        }
    }
}

impl Seek for Source {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        match self {
            Self::File(f) => f.seek(to),
            Self::Remote(r) => r.seek(to),
        }
    }
}

/// Whether a local path exists or a URL answers (a remote check costs one small request).
pub fn exists(path: &Path) -> bool {
    match path.to_str() {
        Some(s) if scheme_len(s).is_some() => RemoteFile::open(s).is_ok(),
        _ => path.exists(),
    }
}

/// Whether a local path exists, or the location is a URL (not checked: that costs a request,
/// and opening it says what is wrong). For lists of files to keep or drop without waiting.
pub fn is_present(path: &Path) -> bool {
    is_remote(path) || path.exists()
}

/// A whole small file (an index): read from disk, or fetched once and kept in the cache folder
/// under the URL (without query) and ETag, so the same file isn't fetched again.
pub fn read_whole(path: &Path) -> io::Result<Vec<u8>> {
    let s = match path.to_str() {
        Some(s) if scheme_len(s).is_some() => s,
        _ => return std::fs::read(path),
    };
    let remote = RemoteFile::open(s)?;
    // Not for a server on this machine (fast anyway, and every test server has its own port).
    let cached = remote.shared.etag.as_ref().filter(|_| !is_loopback(&remote.shared.url)).and_then(|etag| {
        let dir = crate::folders::cache_dir()?.join("remote");
        let key = fnv1a(format!("{}\n{etag}\n{}", remote.shown(), remote.len()).as_bytes());
        Some(dir.join(format!("{key:016x}-{}", file_name(path))))
    });
    if let Some(p) = &cached
        && let Ok(bytes) = std::fs::read(p)
        && bytes.len() as u64 == remote.len()
    {
        return Ok(bytes);
    }
    let bytes = remote.read_all()?;
    if let Some(p) = &cached
        && let Some(dir) = p.parent()
        && std::fs::create_dir_all(dir).is_ok()
    {
        prune(dir);
        let part = p.with_added_extension("part");
        if std::fs::write(&part, &bytes).is_ok() {
            let _ = std::fs::rename(&part, p);
        }
    }
    Ok(bytes)
}

/// Whether a URL's host is this machine.
fn is_loopback(url: &str) -> bool {
    let after = &url[scheme_len(url).unwrap_or(0)..];
    let host = after.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = if host.starts_with('[') { host.split(']').next().unwrap_or(host).trim_start_matches('[') } else { host.split(':').next().unwrap_or(host) };
    host.eq_ignore_ascii_case("localhost") || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Removes cached indexes not used for 30 days, so the folder doesn't only grow.
fn prune(dir: &Path) {
    const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let old = e.metadata().and_then(|m| m.accessed().or_else(|_| m.modified())).is_ok_and(|t| t.elapsed().is_ok_and(|age| age > MAX_AGE));
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// FNV-1a: a stable hash for cache file names (std's hasher may change between releases).
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// A tiny HTTP/1.1 file server with range support, for tests (here and in other crates).
#[doc(hidden)]
pub mod test_server {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    /// Serves files under `root` on `127.0.0.1` until the process ends. Returns the base URL
    /// (`http://127.0.0.1:port`) and a count of requests served.
    pub fn serve(root: PathBuf) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let count = Arc::new(AtomicUsize::new(0));
        let served = count.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let root = root.clone();
                let served = served.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                    let mut stream = stream;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
                        let mut range = None;
                        loop {
                            let mut h = String::new();
                            if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                                break;
                            }
                            if let Some((k, v)) = h.split_once(':')
                                && k.eq_ignore_ascii_case("range")
                                && let Some((a, b)) = v.trim().trim_start_matches("bytes=").split_once('-')
                            {
                                range = Some((a.parse::<u64>().unwrap_or(0), b.parse::<u64>().ok()));
                            }
                        }
                        served.fetch_add(1, Ordering::Relaxed);
                        let file = root.join(super::redact(&path).trim_start_matches('/'));
                        let reply = match std::fs::read(&file) {
                            Err(_) => b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n".to_vec(),
                            Ok(bytes) => {
                                let len = bytes.len() as u64;
                                let etag = format!("\"{len:x}\"");
                                match range {
                                    Some((a, _)) if a >= len => format!("HTTP/1.1 416 Range Not Satisfiable\r\ncontent-range: bytes */{len}\r\ncontent-length: 0\r\n\r\n").into_bytes(),
                                    Some((a, b)) => {
                                        let b = b.map_or(len - 1, |b| b.min(len - 1));
                                        let mut r = format!(
                                            "HTTP/1.1 206 Partial Content\r\ncontent-range: bytes {a}-{b}/{len}\r\netag: {etag}\r\ncontent-length: {}\r\n\r\n",
                                            b + 1 - a
                                        )
                                        .into_bytes();
                                        r.extend_from_slice(&bytes[a as usize..=b as usize]);
                                        r
                                    }
                                    None => {
                                        let mut r = format!("HTTP/1.1 200 OK\r\netag: {etag}\r\ncontent-length: {len}\r\n\r\n").into_bytes();
                                        r.extend_from_slice(&bytes);
                                        r
                                    }
                                }
                            }
                        };
                        if stream.write_all(&reply).is_err() {
                            return;
                        }
                    }
                });
            }
        });
        (base, count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_names() {
        let p = Path::new("https://h.org/d/x.cram?X-Amz-Signature=abc");
        assert!(is_remote(p) && !is_remote(Path::new("/data/x.cram")) && !is_remote(Path::new("https://")));
        assert_eq!(file_name(p), "x.cram");
        assert_eq!(display(p), "https://h.org/d/x.cram");
        assert_eq!(with_added_extension(p, "crai"), PathBuf::from("https://h.org/d/x.cram.crai?X-Amz-Signature=abc"));
        assert_eq!(with_extension(p, "crai"), PathBuf::from("https://h.org/d/x.crai?X-Amz-Signature=abc"));
        assert_eq!(with_extension(Path::new("https://h.org/d.v/x"), "bai"), PathBuf::from("https://h.org/d.v/x.bai"));
        assert_eq!(http_url("gs://b/k 1.bam").unwrap(), "https://storage.googleapis.com/b/k%201.bam");
        assert!(http_url("s3://bucket").is_err());
        assert_eq!(resolve_redirect("https://a.org/x/y.bam?s=1", "/z.bam"), "https://a.org/z.bam");
        assert_eq!(resolve_redirect("https://a.org/x/y.bam", "z.bam"), "https://a.org/x/z.bam");
        assert_eq!(total_of(Some("bytes 0-0/25241482211")), Some(25_241_482_211));
        assert!(is_loopback("http://127.0.0.1:8080/x") && is_loopback("http://[::1]/x") && is_loopback("http://localhost/x") && !is_loopback("https://s3.amazonaws.com/x"));
        assert!(is_private("169.254.169.254".parse().unwrap()) && is_private("::1".parse().unwrap()) && !is_private("52.216.0.1".parse().unwrap()));
    }

    #[test]
    fn reads_by_range_like_a_file() {
        let dir = std::env::temp_dir().join(format!("ggv-remote-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bytes: Vec<u8> = (0..(BLOCK as usize * 3 + 123)).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(dir.join("f.bin"), &bytes).unwrap();
        let (base, served) = test_server::serve(dir.clone());

        let mut f = RemoteFile::open(&format!("{base}/f.bin?sig=1")).unwrap();
        assert_eq!(f.len(), bytes.len() as u64);
        let mut all = Vec::new();
        f.read_to_end(&mut all).unwrap();
        assert_eq!(all, bytes);
        let mut buf = [0u8; 10];
        f.seek(SeekFrom::Start(BLOCK - 5)).unwrap();
        f.read_exact(&mut buf).unwrap();
        assert_eq!(buf, bytes[BLOCK as usize - 5..BLOCK as usize + 5]);
        f.seek(SeekFrom::End(-3)).unwrap();
        assert_eq!(f.read(&mut buf).unwrap(), 3);
        // Everything after the first full read came from the cache.
        let before = served.load(Ordering::Relaxed);
        f.seek(SeekFrom::Start(0)).unwrap();
        f.read_exact(&mut buf).unwrap();
        assert_eq!(served.load(Ordering::Relaxed), before);

        let missing = RemoteFile::open(&format!("{base}/nope.bin")).err().unwrap();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
        assert!(exists(Path::new(&format!("{base}/f.bin"))) && !exists(Path::new(&format!("{base}/nope.bin"))));

        let refused = check_host_with(&format!("{base}/g.bin"), false).err().unwrap();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied, "{refused}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
