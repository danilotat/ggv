//! IGV links: web pages open data in a running IGV with
//! `http://localhost:60151/load?file=<url>&index=<url>&locus=...`. The window listens on the
//! same port (only on this computer's loopback address) and does what the link asks in its
//! active pane, as if an agent had sent `open` and `goto`. See `ggv_io::igv_link` for the
//! parameters.
//!
//! Any page in the browser can send these requests, so:
//! - only data at public URLs opens (no local files, no private or local addresses);
//! - a request whose `Host` isn't this computer is refused (a DNS-rebinding page);
//! - what a link opens is announced in the window, so nothing loads unseen.
//!
//! The port is 60151 as in IGV, `--igv-port`/`GGV_IGV_PORT` to change it, `--no-igv-port`,
//! `GGV_IGV_PORT=off` or `"igv_port": false` in prefs.json to turn it off. When another program
//! (IGV, or another GGV window) has the port, links go there, and this window says so on stderr.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc::Sender,
    time::Duration,
};

use eframe::egui;
use ggv_io::igv_link::{self, Request};
use serde_json::{Value, json};

use crate::attach::{Command, dispatch};

/// The longest request line plus headers read.
const MAX_HEAD: u64 = 16 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// The port to listen on, from (first found) the command line, `GGV_IGV_PORT`, prefs.json
/// (`"igv_port": <port> | false`), else 60151; `None` when turned off.
pub fn configured_port(cli: Option<u16>, off: bool) -> Option<u16> {
    if off {
        return None;
    }
    if let Some(p) = cli {
        return Some(p);
    }
    if let Some(v) = std::env::var_os("GGV_IGV_PORT") {
        let v = v.to_string_lossy().trim().to_ascii_lowercase();
        return match v.as_str() {
            "off" | "no" | "false" | "0" => None,
            _ => match v.parse() {
                Ok(p) => Some(p),
                Err(_) => {
                    eprintln!("ggv: GGV_IGV_PORT={v} is not a port number or `off`; using {}", igv_link::DEFAULT_PORT);
                    Some(igv_link::DEFAULT_PORT)
                }
            },
        };
    }
    match crate::app::load_prefs().get("igv_port") {
        Some(Value::Bool(false)) => None,
        Some(v) => Some(v.as_u64().and_then(|p| u16::try_from(p).ok()).unwrap_or(igv_link::DEFAULT_PORT)),
        None => Some(igv_link::DEFAULT_PORT),
    }
}

/// Listens on `port` (0: any free one, for tests) and returns the port bound, or `None` (with
/// a note on stderr) when it is taken.
pub fn start(port: u16, tx: Sender<Command>, ctx: &egui::Context) -> Option<u16> {
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("ggv: not answering IGV links on port {port} ({e}); another program, such as IGV or another GGV window, may have it");
            return None;
        }
    };
    let bound = listener.local_addr().ok()?.port();
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("ggv-igv-port".into())
        .spawn(move || {
            // One at a time: a link opens files, and two at once would race for the pane.
            for stream in listener.incoming().flatten() {
                serve(stream, bound, &tx, &ctx);
            }
        })
        .ok()?;
    Some(bound)
}

fn serve(stream: TcpStream, port: u16, tx: &Sender<Command>, ctx: &egui::Context) {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
    let Ok(mut writer) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream.take(MAX_HEAD));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut host = None;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return respond(&mut writer, 400, "request headers too long or cut short"),
            Ok(_) if line.trim().is_empty() => break,
            Ok(_) => {
                if let Some((k, v)) = line.split_once(':')
                    && k.trim().eq_ignore_ascii_case("host")
                {
                    host = Some(v.trim().to_owned());
                }
            }
            Err(_) => return,
        }
    }
    let mut parts = request_line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if let Some(h) = &host
        && !local_host(h, port)
    {
        return respond(&mut writer, 403, "this port answers pages addressing localhost only");
    }
    match method {
        // A browser's preflight (Chrome asks before a public page reaches localhost).
        "OPTIONS" => respond(&mut writer, 204, ""),
        "GET" => {
            let (status, body) = answer(target, tx, ctx);
            respond(&mut writer, status, &body);
        }
        _ => respond(&mut writer, 405, "GGV answers GET requests: /load?file=<url>, /goto?locus=<locus>, /echo"),
    }
}

/// What the link asks, done by the window: (HTTP status, body). IGV answers "OK".
fn answer(target: &str, tx: &Sender<Command>, ctx: &egui::Context) -> (u16, String) {
    let request = match igv_link::parse(target) {
        Ok(r) => r,
        Err(e) => return (400, e),
    };
    if request == Request::Echo {
        return (200, "echo".into());
    }
    if let Err(e) = request.check() {
        return (403, e);
    }
    let params = match &request {
        Request::Load { files, merge, locus, genome } => json!({
            "files": files.iter().map(|(f, i)| json!({"file": f, "index": i})).collect::<Vec<_>>(),
            "merge": merge,
            "locus": locus,
            "genome": genome,
        }),
        Request::Goto { locus } => json!({"locus": locus}),
        Request::Echo => unreachable!(),
    };
    let reply = dispatch(tx, ctx, "link".into(), params);
    match reply.get("error") {
        None => (200, "OK".into()),
        Some(e) => (500, e.as_str().map_or_else(|| e.to_string(), str::to_owned)),
    }
}

/// Whether a `Host` header names this computer on our port: `localhost`, `127.0.0.1` or
/// `[::1]`, with the port or without.
fn local_host(host: &str, port: u16) -> bool {
    let host = host.trim().to_ascii_lowercase();
    let (name, p) = match host.rsplit_once(':') {
        Some((n, p)) if !n.is_empty() && !p.contains(']') => (n.to_owned(), Some(p.to_owned())),
        _ => (host.clone(), None),
    };
    ["localhost", "127.0.0.1", "[::1]"].contains(&name.as_str()) && p.is_none_or(|p| p == port.to_string())
}

fn respond(w: &mut TcpStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, OPTIONS\r\nAccess-Control-Allow-Headers: *\r\nAccess-Control-Allow-Private-Network: true\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = w.write_all(head.as_bytes()).and_then(|()| w.write_all(body.as_bytes()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_computer_as_host() {
        for ok in ["localhost:60151", "127.0.0.1:60151", "[::1]:60151", "LOCALHOST", "127.0.0.1"] {
            assert!(local_host(ok, 60151), "{ok}");
        }
        for bad in ["evil.example:60151", "localhost:8080", "127.0.0.1.evil.example", "", "[::1]:1"] {
            assert!(!local_host(bad, 60151), "{bad}");
        }
    }

    /// A raw request, as a browser sends it; returns (status, body).
    pub(crate) fn get(port: u16, target: &str, host: &str) -> (u16, String) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(s, "GET {target} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: test\r\n\r\n").unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        let status = out.split_whitespace().nth(1).unwrap().parse().unwrap();
        assert!(out.contains("Access-Control-Allow-Origin: *"), "{out}");
        (status, out.split("\r\n\r\n").nth(1).unwrap_or("").to_owned())
    }

    #[test]
    fn echo_refusals_and_the_window_asked() {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel::<Command>();
        let port = start(0, tx, &ctx).unwrap();
        let host = format!("localhost:{port}");
        assert_eq!(get(port, "/echo", &host), (200, "echo".into()));
        assert_eq!(get(port, "/echo", "attacker.example").0, 403);
        let (status, body) = get(port, "/load?file=/etc/hosts.bam", &host);
        assert_eq!(status, 403, "{body}");
        assert_eq!(get(port, "/load?file=http://127.0.0.1/a.bam", &host).0, 403);
        assert_eq!(get(port, "/frobnicate", &host).0, 400);

        // A good link reaches the window as `link`, and its answer is the reply.
        let window = std::thread::spawn(move || {
            let c = rx.recv().unwrap();
            assert!(c.take());
            assert_eq!(c.method, "link");
            assert_eq!(c.params["files"][0]["file"], "https://1.1.1.1/a.bam");
            assert_eq!(c.params["files"][0]["index"], "https://1.1.1.1/a.bam.bai");
            assert_eq!(c.params["merge"], true);
            c.reply.send(Ok(json!({}))).unwrap();
            let c = rx.recv().unwrap();
            assert!(c.take());
            c.reply.send(Err("no such file".into())).unwrap();
        });
        assert_eq!(get(port, "/load?merge=true&file=https://1.1.1.1/a.bam&index=https://1.1.1.1/a.bam.bai&", &host), (200, "OK".into()));
        assert_eq!(get(port, "/load?file=https://1.1.1.1/b.bam", &host), (500, "no such file".into()));
        window.join().unwrap();
    }
}
