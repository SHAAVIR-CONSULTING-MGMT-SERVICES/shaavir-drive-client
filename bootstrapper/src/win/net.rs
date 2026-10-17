//! WinHTTP transport (D-160): uses the system proxy settings (automatic proxy) and the Windows
//! certificate store, so corporate proxies and TLS inspection work like in the browser.
//! HTTPS only, one pinned host, port 443, redirects disabled (a 3xx comes back as a status and the
//! portable code refuses it), TLS 1.2 or newer.
use super::wide;
use sdrive_setup::download::{Response, Transport};
use std::ffi::c_void;
use std::io::Read;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Networking::WinHttp::*;

const OPTION_DISABLE_FEATURE: u32 = 63;
const OPTION_ENABLE_FEATURE: u32 = 79;
const ENABLE_SSL_REVOCATION: u32 = 0x0000_0001;
const DISABLE_REDIRECTS: u32 = 0x0000_0002;
const OPTION_SECURE_PROTOCOLS: u32 = 84;
const PROTO_TLS12: u32 = 0x0000_0800;
const PROTO_TLS13: u32 = 0x0000_2000;
const QUERY_STATUS_CODE: u32 = 19;
const QUERY_CONTENT_LENGTH: u32 = 5;
const QUERY_CUSTOM: u32 = 65535;
const QUERY_FLAG_NUMBER: u32 = 0x2000_0000;

struct Handle(*mut c_void);
// WinHTTP handles may be used from another thread as long as calls are not concurrent; each
// request is owned by exactly one reader.
unsafe impl Send for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

pub struct WinHttp {
    session: Handle,
    connect: Handle,
}

fn err(what: &str) -> String {
    format!("{what} failed (error {})", unsafe { GetLastError() })
}

impl WinHttp {
    pub fn new(host: &str) -> Result<WinHttp, String> {
        let agent = wide(&format!("{}Setup/1.0", sdrive_setup::tenant::SHORTNAME));
        let session = unsafe { WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY as _, null(), null(), 0) };
        if session.is_null() {
            // Windows before 8.1 has no automatic-proxy mode; fall back to the default proxy settings
            let s2 = unsafe { WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_DEFAULT_PROXY as _, null(), null(), 0) };
            if s2.is_null() {
                return Err(err("WinHttpOpen"));
            }
            return Self::finish(Handle(s2), host);
        }
        Self::finish(Handle(session), host)
    }

    fn finish(session: Handle, host: &str) -> Result<WinHttp, String> {
        unsafe {
            let both = PROTO_TLS12 | PROTO_TLS13;
            if WinHttpSetOption(session.0, OPTION_SECURE_PROTOCOLS, &both as *const u32 as *const c_void, 4) == 0 {
                // TLS 1.3 flag unknown on older Windows 10: TLS 1.2 only
                let only12 = PROTO_TLS12;
                WinHttpSetOption(session.0, OPTION_SECURE_PROTOCOLS, &only12 as *const u32 as *const c_void, 4);
            }
            WinHttpSetTimeouts(session.0, 30_000, 30_000, 30_000, 60_000);
        }
        let h = wide(host);
        let connect = unsafe { WinHttpConnect(session.0, h.as_ptr(), 443, 0) };
        if connect.is_null() {
            return Err(err("WinHttpConnect"));
        }
        Ok(WinHttp { session, connect: Handle(connect) })
    }
}

fn query_string(req: *mut c_void, level: u32, name: Option<&str>) -> Option<String> {
    let name_w = name.map(wide);
    let name_p = name_w.as_ref().map_or(null(), |v| v.as_ptr());
    let mut buf = vec![0u16; 256];
    let mut len: u32 = (buf.len() * 2) as u32;
    let ok = unsafe { WinHttpQueryHeaders(req, level, name_p, buf.as_mut_ptr() as *mut c_void, &mut len, null_mut()) };
    if ok == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..(len as usize / 2)]))
}

struct Body {
    req: Handle,
}

impl Read for Body {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let mut n: u32 = 0;
        let want = out.len().min(1 << 20) as u32;
        let ok = unsafe { WinHttpReadData(self.req.0, out.as_mut_ptr() as *mut c_void, want, &mut n) };
        if ok == 0 {
            return Err(std::io::Error::other(err("WinHttpReadData")));
        }
        Ok(n as usize)
    }
}

impl Transport for WinHttp {
    fn get(&mut self, path: &str, from: u64) -> Result<Response, String> {
        let verb = wide("GET");
        let obj = wide(path);
        let req = unsafe {
            WinHttpOpenRequest(self.connect.0, verb.as_ptr(), obj.as_ptr(), null(), null(), null(), WINHTTP_FLAG_SECURE as _)
        };
        if req.is_null() {
            return Err(err("WinHttpOpenRequest"));
        }
        let req = Handle(req);
        unsafe {
            let dis = DISABLE_REDIRECTS;
            if WinHttpSetOption(req.0, OPTION_DISABLE_FEATURE, &dis as *const u32 as *const c_void, 4) == 0 {
                return Err(err("disabling redirects"));
            }
            // certificate revocation check, soft-fail like the browsers (audit L6)
            let rev = ENABLE_SSL_REVOCATION;
            WinHttpSetOption(req.0, OPTION_ENABLE_FEATURE, &rev as *const u32 as *const c_void, 4);
        }
        let mut headers = String::from("Cache-Control: no-cache\r\n");
        if from > 0 {
            headers.push_str(&format!("Range: bytes={from}-\r\n"));
        }
        let hw = wide(&headers);
        let sent = unsafe { WinHttpSendRequest(req.0, hw.as_ptr(), u32::MAX, null(), 0, 0, 0) };
        if sent == 0 {
            return Err(err("connecting to the update server"));
        }
        if unsafe { WinHttpReceiveResponse(req.0, null_mut()) } == 0 {
            return Err(err("waiting for the update server"));
        }
        let mut status: u32 = 0;
        let mut len: u32 = 4;
        let ok = unsafe {
            WinHttpQueryHeaders(req.0, QUERY_STATUS_CODE | QUERY_FLAG_NUMBER, null(), &mut status as *mut u32 as *mut c_void, &mut len, null_mut())
        };
        if ok == 0 {
            return Err(err("reading the status"));
        }
        let content_length = query_string(req.0, QUERY_CONTENT_LENGTH, None).and_then(|s| s.trim().parse().ok());
        let range = query_string(req.0, QUERY_CUSTOM, Some("Content-Range")).and_then(|s| {
            // "bytes 100-999/1000"
            let v = s.trim().strip_prefix("bytes ")?;
            let (span, total) = v.split_once('/')?;
            let start = span.split_once('-')?.0.parse().ok()?;
            Some((start, total.parse().ok()?))
        });
        Ok(Response { status: status as u16, content_length, range, body: Box::new(Body { req }) })
    }
}

impl Drop for WinHttp {
    fn drop(&mut self) {
        // connect before session
        let c = std::mem::replace(&mut self.connect, Handle(null_mut()));
        drop(c);
        let s = std::mem::replace(&mut self.session, Handle(null_mut()));
        drop(s);
    }
}
