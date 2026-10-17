//! The machine: Windows build and architecture, what is installed, msiexec, opening the app.
use super::net::WinHttp;
use super::wide;
use sdrive_setup::app::Platform;
use sdrive_setup::download::{Response, Transport};
use sdrive_setup::plan::{Facts, MachineArch};
use sdrive_setup::tenant;
use sdrive_setup::version::Version;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::ApplicationInstallationAndServicing::{MsiEnumRelatedProductsW, MsiGetProductInfoW};
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemDirectoryW};
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteExW, ShellExecuteW, SHELLEXECUTEINFOW};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Display names of the upstream (unbranded) desktop client. Upstream's name, not a tenant value.
const STOCK_CLIENT_NAMES: &[&str] = &["Nextcloud"];

const SEE_MASK_NOCLOSEPROCESS: u32 = 0x0000_0040;
const SEE_MASK_NOASYNC: u32 = 0x0000_0100;
const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;

pub fn elevated() -> bool {
    unsafe { IsUserAnAdmin() != 0 }
}

pub fn timestamp() -> String {
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

fn reg_string(root: HKEY, sub: &str, name: &str, sam: u32) -> Option<String> {
    let subw = wide(sub);
    let mut key: HKEY = null_mut();
    if unsafe { RegOpenKeyExW(root, subw.as_ptr(), 0, KEY_READ | sam, &mut key) } != ERROR_SUCCESS {
        return None;
    }
    let v = reg_value(key, name);
    unsafe { RegCloseKey(key) };
    v
}

fn reg_value(key: HKEY, name: &str) -> Option<String> {
    let namew = wide(name);
    let mut buf = vec![0u16; 1024];
    let mut bytes: u32 = (buf.len() * 2) as u32;
    let rc = unsafe { RegGetValueW(key, null(), namew.as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr() as *mut c_void, &mut bytes) };
    if rc != ERROR_SUCCESS {
        return None;
    }
    let n = (bytes as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..n]))
}

fn windows_build() -> u32 {
    reg_string(HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuildNumber", KEY_WOW64_64KEY)
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

fn machine_arch() -> MachineArch {
    let mut process: u16 = 0;
    let mut native: u16 = 0;
    let ok = unsafe { IsWow64Process2(GetCurrentProcess(), &mut process, &mut native) };
    if ok == 0 {
        return if cfg!(target_arch = "x86_64") { MachineArch::X64 } else { MachineArch::Other };
    }
    match native {
        0x8664 => MachineArch::X64,
        0xAA64 => MachineArch::Arm64,
        0x014c => MachineArch::X86,
        _ => MachineArch::Other,
    }
}

/// Our product, found by its MSI upgrade code (never by name, which a tenant may change).
fn installed_product() -> Option<(Version, String)> {
    let code = format!("{{{}}}", tenant::MSI_UPGRADE_CODE.trim_matches(|c| c == '{' || c == '}'));
    let codew = wide(&code);
    let mut best: Option<(Version, String)> = None;
    for i in 0..16u32 {
        let mut product = [0u16; 39];
        if unsafe { MsiEnumRelatedProductsW(codew.as_ptr(), 0, i, product.as_mut_ptr()) } != ERROR_SUCCESS {
            break;
        }
        let ver = msi_info(&product, "VersionString").and_then(|v| Version::parse_installed(&v));
        let loc = msi_info(&product, "InstallLocation").unwrap_or_default();
        if let Some(v) = ver {
            if best.as_ref().is_none_or(|(b, _)| v > *b) {
                best = Some((v, loc));
            }
        }
    }
    best
}

fn msi_info(product: &[u16; 39], attr: &str) -> Option<String> {
    let a = wide(attr);
    let mut buf = vec![0u16; 1024];
    let mut len: u32 = buf.len() as u32;
    if unsafe { MsiGetProductInfoW(product.as_ptr(), a.as_ptr(), buf.as_mut_ptr(), &mut len) } != ERROR_SUCCESS {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

/// The stock client, by its uninstall entry (machine-wide 64/32-bit and per-user).
fn stock_client() -> Option<String> {
    let roots: [(HKEY, u32); 3] = [(HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY), (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY), (HKEY_CURRENT_USER, 0)];
    let base = wide(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall");
    for (root, sam) in roots {
        let mut key: HKEY = null_mut();
        if unsafe { RegOpenKeyExW(root, base.as_ptr(), 0, KEY_READ | sam, &mut key) } != ERROR_SUCCESS {
            continue;
        }
        let mut i = 0u32;
        loop {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            let rc = unsafe { RegEnumKeyExW(key, i, name.as_mut_ptr(), &mut len, null(), null_mut(), null_mut(), null_mut()) };
            if rc != ERROR_SUCCESS {
                break;
            }
            i += 1;
            let mut sub: HKEY = null_mut();
            if unsafe { RegOpenKeyExW(key, name.as_ptr(), 0, KEY_READ | sam, &mut sub) } != ERROR_SUCCESS {
                continue;
            }
            let display = reg_value(sub, "DisplayName").unwrap_or_default();
            let publisher = reg_value(sub, "Publisher").unwrap_or_default();
            unsafe { RegCloseKey(sub) };
            if display == tenant::PRODUCT_NAME {
                continue;
            }
            let hit = STOCK_CLIENT_NAMES.iter().any(|n| {
                (display == *n || display.starts_with(&format!("{n} "))) && publisher.contains(n)
            });
            if hit {
                unsafe { RegCloseKey(key) };
                return Some(display);
            }
        }
        unsafe { RegCloseKey(key) };
    }
    None
}

fn msi_busy() -> bool {
    let name = wide("Global\\_MSIExecute");
    let h = unsafe { OpenMutexW(SYNCHRONIZE_ACCESS, 0, name.as_ptr()) };
    if h.is_null() {
        return false;
    }
    unsafe { CloseHandle(h) };
    true
}

fn system_dir() -> PathBuf {
    let mut buf = [0u16; 260];
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return PathBuf::from(r"C:\Windows\System32");
    }
    PathBuf::from(String::from_utf16_lossy(&buf[..n]))
}

/// ShellExecuteEx and wait; Ok(exit code). The UAC "No" comes back as Ok(1223).
fn shell_wait(hwnd: HWND, verb: &str, file: &Path, params: &str) -> Result<u32, String> {
    let verbw = wide(verb);
    let filew = wide(&file.display().to_string());
    let paramw = wide(params);
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.hwnd = hwnd;
    info.lpVerb = verbw.as_ptr();
    info.lpFile = filew.as_ptr();
    info.lpParameters = paramw.as_ptr();
    info.nShow = SW_SHOWNORMAL;
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let e = unsafe { GetLastError() };
        if e == ERROR_CANCELLED {
            return Ok(ERROR_CANCELLED);
        }
        return Err(format!("could not start {} (error {e})", file.display()));
    }
    if info.hProcess.is_null() {
        return Err("no process handle".into());
    }
    let mut code: u32 = 0;
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
    }
    Ok(code)
}

pub fn open_url(target: &str) {
    let open = wide("open");
    let t = wide(target);
    unsafe { ShellExecuteW(null_mut(), open.as_ptr(), t.as_ptr(), null(), null(), SW_SHOWNORMAL) };
}

struct Unreachable(String);
impl Transport for Unreachable {
    fn get(&mut self, _p: &str, _f: u64) -> Result<Response, String> {
        Err(self.0.clone())
    }
}

pub struct WinPlatform {
    hwnd: isize,
    http: Option<Box<dyn Transport>>,
    install_location: String,
}

impl WinPlatform {
    pub fn new(hwnd: Option<HWND>) -> WinPlatform {
        WinPlatform { hwnd: hwnd.map_or(0, |h| h as isize), http: None, install_location: String::new() }
    }
}

impl Platform for WinPlatform {
    fn facts(&mut self) -> Facts {
        let installed = installed_product();
        if let Some((_, loc)) = &installed {
            self.install_location = loc.clone();
        }
        Facts {
            windows_build: windows_build(),
            arch: machine_arch(),
            stock_client: stock_client(),
            installed: installed.map(|(v, _)| v),
            msi_busy: msi_busy(),
        }
    }

    fn transport(&mut self) -> &mut dyn Transport {
        if self.http.is_none() {
            self.http = Some(match WinHttp::new(tenant::UPDATES_HOST) {
                Ok(t) => Box::new(t),
                Err(e) => Box::new(Unreachable(e)),
            });
        }
        self.http.as_deref_mut().unwrap()
    }

    fn work_dir(&mut self) -> std::io::Result<PathBuf> {
        let base = std::env::temp_dir();
        for n in 0..100u32 {
            let d = base.join(format!("{}Setup-{}-{}", tenant::SHORTNAME, std::process::id(), n));
            match std::fs::create_dir(&d) {
                Ok(()) => return Ok(d),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::other("no free folder name"))
    }

    fn run_msiexec(&mut self, msi: &Path, log: &Path, silent: bool) -> Result<u32, String> {
        let ui = if silent { "/qn" } else { "/qb!" };
        let params = format!("/i \"{}\" {ui} /norestart /l*v \"{}\"", msi.display(), log.display());
        let exe = system_dir().join("msiexec.exe");
        // Already elevated (IT tooling, or "Run as administrator"): no second prompt.
        let verb = if unsafe { IsUserAnAdmin() } != 0 { "open" } else { "runas" };
        shell_wait(self.hwnd as HWND, verb, &exe, &params)
    }

    fn launch_app(&mut self) -> Result<(), String> {
        // Never start the sync client elevated (someone used "Run as administrator"): it would run, sync
        // and update as admin. The person opens it from the Start menu instead (audit M1).
        if unsafe { IsUserAnAdmin() } != 0 {
            return Err("setup is running elevated; not opening the app from here".into());
        }
        let loc = if self.install_location.is_empty() {
            installed_product().map(|(_, l)| l).unwrap_or_default()
        } else {
            self.install_location.clone()
        };
        let mut candidates = Vec::new();
        if !loc.is_empty() {
            candidates.push(PathBuf::from(&loc).join(format!("{}.exe", tenant::APP_EXECUTABLE)));
        }
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            candidates.push(PathBuf::from(pf).join(tenant::PRODUCT_NAME).join(format!("{}.exe", tenant::APP_EXECUTABLE)));
        }
        for c in candidates {
            if c.is_file() {
                open_url(&c.display().to_string());
                return Ok(());
            }
        }
        Err("the app was not found where the installer puts it".into())
    }
}
