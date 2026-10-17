//! The whole run, start to finish, over two small interfaces: `Platform` (the machine) and `Ui` (the
//! window or the silent log). Windows implements both in src/win/; the tests use fakes, so every path
//! below - including "nothing is run unless the signature and hash check out" - is tested on any OS.
use crate::download::{self, DownloadError, Expect, Transport};
use crate::exitcode::{self, Outcome};
use crate::manifest::{self, Arch};
use crate::minisign::{self, PublicKey};
use crate::plan::{self, Decision, Facts};
use crate::strings::Msg;
use crate::version::Version;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const MANIFEST_PATH: &str = "/client/latest.json";
pub const SIGNATURE_PATH: &str = "/client/latest.json.sig";

/// Exit codes when nothing was handed to msiexec (silent mode returns these; IT tools know them).
pub mod code {
    pub const OK: u32 = 0;
    pub const CANCELLED: u32 = 1602; // ERROR_INSTALL_USEREXIT
    pub const BUSY: u32 = 1618; // ERROR_INSTALL_ALREADY_RUNNING
    pub const PLATFORM: u32 = 1633; // ERROR_INSTALL_PLATFORM_UNSUPPORTED (Windows version, architecture)
    pub const OTHER_CLIENT: u32 = 1638; // ERROR_PRODUCT_VERSION (another client must be removed first)
    pub const NETWORK: u32 = 1612; // ERROR_INSTALL_SOURCE_ABSENT (download server not reachable)
    pub const NOT_TRUSTED: u32 = 1330; // ERROR_INVALID_SIGNATURE family: manifest or installer failed checks
    pub const DISK: u32 = 112; // ERROR_DISK_FULL
    pub const INTERNAL: u32 = 1603; // ERROR_INSTALL_FAILURE
}

pub struct Config {
    pub product: String,
    pub host: String,
    pub pubkey: String,
    pub floor: String,
    pub dev_build: bool,
    /// Backoff between download retries (std::thread::sleep in the program, a no-op in tests)
    pub sleep: fn(Duration),
}

pub trait Platform {
    fn facts(&mut self) -> Facts;
    fn transport(&mut self) -> &mut dyn Transport;
    /// A new, empty, private folder for this run's download
    fn work_dir(&mut self) -> std::io::Result<PathBuf>;
    /// Runs msiexec elevated (UAC once) and waits; Err if it could not be started at all
    fn run_msiexec(&mut self, msi: &Path, log: &Path, silent: bool) -> Result<u32, String>;
    fn launch_app(&mut self) -> Result<(), String>;
}

pub trait Ui {
    fn status(&self, msg: Msg, args: &[String]);
    fn progress(&self, done: u64, total: u64, bytes_per_sec: f64);
    fn log(&self, line: &str);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    /// App opened (fresh install, upgrade, or already current)
    Opened,
    /// Installed (or current) and not opened: silent mode, or opening failed
    Installed { reboot: bool },
    /// Something stopped the run; `retry` = offer "Try again", `apps` = offer "Open Installed apps"
    Stopped { msg: Msg, args: Vec<String>, retry: bool, apps: bool, details: String },
}

pub struct Result_ {
    pub ending: Ending,
    pub exit_code: u32,
}

fn stop(msg: Msg, args: Vec<String>, retry: bool, apps: bool, details: String, code: u32) -> Result_ {
    Result_ { ending: Ending::Stopped { msg, args, retry, apps, details }, exit_code: code }
}

fn net_stop(e: &DownloadError, what: &str) -> Result_ {
    let details = format!("{what}: {e}");
    match e {
        DownloadError::Cancelled => stop(Msg::InstallCancelled, vec![], true, false, details, code::CANCELLED),
        DownloadError::Corrupt | DownloadError::TooLarge | DownloadError::Redirect(_) => {
            stop(Msg::DownloadCorrupt, vec![], true, false, details, code::NOT_TRUSTED)
        }
        DownloadError::DiskFull => stop(Msg::DiskFull, vec![], true, false, details, code::DISK),
        DownloadError::Io(_) => stop(Msg::InstallFailed, vec![code::INTERNAL.to_string()], true, false, details, code::INTERNAL),
        DownloadError::Network(_) | DownloadError::Status(_) => stop(Msg::NetworkFailed, vec![], true, false, details, code::NETWORK),
    }
}

pub fn run(p: &mut dyn Platform, ui: &dyn Ui, cfg: &Config, silent: bool, cancel: &AtomicBool) -> Result_ {
    ui.log(&format!("{} setup, update host {}", cfg.product, cfg.host));
    if cfg.dev_build {
        return stop(Msg::NoReleaseKey, vec![], false, false, "built without a manifest key".into(), code::INTERNAL);
    }
    let key = match PublicKey::parse(&cfg.pubkey) {
        Ok(k) => k,
        Err(e) => return stop(Msg::NoReleaseKey, vec![], false, false, format!("compiled-in key: {e:?}"), code::INTERNAL),
    };
    let floor = Version::parse_strict(&cfg.floor).unwrap_or(Version([0, 0, 0, 0]));

    ui.status(Msg::Checking, &[]);
    let facts = p.facts();
    ui.log(&format!("machine: {facts:?}"));

    // 1. manifest + signature, verified before anything else is read from it
    let body = match download::fetch_small(p.transport(), MANIFEST_PATH) {
        Ok(b) => b,
        Err(e) => return net_stop(&e, "latest.json"),
    };
    let sig = match download::fetch_small(p.transport(), SIGNATURE_PATH) {
        Ok(b) => b,
        Err(e) => return net_stop(&e, "latest.json.sig"),
    };
    let sig_text = String::from_utf8_lossy(&sig);
    let trusted = match minisign::verify(&key, &body, &sig_text) {
        Ok(t) => t,
        Err(e) => {
            return stop(Msg::ManifestInvalid, vec![], true, false, format!("signature: {e:?} (key {})", key.key_id_hex()), code::NOT_TRUSTED)
        }
    };
    ui.log(&format!("manifest signature ok (key {}, {trusted})", key.key_id_hex()));
    let m = match manifest::parse(&body, &cfg.product, &cfg.host) {
        Ok(m) => m,
        Err(e) => return stop(Msg::ManifestInvalid, vec![], true, false, format!("manifest: {e}"), code::NOT_TRUSTED),
    };
    ui.log(&format!("release {} for build >= {}", m.version, m.min_windows_build));

    // 2. decide
    let arch = match plan::decide(&facts, &m, floor) {
        Decision::Install { arch, upgrade_from } => {
            ui.log(&format!("install {arch:?} (upgrade from {upgrade_from:?})"));
            arch
        }
        Decision::OpenInstalled { installed } => {
            ui.log(&format!("installed {installed} is current"));
            if silent {
                return Result_ { ending: Ending::Installed { reboot: false }, exit_code: code::OK };
            }
            ui.status(Msg::AlreadyCurrent, &[installed.to_string()]);
            return open(p, ui);
        }
        Decision::WindowsTooOld { have, need } => {
            return stop(Msg::WindowsTooOld, vec![], false, false, format!("Windows build {have}, needs {need}"), code::PLATFORM)
        }
        Decision::UnsupportedArch(a) => return stop(Msg::UnsupportedArch, vec![], false, false, format!("architecture {a:?}"), code::PLATFORM),
        Decision::RemoveStockClient(name) => {
            return stop(Msg::RemoveStockClient, vec![name.clone()], true, true, format!("installed: {name}"), code::OTHER_CLIENT)
        }
        Decision::InstallerBusy => return stop(Msg::InstallerBusy, vec![], true, false, "Global\\_MSIExecute held".into(), code::BUSY),
        Decision::ManifestTooOld { manifest, floor } => {
            return stop(Msg::ManifestInvalid, vec![], true, false, format!("manifest {manifest} older than this setup's {floor}"), code::NOT_TRUSTED)
        }
    };
    let inst = m.installer_for(arch).expect("plan picked an arch the manifest has").clone();
    debug_assert!(matches!(arch, Arch::X64 | Arch::Arm64));

    // 3. download into a fresh folder, checked
    let dir = match p.work_dir() {
        Ok(d) => d,
        Err(e) => return stop(Msg::InstallFailed, vec![code::INTERNAL.to_string()], true, false, format!("work folder: {e}"), code::INTERNAL),
    };
    let msi = dir.join(&inst.file_name);
    ui.status(Msg::Downloading, &[]);
    let started = Instant::now();
    let mut last = Instant::now() - Duration::from_secs(1);
    let mut on_progress = |done: u64, total: u64| {
        if last.elapsed() >= Duration::from_millis(150) || done == total {
            last = Instant::now();
            let secs = started.elapsed().as_secs_f64().max(0.001);
            ui.progress(done, total, if secs > 2.0 { done as f64 / secs } else { 0.0 });
        }
    };
    let want = Expect { path: &inst.path, size: inst.size, sha256: inst.sha256 };
    let sleeper = cfg.sleep;
    let r = {
        let t = p.transport();
        download::download(t, &want, &msi, cancel, &mut on_progress, &sleeper)
    };
    if let Err(e) = r {
        download::cleanup(&dir);
        return net_stop(&e, &inst.file_name);
    }
    ui.status(Msg::Verifying, &[]);
    ui.log(&format!("{} verified: {} bytes, sha256 ok", inst.file_name, inst.size));
    if cancel.load(Ordering::SeqCst) {
        download::cleanup(&dir);
        return stop(Msg::InstallCancelled, vec![], true, false, "cancelled".into(), code::CANCELLED);
    }

    // 4. install (the only elevated step)
    ui.status(if silent { Msg::Installing } else { Msg::WaitingForPermission }, &[]);
    let log = dir.parent().unwrap_or(&dir).join(format!("{}-msiexec.log", inst.file_name.trim_end_matches(".msi")));
    let rc = p.run_msiexec(&msi, &log, silent);
    download::cleanup(&dir);
    let rc = match rc {
        Ok(c) => c,
        Err(e) => return stop(Msg::InstallFailed, vec![code::INTERNAL.to_string()], true, false, format!("msiexec did not start: {e}"), code::INTERNAL),
    };
    ui.log(&format!("msiexec exit code {rc} (log {})", log.display()));
    let outcome = exitcode::classify(rc);
    match outcome {
        Outcome::Installed if !silent => open(p, ui),
        Outcome::Installed | Outcome::InstalledRebootNeeded => {
            let reboot = outcome == Outcome::InstalledRebootNeeded;
            if reboot && !silent {
                ui.status(Msg::RebootNeeded, &[]);
            }
            Result_ { ending: Ending::Installed { reboot }, exit_code: rc }
        }
        Outcome::Cancelled => stop(Msg::InstallCancelled, vec![], true, false, format!("msiexec {rc}"), rc),
        Outcome::Busy => stop(Msg::InstallerBusy, vec![], true, false, format!("msiexec {rc}"), rc),
        Outcome::Failed(c) => stop(Msg::InstallFailed, vec![c.to_string()], true, false, format!("msiexec {c}, log {}", log.display()), rc),
    }
}

fn open(p: &mut dyn Platform, ui: &dyn Ui) -> Result_ {
    ui.status(Msg::Opening, &[]);
    match p.launch_app() {
        Ok(()) => Result_ { ending: Ending::Opened, exit_code: code::OK },
        Err(e) => {
            ui.log(&format!("could not open the app: {e}"));
            Result_ { ending: Ending::Installed { reboot: false }, exit_code: code::OK }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::Response;
    use crate::minisign::testkit::TestSigner;
    use crate::plan::MachineArch;
    use sha2::{Digest, Sha256};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::fs;

    const HOST: &str = "updates.example.com";

    struct MemTransport {
        files: HashMap<String, Vec<u8>>,
        requests: Vec<String>,
    }
    impl Transport for MemTransport {
        fn get(&mut self, path: &str, from: u64) -> Result<Response, String> {
            self.requests.push(path.to_string());
            match self.files.get(path) {
                Some(d) => {
                    let d = d[from as usize..].to_vec();
                    Ok(Response { status: if from > 0 { 206 } else { 200 }, content_length: Some(d.len() as u64), range: None, body: Box::new(std::io::Cursor::new(d)) })
                }
                None => Ok(Response { status: 404, content_length: Some(0), range: None, body: Box::new(std::io::empty()) }),
            }
        }
    }

    struct Fake {
        facts: Facts,
        t: MemTransport,
        dir: PathBuf,
        msiexec: Option<u32>,
        ran_msi: RefCell<Vec<Vec<u8>>>,
        launched: bool,
    }
    impl Platform for Fake {
        fn facts(&mut self) -> Facts {
            self.facts.clone()
        }
        fn transport(&mut self) -> &mut dyn Transport {
            &mut self.t
        }
        fn work_dir(&mut self) -> std::io::Result<PathBuf> {
            let d = self.dir.join("work");
            fs::create_dir_all(&d)?;
            Ok(d)
        }
        fn run_msiexec(&mut self, msi: &Path, _log: &Path, _silent: bool) -> Result<u32, String> {
            self.ran_msi.borrow_mut().push(fs::read(msi).unwrap());
            self.msiexec.ok_or_else(|| "no".to_string())
        }
        fn launch_app(&mut self) -> Result<(), String> {
            self.launched = true;
            Ok(())
        }
    }

    struct NullUi(RefCell<Vec<Msg>>);
    impl Ui for NullUi {
        fn status(&self, m: Msg, _a: &[String]) {
            self.0.borrow_mut().push(m);
        }
        fn progress(&self, _d: u64, _t: u64, _b: f64) {}
        fn log(&self, _l: &str) {}
    }

    fn setup(name: &str, msi: &[u8], listed_hash: Option<[u8; 32]>, sign_with: &TestSigner) -> (Fake, Config) {
        let signer = TestSigner::new(7);
        let h: [u8; 32] = listed_hash.unwrap_or_else(|| Sha256::digest(msi).into());
        let hex: String = h.iter().map(|b| format!("{b:02x}")).collect();
        let man = crate::manifest::sample("34.0.6.1", HOST)
            .replace("\"size\": 1000", &format!("\"size\": {}", msi.len()))
            .replace(&"ab".repeat(32), &hex);
        let sig = sign_with.sign(man.as_bytes(), "timestamp:1 file:latest.json");
        let mut files = HashMap::new();
        files.insert(MANIFEST_PATH.to_string(), man.into_bytes());
        files.insert(SIGNATURE_PATH.to_string(), sig.into_bytes());
        files.insert("/client/ExampleDrive-34.0.6.1-x64.msi".to_string(), msi.to_vec());
        let dir = std::env::temp_dir().join(format!("sdsetup-app-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let fake = Fake {
            facts: Facts { windows_build: 22631, arch: MachineArch::X64, stock_client: None, installed: None, msi_busy: false },
            t: MemTransport { files, requests: vec![] },
            dir,
            msiexec: Some(0),
            ran_msi: RefCell::new(vec![]),
            launched: false,
        };
        let cfg = Config { product: "Example Drive".into(), host: HOST.into(), pubkey: signer.public_line(), floor: "34.0.5.1".into(), dev_build: false, sleep: |_| {} };
        (fake, cfg)
    }

    fn go(f: &mut Fake, cfg: &Config, silent: bool) -> Result_ {
        let ui = NullUi(RefCell::new(vec![]));
        run(f, &ui, cfg, silent, &AtomicBool::new(false))
    }

    #[test]
    fn happy_path_installs_then_opens_and_cleans_up() {
        let msi = b"MSI-BYTES".repeat(1000);
        let (mut f, cfg) = setup("happy", &msi, None, &TestSigner::new(7));
        let r = go(&mut f, &cfg, false);
        assert_eq!(r.ending, Ending::Opened);
        assert_eq!(r.exit_code, 0);
        assert_eq!(f.ran_msi.borrow()[0], msi, "msiexec got exactly the verified bytes");
        assert!(f.launched);
        assert!(!f.dir.join("work").exists(), "download folder removed");
    }

    #[test]
    fn wrong_signing_key_runs_nothing() {
        let msi = b"x".repeat(100);
        let (mut f, cfg) = setup("wrongkey", &msi, None, &TestSigner::new(9));
        let r = go(&mut f, &cfg, false);
        assert!(matches!(r.ending, Ending::Stopped { msg: Msg::ManifestInvalid, .. }));
        assert_eq!(r.exit_code, code::NOT_TRUSTED);
        assert!(f.ran_msi.borrow().is_empty());
        assert_eq!(f.t.requests.len(), 2, "installer never even requested");
    }

    #[test]
    fn tampered_installer_runs_nothing() {
        let msi = b"x".repeat(100);
        let (mut f, cfg) = setup("tampered", &msi, Some([1u8; 32]), &TestSigner::new(7));
        let r = go(&mut f, &cfg, false);
        assert!(matches!(r.ending, Ending::Stopped { msg: Msg::DownloadCorrupt, .. }));
        assert!(f.ran_msi.borrow().is_empty());
        assert!(!f.dir.join("work").exists());
    }

    #[test]
    fn silent_returns_msiexec_code_and_does_not_open() {
        let msi = b"x".repeat(100);
        let (mut f, cfg) = setup("silent", &msi, None, &TestSigner::new(7));
        f.msiexec = Some(3010);
        let r = go(&mut f, &cfg, true);
        assert_eq!(r.exit_code, 3010);
        assert_eq!(r.ending, Ending::Installed { reboot: true });
        assert!(!f.launched);
        f.msiexec = Some(1603);
        let r = go(&mut f, &cfg, true);
        assert_eq!(r.exit_code, 1603);
    }

    #[test]
    fn current_version_just_opens_without_download() {
        let msi = b"x".repeat(100);
        let (mut f, cfg) = setup("current", &msi, None, &TestSigner::new(7));
        f.facts.installed = Some(Version([34, 0, 6, 1]));
        let r = go(&mut f, &cfg, false);
        assert_eq!(r.ending, Ending::Opened);
        assert_eq!(f.t.requests.len(), 2);
        assert!(f.ran_msi.borrow().is_empty());
    }

    #[test]
    fn stock_client_offers_apps_button() {
        let msi = b"x".repeat(100);
        let (mut f, cfg) = setup("stock", &msi, None, &TestSigner::new(7));
        f.facts.stock_client = Some("Nextcloud".into());
        let r = go(&mut f, &cfg, false);
        assert!(matches!(r.ending, Ending::Stopped { msg: Msg::RemoveStockClient, apps: true, .. }));
        assert_eq!(r.exit_code, code::OTHER_CLIENT);
    }

    #[test]
    fn uac_declined_is_cancelled() {
        let msi = b"x".repeat(100);
        let (mut f, cfg) = setup("uac", &msi, None, &TestSigner::new(7));
        f.msiexec = Some(1223);
        let r = go(&mut f, &cfg, false);
        assert!(matches!(r.ending, Ending::Stopped { msg: Msg::InstallCancelled, retry: true, .. }));
    }

    #[test]
    fn dev_build_refuses() {
        let msi = b"x".repeat(100);
        let (mut f, mut cfg) = setup("dev", &msi, None, &TestSigner::new(7));
        cfg.dev_build = true;
        let r = go(&mut f, &cfg, false);
        assert!(matches!(r.ending, Ending::Stopped { msg: Msg::NoReleaseKey, .. }));
        assert!(f.t.requests.is_empty());
    }

    #[test]
    fn offline() {
        let msi = b"x".repeat(100);
        let (mut f, cfg) = setup("offline", &msi, None, &TestSigner::new(7));
        f.t.files.clear();
        let r = go(&mut f, &cfg, false);
        assert!(matches!(r.ending, Ending::Stopped { .. }));
        assert_eq!(r.exit_code, code::NETWORK);
    }
}
