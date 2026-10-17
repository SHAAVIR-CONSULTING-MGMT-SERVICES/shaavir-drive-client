//! Windows side of the stub: WinHTTP (net.rs), the machine (sys.rs) and the window (ui.rs).
//! Decisions are all in the portable crate (sdrive_setup::app); this module only does what it is told.
mod net;
mod sys;
mod ui;

use sdrive_setup::app::{self, Config, Ui};
use sdrive_setup::args::{self, ERROR_INVALID_PARAMETER};
use sdrive_setup::strings::Msg;
use sdrive_setup::tenant;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) fn config() -> Config {
    Config {
        product: tenant::PRODUCT_NAME.to_string(),
        host: tenant::UPDATES_HOST.to_string(),
        pubkey: tenant::MANIFEST_PUBKEY.to_string(),
        floor: tenant::VERSION_FLOOR.to_string(),
        dev_build: tenant::DEV_BUILD,
        sleep: std::thread::sleep,
    }
}

/// Log sink shared by the window and silent mode: kept in memory (for "Copy details") and, with
/// /log <file>, appended to that file.
pub(crate) struct Log {
    lines: Mutex<Vec<String>>,
    file: Option<Mutex<std::fs::File>>,
}

impl Log {
    pub fn new(path: Option<&str>) -> Log {
        let file = path.and_then(|p| OpenOptions::new().create(true).append(true).open(p).ok()).map(Mutex::new);
        Log { lines: Mutex::new(Vec::new()), file }
    }
    pub fn line(&self, s: &str) {
        let stamp = sys::timestamp();
        let l = format!("{stamp} {s}");
        if let Some(f) = &self.file {
            if let Ok(mut f) = f.lock() {
                let _ = writeln!(f, "{l}");
            }
        }
        if let Ok(mut v) = self.lines.lock() {
            v.push(l);
            if v.len() > 200 {
                v.remove(0);
            }
        }
    }
    pub fn text(&self) -> String {
        self.lines.lock().map(|v| v.join("\r\n")).unwrap_or_default()
    }
}

struct SilentUi<'a>(&'a Log);

impl Ui for SilentUi<'_> {
    fn status(&self, msg: Msg, args: &[String]) {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        self.0.line(&sdrive_setup::strings::text(msg, tenant::PRODUCT_NAME, &a));
    }
    fn progress(&self, _done: u64, _total: u64, _bps: f64) {}
    fn log(&self, line: &str) {
        self.0.line(line);
    }
}

pub fn run() -> u32 {
    let parsed = args::parse(std::env::args().skip(1));
    let a = match parsed {
        Ok(a) => a,
        Err(e) => {
            ui::message_box(&format!("{e}\n\n{}", args::USAGE));
            return ERROR_INVALID_PARAMETER;
        }
    };
    if a.help {
        ui::message_box(args::USAGE);
        return 0;
    }
    let log = Log::new(a.log.as_deref());
    if a.silent {
        let cancel = AtomicBool::new(false);
        let mut p = sys::WinPlatform::new(None);
        let r = app::run(&mut p, &SilentUi(&log), &config(), true, &cancel);
        if let app::Ending::Stopped { details, .. } = &r.ending {
            log.line(&format!("stopped: {details}"));
        }
        log.line(&format!("exit code {}", r.exit_code));
        return r.exit_code;
    }
    ui::run_window(log)
}
