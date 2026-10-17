//! Fetching the manifest and the installer (D-160). The HTTP itself is behind `Transport`
//! (WinHTTP on Windows, a plain local test server in the tests), so resume, retries, size and hash
//! checks are the same code everywhere and are tested here.
//!
//! Rules:
//! - Only paths on the one pinned host; the transport never follows redirects (a 3xx is an error).
//! - Small files (manifest, signature) have a hard size cap.
//! - The installer is written to a fresh folder; an interrupted transfer resumes with a Range request
//!   (3 retries, backoff 1 s, 2 s, 4 s). A server that ignores Range restarts the file from 0.
//! - Never more bytes than the manifest's size; at the end the SHA-256 must match, otherwise the file
//!   is deleted, downloaded once more from scratch, and if it fails again the download is refused.
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const RETRIES: u32 = 3;
pub const MAX_SMALL: u64 = 64 * 1024;

pub struct Response {
    pub status: u16,
    pub content_length: Option<u64>,
    /// From Content-Range on a 206: (first byte, total size)
    pub range: Option<(u64, u64)>,
    pub body: Box<dyn Read + Send>,
}

pub trait Transport {
    /// GET `path` on the pinned host. `from > 0` adds `Range: bytes=<from>-`.
    fn get(&mut self, path: &str, from: u64) -> Result<Response, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    Cancelled,
    Network(String),
    Status(u16),
    Redirect(u16),
    TooLarge,
    Corrupt,
    Io(String),
    DiskFull,
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Cancelled => write!(f, "cancelled"),
            DownloadError::Network(e) => write!(f, "network: {e}"),
            DownloadError::Status(s) => write!(f, "server answered HTTP {s}"),
            DownloadError::Redirect(s) => write!(f, "server answered a redirect (HTTP {s}), which is refused"),
            DownloadError::TooLarge => write!(f, "server sent more data than expected"),
            DownloadError::Corrupt => write!(f, "size or SHA-256 does not match the signed manifest"),
            DownloadError::Io(e) => write!(f, "file: {e}"),
            DownloadError::DiskFull => write!(f, "disk full"),
        }
    }
}

fn status_error(status: u16) -> DownloadError {
    if (300..400).contains(&status) {
        DownloadError::Redirect(status)
    } else {
        DownloadError::Status(status)
    }
}

/// GET a small file (manifest, signature), exactly as served.
pub fn fetch_small(t: &mut dyn Transport, path: &str) -> Result<Vec<u8>, DownloadError> {
    let r = t.get(path, 0).map_err(DownloadError::Network)?;
    if r.status != 200 {
        return Err(status_error(r.status));
    }
    if r.content_length.is_some_and(|n| n > MAX_SMALL) {
        return Err(DownloadError::TooLarge);
    }
    let mut buf = Vec::new();
    r.body.take(MAX_SMALL + 1).read_to_end(&mut buf).map_err(|e| DownloadError::Network(e.to_string()))?;
    if buf.len() as u64 > MAX_SMALL {
        return Err(DownloadError::TooLarge);
    }
    if r.content_length.is_some_and(|n| n != buf.len() as u64) {
        return Err(DownloadError::Network("connection closed early".into()));
    }
    Ok(buf)
}

pub fn sha256_file(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().into())
}

fn io_err(e: std::io::Error) -> DownloadError {
    // ERROR_DISK_FULL (112) / ERROR_HANDLE_DISK_FULL (39) on Windows, ENOSPC (28) elsewhere
    match e.raw_os_error() {
        Some(112) | Some(39) if cfg!(windows) => DownloadError::DiskFull,
        Some(28) if !cfg!(windows) => DownloadError::DiskFull,
        _ => DownloadError::Io(e.to_string()),
    }
}

pub struct Expect<'a> {
    pub path: &'a str,
    pub size: u64,
    pub sha256: [u8; 32],
}

/// Downloads to `dest` and returns only when the file has exactly `size` bytes and the right SHA-256.
pub fn download(
    t: &mut dyn Transport,
    want: &Expect<'_>,
    dest: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
    sleep: &dyn Fn(Duration),
) -> Result<(), DownloadError> {
    let mut failures = 0u32;
    let mut hash_retry_used = false;
    let mut file = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(dest).map_err(io_err)?;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        let have = file.seek(SeekFrom::End(0)).map_err(io_err)?;
        if have > want.size {
            file.set_len(0).map_err(io_err)?;
            continue;
        }
        let step = if have == want.size { Ok(()) } else { fetch_range(t, want, &mut file, have, cancel, progress) };
        match step {
            Ok(()) => {
                file.flush().map_err(io_err)?;
                file.sync_all().map_err(io_err)?;
                let got = sha256_file(dest).map_err(io_err)?;
                if got == want.sha256 {
                    return Ok(());
                }
                file.set_len(0).map_err(io_err)?;
                if hash_retry_used {
                    return Err(DownloadError::Corrupt);
                }
                hash_retry_used = true;
            }
            Err(e @ (DownloadError::Cancelled | DownloadError::Redirect(_) | DownloadError::TooLarge
                | DownloadError::Io(_) | DownloadError::DiskFull)) => return Err(e),
            Err(DownloadError::Status(s)) if (400..500).contains(&s) && s != 416 && s != 408 && s != 429 => {
                return Err(DownloadError::Status(s))
            }
            Err(e) => {
                if failures >= RETRIES {
                    return Err(e);
                }
                if matches!(e, DownloadError::Status(416)) {
                    file.set_len(0).map_err(io_err)?;
                }
                sleep(Duration::from_secs(1 << failures));
                failures += 1;
            }
        }
    }
}

fn fetch_range(
    t: &mut dyn Transport,
    want: &Expect<'_>,
    file: &mut File,
    from: u64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(), DownloadError> {
    let r = t.get(want.path, from).map_err(DownloadError::Network)?;
    let mut pos = from;
    match (r.status, from) {
        (200, _) => {
            if r.content_length.is_some_and(|n| n != want.size) {
                return Err(DownloadError::Corrupt);
            }
            if from > 0 {
                // server ignored the Range header: start over
                file.set_len(0).map_err(io_err)?;
                file.seek(SeekFrom::Start(0)).map_err(io_err)?;
                pos = 0;
            }
        }
        (206, f) if f > 0 => match r.range {
            Some((start, total)) if start == from && total == want.size => {}
            _ => return Err(DownloadError::Network("unexpected Content-Range".into())),
        },
        (s, _) => return Err(status_error(s)),
    }
    progress(pos, want.size);
    let mut body = r.body;
    let mut buf = vec![0u8; 1 << 16];
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        let n = body.read(&mut buf).map_err(|e| DownloadError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        if pos + n as u64 > want.size {
            file.set_len(0).map_err(io_err)?;
            return Err(DownloadError::TooLarge);
        }
        file.write_all(&buf[..n]).map_err(io_err)?;
        pos += n as u64;
        progress(pos, want.size);
    }
    if pos != want.size {
        return Err(DownloadError::Network("connection closed early".into()));
    }
    Ok(())
}

/// Opens the finished download read-only and denies other writers and deleters (Windows share mode
/// FILE_SHARE_READ only) for as long as the returned File lives. The caller hashes through this handle and
/// keeps it open until msiexec has finished, so the verified bytes are the bytes msiexec reads: nobody can
/// replace or rename the file (or its folder) in between (D-160, audit H1).
pub fn open_locked(path: &Path) -> std::io::Result<File> {
    let mut o = OpenOptions::new();
    o.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.share_mode(0x0000_0001); // FILE_SHARE_READ
    }
    o.open(path)
}

/// SHA-256 and length through an open handle, from the start.
pub fn sha256_handle(f: &mut File) -> std::io::Result<([u8; 32], u64)> {
    f.seek(SeekFrom::Start(0))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut n_total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        n_total += n as u64;
        h.update(&buf[..n]);
    }
    Ok((h.finalize().into(), n_total))
}

/// Removes the download folder and everything in it (best effort; called on every exit path).
pub fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

#[cfg(test)]
pub(crate) mod testserver {
    //! A tiny HTTP/1.1 server on 127.0.0.1 and a matching plain-HTTP Transport, for resume, retry and
    //! corrupt-file tests. Behaviour per request is scripted.
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread;

    #[derive(Clone, Debug)]
    pub enum Act {
        /// Serve normally (honour Range)
        Ok,
        /// Serve, but cut the connection after n body bytes
        Cut(usize),
        /// Ignore Range, always send the whole file with 200
        IgnoreRange,
        /// Flip one byte at offset
        Corrupt(usize),
        /// Answer this status with an empty body
        Status(u16),
        /// Send extra bytes after the file
        Overlong,
    }

    pub struct Server {
        pub addr: String,
        pub log: Arc<Mutex<Vec<String>>>,
    }

    pub fn start(data: Vec<u8>, script: Vec<Act>) -> Server {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let log = Arc::new(Mutex::new(Vec::new()));
        let log2 = log.clone();
        let script = Arc::new(Mutex::new(script.into_iter()));
        thread::spawn(move || {
            for s in l.incoming() {
                let Ok(s) = s else { break };
                let act = script.lock().unwrap().next().unwrap_or(Act::Ok);
                handle(s, &data, act, &log2);
            }
        });
        Server { addr, log }
    }

    fn handle(mut s: TcpStream, data: &[u8], act: Act, log: &Arc<Mutex<Vec<String>>>) {
        let mut rd = BufReader::new(s.try_clone().unwrap());
        let mut line = String::new();
        rd.read_line(&mut line).unwrap();
        let mut from = 0u64;
        loop {
            let mut h = String::new();
            if rd.read_line(&mut h).unwrap() == 0 || h == "\r\n" {
                break;
            }
            if let Some(v) = h.to_ascii_lowercase().strip_prefix("range: bytes=") {
                from = v.trim().trim_end_matches('-').parse().unwrap();
            }
        }
        log.lock().unwrap().push(format!("{} from={from} act={act:?}", line.trim()));
        let mut body = data.to_vec();
        let (status, start) = match act {
            Act::Status(code) => {
                let _ = write!(s, "HTTP/1.1 {code} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                return;
            }
            Act::IgnoreRange => (200, 0),
            _ if from > 0 => (206, from as usize),
            _ => (200, 0),
        };
        if let Act::Corrupt(i) = act {
            body[i] ^= 0xff;
        }
        let mut payload = body[start..].to_vec();
        if let Act::Overlong = act {
            payload.extend_from_slice(b"EXTRA");
        }
        let mut head = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n", payload.len());
        if status == 206 {
            head += &format!("Content-Range: bytes {}-{}/{}\r\n", start, data.len() - 1, data.len());
        }
        head += "\r\n";
        let _ = s.write_all(head.as_bytes());
        let n = match act {
            Act::Cut(n) => n.min(payload.len()),
            _ => payload.len(),
        };
        let _ = s.write_all(&payload[..n]);
    }

    pub struct PlainHttp {
        pub addr: String,
    }

    impl Transport for PlainHttp {
        fn get(&mut self, path: &str, from: u64) -> Result<Response, String> {
            let mut s = TcpStream::connect(&self.addr).map_err(|e| e.to_string())?;
            let range = if from > 0 { format!("Range: bytes={from}-\r\n") } else { String::new() };
            write!(s, "GET {path} HTTP/1.1\r\nHost: test\r\n{range}Connection: close\r\n\r\n").map_err(|e| e.to_string())?;
            let mut rd = BufReader::new(s);
            let mut line = String::new();
            rd.read_line(&mut line).map_err(|e| e.to_string())?;
            let status: u16 = line.split_whitespace().nth(1).ok_or("bad status")?.parse().map_err(|_| "bad status")?;
            let (mut len, mut range) = (None, None);
            loop {
                let mut h = String::new();
                if rd.read_line(&mut h).map_err(|e| e.to_string())? == 0 || h == "\r\n" {
                    break;
                }
                let lower = h.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    len = v.trim().parse().ok();
                }
                if let Some(v) = lower.strip_prefix("content-range: bytes ") {
                    let (span, total) = v.trim().split_once('/').unwrap();
                    let start = span.split_once('-').unwrap().0.parse().unwrap();
                    range = Some((start, total.parse().unwrap()));
                }
            }
            Ok(Response { status, content_length: len, range, body: Box::new(rd) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testserver::{start, Act, PlainHttp};
    use super::*;
    use std::cell::RefCell;

    fn blob() -> Vec<u8> {
        (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect()
    }
    fn hash(d: &[u8]) -> [u8; 32] {
        Sha256::digest(d).into()
    }
    struct Tmp(std::path::PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn tmp(name: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("sdsetup-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        Tmp(d)
    }

    fn run(script: Vec<Act>, data: &[u8], expect_hash: [u8; 32], name: &str) -> (Result<(), DownloadError>, Vec<String>, Vec<Duration>, Option<Vec<u8>>) {
        let srv = start(data.to_vec(), script);
        let mut t = PlainHttp { addr: srv.addr.clone() };
        let d = tmp(name);
        let dest = d.0.join("x.msi");
        let sleeps = RefCell::new(Vec::new());
        let want = Expect { path: "/client/x.msi", size: data.len() as u64, sha256: expect_hash };
        let cancel = AtomicBool::new(false);
        let mut last = 0;
        let r = download(&mut t, &want, &dest, &cancel, &mut |done, _| last = done, &|dur| sleeps.borrow_mut().push(dur));
        let file = fs::read(&dest).ok();
        let log = srv.log.lock().unwrap().clone();
        (r, log, sleeps.into_inner(), file)
    }

    #[test]
    fn plain() {
        let d = blob();
        let (r, log, sleeps, file) = run(vec![Act::Ok], &d, hash(&d), "plain");
        assert_eq!(r, Ok(()));
        assert_eq!(file.unwrap(), d);
        assert_eq!(log.len(), 1);
        assert!(sleeps.is_empty());
    }

    #[test]
    fn resume_after_cut() {
        let d = blob();
        let (r, log, sleeps, file) = run(vec![Act::Cut(100_000), Act::Cut(50_000), Act::Ok], &d, hash(&d), "resume");
        assert_eq!(r, Ok(()));
        assert_eq!(file.unwrap(), d);
        assert!(log[1].contains("from=100000"), "{log:?}");
        assert!(log[2].contains("from=150000"), "{log:?}");
        assert_eq!(sleeps, vec![Duration::from_secs(1), Duration::from_secs(2)]);
    }

    #[test]
    fn server_ignoring_range_restarts() {
        let d = blob();
        let (r, _, _, file) = run(vec![Act::Cut(100_000), Act::IgnoreRange], &d, hash(&d), "norange");
        assert_eq!(r, Ok(()));
        assert_eq!(file.unwrap(), d);
    }

    #[test]
    fn gives_up_after_three_retries() {
        let d = blob();
        let s = vec![Act::Cut(10), Act::Cut(10), Act::Cut(10), Act::Cut(10), Act::Ok];
        let (r, log, sleeps, _) = run(s, &d, hash(&d), "giveup");
        assert!(matches!(r, Err(DownloadError::Network(_))), "{r:?}");
        assert_eq!(log.len(), 4);
        assert_eq!(sleeps.len(), 3);
    }

    #[test]
    fn corrupt_once_then_good() {
        let d = blob();
        let (r, log, _, file) = run(vec![Act::Corrupt(5000), Act::Ok], &d, hash(&d), "corrupt1");
        assert_eq!(r, Ok(()));
        assert_eq!(file.unwrap(), d);
        assert_eq!(log.len(), 2);
    }

    #[test]
    fn corrupt_twice_refused_and_file_emptied() {
        let d = blob();
        let (r, _, _, file) = run(vec![Act::Corrupt(5000), Act::Corrupt(9)], &d, hash(&d), "corrupt2");
        assert_eq!(r, Err(DownloadError::Corrupt));
        assert_eq!(file.unwrap().len(), 0, "a bad file must never be left for msiexec");
    }

    #[test]
    fn wrong_expected_hash_refused() {
        let d = blob();
        let (r, _, _, _) = run(vec![Act::Ok, Act::Ok], &d, [0u8; 32], "badhash");
        assert_eq!(r, Err(DownloadError::Corrupt));
    }

    #[test]
    fn overlong_refused() {
        let d = blob();
        // a wrong Content-Length is treated like a broken transfer: retried, then refused
        let (r, log, _, file) = run(vec![Act::Overlong; 5], &d, hash(&d), "overlong");
        assert!(matches!(r, Err(DownloadError::Corrupt) | Err(DownloadError::TooLarge)), "{r:?}");
        assert_eq!(log.len(), 4);
        assert!(file.unwrap().len() <= d.len());
    }

    #[test]
    fn redirect_and_404_not_retried() {
        let d = blob();
        let (r, log, _, _) = run(vec![Act::Status(302)], &d, hash(&d), "redir");
        assert_eq!(r, Err(DownloadError::Redirect(302)));
        assert_eq!(log.len(), 1);
        let (r, log, _, _) = run(vec![Act::Status(404)], &d, hash(&d), "404");
        assert_eq!(r, Err(DownloadError::Status(404)));
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn server_error_retried() {
        let d = blob();
        let (r, log, _, _) = run(vec![Act::Status(503), Act::Ok], &d, hash(&d), "503");
        assert_eq!(r, Ok(()));
        assert_eq!(log.len(), 2);
    }

    #[test]
    fn cancel_stops() {
        let d = blob();
        let srv = start(d.clone(), vec![Act::Ok]);
        let mut t = PlainHttp { addr: srv.addr.clone() };
        let dir = tmp("cancel");
        let cancel = AtomicBool::new(true);
        let want = Expect { path: "/client/x.msi", size: d.len() as u64, sha256: hash(&d) };
        let r = download(&mut t, &want, &dir.0.join("x.msi"), &cancel, &mut |_, _| {}, &|_| {});
        assert_eq!(r, Err(DownloadError::Cancelled));
    }

    #[test]
    fn small_files() {
        let srv = start(b"{\"a\":1}".to_vec(), vec![Act::Ok, Act::Status(302)]);
        let mut t = PlainHttp { addr: srv.addr.clone() };
        assert_eq!(fetch_small(&mut t, "/client/latest.json").unwrap(), b"{\"a\":1}");
        assert_eq!(fetch_small(&mut t, "/client/latest.json"), Err(DownloadError::Redirect(302)));
        let big = start(vec![b'x'; (MAX_SMALL + 10) as usize], vec![Act::Ok]);
        let mut t = PlainHttp { addr: big.addr.clone() };
        assert_eq!(fetch_small(&mut t, "/client/latest.json"), Err(DownloadError::TooLarge));
    }
}
