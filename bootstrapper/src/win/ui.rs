//! The window: logo, title, one status line, a progress bar with "MB and time left", and buttons
//! (Cancel / Try again / Close, Copy details, Open Installed apps). Standard Win32 controls, so
//! keyboard (Tab, Enter, Esc) and screen readers work the usual way; per-monitor DPI aware (v2),
//! follows the light/dark app setting, and uses system colours under High Contrast.
//! The run itself happens on a worker thread (sdrive_setup::app::run); it posts events here.
use super::sys::{self, WinPlatform};
use super::{config, wide, Log};
use sdrive_setup::app::{self, Ending, Ui};
use sdrive_setup::strings::{text, Msg};
use sdrive_setup::tenant::PRODUCT_NAME;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows_sys::Win32::UI::Accessibility::{NotifyWinEvent, HIGHCONTRASTW};
use windows_sys::Win32::UI::Controls::{InitCommonControlsEx, SetWindowTheme, INITCOMMONCONTROLSEX, ICC_PROGRESS_CLASS, ICC_STANDARD_CLASSES};
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow, SystemParametersInfoForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_EVENT: u32 = WM_APP + 1;
const ID_PRIMARY: usize = 101;
const ID_COPY: usize = 102;
const ID_APPS: usize = 103;
const TIMER_CLOSE: usize = 1;
const PBM_SETRANGE32: u32 = WM_USER + 6;
const PBM_SETPOS: u32 = WM_USER + 2;
const PBM_SETMARQUEE: u32 = WM_USER + 10;
const PBS_SMOOTH: u32 = 0x01;
const PBS_MARQUEE: u32 = 0x08;
const EVENT_OBJECT_NAMECHANGE: u32 = 0x800C;
const OBJID_CLIENT: i32 = -4;
const CF_UNICODETEXT: u32 = 13;
const SS_NOPREFIX: u32 = 0x80;
const SS_ENDELLIPSIS: u32 = 0x4000;
const DWMWA_USE_IMMERSIVE_DARK_MODE: i32 = 20;

// Client size in DIPs
const W: i32 = 480;
const H: i32 = 214;

enum Event {
    Status(String),
    Progress(u64, u64, f64),
    Finished(app::Result_),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Working,
    Installing,
    Done,
}

#[derive(Clone, Copy)]
enum Primary {
    Cancel,
    TryAgain,
    Close,
}

#[derive(Clone, Copy)]
struct Ctrls {
    main: HWND,
    title: HWND,
    status: HWND,
    progress: HWND,
    detail: HWND,
    primary: HWND,
    copy: HWND,
    apps: HWND,
}

struct State {
    c: Cell<Option<Ctrls>>,
    phase: Cell<Phase>,
    primary: Cell<Primary>,
    exit_code: Cell<u32>,
    font: Cell<HFONT>,
    title_font: Cell<HFONT>,
    bg: Cell<HBRUSH>,
    bg_owned: Cell<bool>,
    dark: Cell<bool>,
    icon: Cell<HICON>,
    marquee: Cell<bool>,
    details: RefCell<String>,
    cancel: Arc<AtomicBool>,
    log: Arc<Log>,
    worker_running: Cell<bool>,
    close_pending: Cell<bool>,
}

thread_local! {
    static ST: RefCell<Option<std::rc::Rc<State>>> = const { RefCell::new(None) };
}

fn st() -> std::rc::Rc<State> {
    ST.with(|s| s.borrow().as_ref().cloned()).expect("window state")
}

fn set_text(h: HWND, s: &str) {
    let w = wide(s);
    unsafe { SetWindowTextW(h, w.as_ptr()) };
}

pub fn message_box(msg: &str) {
    let t = wide(&text(Msg::WindowTitle, PRODUCT_NAME, &[]));
    let m = wide(msg);
    unsafe { MessageBoxW(null_mut(), m.as_ptr(), t.as_ptr(), MB_OK | MB_ICONINFORMATION) };
}

struct PostUi {
    hwnd: isize,
    log: Arc<Log>,
}

impl PostUi {
    fn post(&self, e: Event) {
        let b = Box::into_raw(Box::new(e));
        if unsafe { PostMessageW(self.hwnd as HWND, WM_EVENT, 0, b as isize) } == 0 {
            // window already gone
            unsafe { drop(Box::from_raw(b)) };
        }
    }
}

impl Ui for PostUi {
    fn status(&self, msg: Msg, args: &[String]) {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        let t = text(msg, PRODUCT_NAME, &a);
        self.log.line(&t);
        self.post(Event::Status(t));
    }
    fn progress(&self, done: u64, total: u64, bps: f64) {
        self.post(Event::Progress(done, total, bps));
    }
    fn log(&self, line: &str) {
        self.log.line(line);
    }
}

fn start_worker(s: &State, hwnd: HWND) {
    s.cancel.store(false, Ordering::SeqCst);
    s.worker_running.set(true);
    s.phase.set(Phase::Working);
    let cancel = s.cancel.clone();
    let log = s.log.clone();
    let h = hwnd as isize;
    std::thread::spawn(move || {
        let ui = PostUi { hwnd: h, log };
        let mut p = WinPlatform::new(Some(h as HWND));
        let r = app::run(&mut p, &ui, &config(), false, &cancel);
        ui.post(Event::Finished(r));
    });
}

fn scale(v: i32, dpi: u32) -> i32 {
    (v as i64 * dpi as i64 / 96) as i32
}

fn make_fonts(s: &State, dpi: u32) {
    unsafe {
        let mut ncm: NONCLIENTMETRICSW = std::mem::zeroed();
        ncm.cbSize = std::mem::size_of::<NONCLIENTMETRICSW>() as u32;
        SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS, ncm.cbSize, &mut ncm as *mut _ as *mut c_void, 0, dpi);
        let old = s.font.replace(CreateFontIndirectW(&ncm.lfMessageFont));
        if !old.is_null() {
            DeleteObject(old);
        }
        let mut big = ncm.lfMessageFont;
        big.lfHeight = big.lfHeight * 3 / 2;
        big.lfWeight = 600;
        let old = s.title_font.replace(CreateFontIndirectW(&big));
        if !old.is_null() {
            DeleteObject(old);
        }
    }
}

fn high_contrast() -> bool {
    unsafe {
        let mut hc: HIGHCONTRASTW = std::mem::zeroed();
        hc.cbSize = std::mem::size_of::<HIGHCONTRASTW>() as u32;
        SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cbSize, &mut hc as *mut _ as *mut c_void, 0) != 0 && (hc.dwFlags & 1) != 0
    }
}

fn apps_use_dark() -> bool {
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let name = wide("AppsUseLightTheme");
    let mut v: u32 = 1;
    let mut n: u32 = 4;
    let rc = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, null_mut(), &mut v as *mut u32 as *mut c_void, &mut n) };
    rc == ERROR_SUCCESS && v == 0
}

fn apply_theme(s: &State) {
    let Some(c) = s.c.get() else { return };
    let dark = !high_contrast() && apps_use_dark();
    s.dark.set(dark);
    unsafe {
        let flag: BOOL = dark as BOOL;
        DwmSetWindowAttribute(c.main, DWMWA_USE_IMMERSIVE_DARK_MODE, &flag as *const BOOL as *const c_void, 4);
        let theme = wide(if dark { "DarkMode_Explorer" } else { "Explorer" });
        for b in [c.primary, c.copy, c.apps] {
            SetWindowTheme(b, theme.as_ptr(), null());
        }
        let was_dark = s.bg_owned.get();
        let old = s.bg.replace(if dark { CreateSolidBrush(rgb(32, 32, 32)) } else { GetSysColorBrush(COLOR_WINDOW) });
        if was_dark {
            DeleteObject(old); // only the brush we created; system brushes are never deleted
        }
        s.bg_owned.set(dark);
        InvalidateRect(c.main, null(), 1);
    }
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

fn layout(s: &State) {
    let Some(c) = s.c.get() else { return };
    let dpi = unsafe { GetDpiForWindow(c.main) }.max(96);
    let px = |v: i32| scale(v, dpi);
    let place = |h: HWND, x: i32, y: i32, w: i32, hh: i32| unsafe {
        SetWindowPos(h, null_mut(), px(x), px(y), px(w), px(hh), SWP_NOZORDER | SWP_NOACTIVATE);
    };
    place(c.title, 80, 24, W - 104, 30);
    place(c.status, 80, 58, W - 104, 42);
    place(c.progress, 24, 112, W - 48, 10);
    place(c.detail, 24, 128, W - 48, 22);
    place(c.primary, W - 24 - 112, H - 24 - 32, 112, 32);
    place(c.copy, W - 24 - 112 - 8 - 124, H - 24 - 32, 124, 32);
    place(c.apps, 24, H - 24 - 32, 168, 32);
    unsafe {
        for h in [c.status, c.detail, c.primary, c.copy, c.apps] {
            SendMessageW(h, WM_SETFONT, s.font.get() as usize, 1);
        }
        SendMessageW(c.title, WM_SETFONT, s.title_font.get() as usize, 1);
        let old = s.icon.replace(LoadImageW(GetModuleHandleW(null()), 1 as _, IMAGE_ICON, px(40), px(40), LR_DEFAULTCOLOR) as HICON);
        if !old.is_null() {
            DestroyIcon(old);
        }
        InvalidateRect(c.main, null(), 1);
    }
}

fn set_marquee(s: &State, on: bool) {
    let Some(c) = s.c.get() else { return };
    if s.marquee.get() == on {
        return;
    }
    s.marquee.set(on);
    unsafe {
        let style = GetWindowLongPtrW(c.progress, GWL_STYLE) as u32;
        let ns = if on { style | PBS_MARQUEE } else { style & !PBS_MARQUEE };
        SetWindowLongPtrW(c.progress, GWL_STYLE, ns as isize);
        SendMessageW(c.progress, PBM_SETMARQUEE, on as usize, 30);
        if !on {
            SendMessageW(c.progress, PBM_SETRANGE32, 0, 1000);
        }
    }
}

fn set_primary(s: &State, p: Primary) {
    let Some(c) = s.c.get() else { return };
    s.primary.set(p);
    let label = match p {
        Primary::Cancel => Msg::BtnCancel,
        Primary::TryAgain => Msg::BtnTryAgain,
        Primary::Close => Msg::BtnClose,
    };
    set_text(c.primary, &text(label, PRODUCT_NAME, &[]));
}

fn show(h: HWND, on: bool) {
    unsafe { ShowWindow(h, if on { SW_SHOW } else { SW_HIDE }) };
}

fn announce(h: HWND) {
    unsafe { NotifyWinEvent(EVENT_OBJECT_NAMECHANGE, h, OBJID_CLIENT, 0) };
}

fn on_event(s: &State, e: Event) {
    let Some(c) = s.c.get() else { return };
    match e {
        Event::Status(t) => {
            let installing = t == text(Msg::WaitingForPermission, PRODUCT_NAME, &[]) || t == text(Msg::Installing, PRODUCT_NAME, &[]);
            let downloading = t == text(Msg::Downloading, PRODUCT_NAME, &[]);
            set_text(c.status, &t);
            announce(c.status);
            if installing {
                s.phase.set(Phase::Installing);
                unsafe { EnableWindow(c.primary, 0) };
                set_text(c.detail, "");
            }
            set_marquee(s, !downloading);
        }
        Event::Progress(done, total, bps) => {
            set_marquee(s, false);
            let pos = if total > 0 { (done.saturating_mul(1000) / total) as usize } else { 0 };
            unsafe { SendMessageW(c.progress, PBM_SETPOS, pos, 0) };
            set_text(c.detail, &sdrive_setup::progress_line(done, total, bps));
        }
        Event::Finished(r) => {
            s.worker_running.set(false);
            s.phase.set(Phase::Done);
            s.exit_code.set(r.exit_code);
            set_marquee(s, false);
            unsafe { EnableWindow(c.primary, 1) };
            if s.close_pending.get() {
                unsafe { DestroyWindow(c.main) };
                return;
            }
            match r.ending {
                Ending::Opened => {
                    unsafe { SendMessageW(c.progress, PBM_SETPOS, 1000, 0) };
                    set_text(c.detail, "");
                    set_primary(s, Primary::Close);
                    unsafe { SetTimer(c.main, TIMER_CLOSE, 1500, None) };
                }
                Ending::Installed { reboot } => {
                    let m = if reboot { Msg::RebootNeeded } else { Msg::Done };
                    set_text(c.status, &text(m, PRODUCT_NAME, &[]));
                    announce(c.status);
                    set_text(c.detail, "");
                    set_primary(s, Primary::Close);
                }
                Ending::Stopped { msg, args, retry, apps, details } => {
                    let a: Vec<&str> = args.iter().map(String::as_str).collect();
                    let t = text(msg, PRODUCT_NAME, &a);
                    s.log.line(&format!("stopped: {t} [{details}]"));
                    *s.details.borrow_mut() = details;
                    set_text(c.status, &t);
                    announce(c.status);
                    set_text(c.detail, "");
                    unsafe { SendMessageW(c.progress, PBM_SETPOS, 0, 0) };
                    set_primary(s, if retry { Primary::TryAgain } else { Primary::Close });
                    show(c.copy, true);
                    show(c.apps, apps);
                }
            }
            unsafe { SetFocus(c.primary) };
        }
    }
}

fn on_primary(s: &State, hwnd: HWND) {
    let Some(c) = s.c.get() else { return };
    match (s.phase.get(), s.primary.get()) {
        (Phase::Working, _) => {
            s.cancel.store(true, Ordering::SeqCst);
            unsafe { EnableWindow(c.primary, 0) };
        }
        (Phase::Installing, _) => {} // msiexec has its own Cancel
        (Phase::Done, Primary::TryAgain) => {
            show(c.copy, false);
            show(c.apps, false);
            set_primary(s, Primary::Cancel);
            set_text(c.detail, "");
            start_worker(s, hwnd);
        }
        (Phase::Done, _) => unsafe {
            DestroyWindow(hwnd);
        },
    }
}

fn copy_details(s: &State, hwnd: HWND) {
    let body = format!(
        "{} setup\r\nResult: {}\r\nExit code: {}\r\n\r\n{}",
        PRODUCT_NAME,
        s.details.borrow(),
        s.exit_code.get(),
        s.log.text()
    );
    let w: Vec<u16> = body.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            return;
        }
        EmptyClipboard();
        let h = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2);
        if !h.is_null() {
            let p = GlobalLock(h) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                GlobalUnlock(h);
                SetClipboardData(CF_UNICODETEXT, h);
            }
        }
        CloseClipboard();
    }
    if let Some(c) = s.c.get() {
        set_text(c.detail, &text(Msg::DetailsCopied, PRODUCT_NAME, &[]));
        announce(c.detail);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(s) = ST.with(|x| x.borrow().as_ref().cloned()) else {
        return DefWindowProcW(hwnd, msg, wp, lp);
    };
    match msg {
        WM_EVENT => {
            let e = *Box::from_raw(lp as *mut Event);
            on_event(&s, e);
            0
        }
        WM_COMMAND => {
            let id = (wp & 0xffff) as usize;
            match id {
                ID_PRIMARY | 1 /* IDOK: Enter */ => on_primary(&s, hwnd),
                ID_COPY => copy_details(&s, hwnd),
                ID_APPS => sys::open_url("ms-settings:appsfeatures"),
                2 /* IDCANCEL: Esc */ => {
                    if s.phase.get() != Phase::Installing {
                        PostMessageW(hwnd, WM_CLOSE, 0, 0);
                    }
                }
                _ => {}
            }
            0
        }
        WM_TIMER if wp == TIMER_CLOSE => {
            KillTimer(hwnd, TIMER_CLOSE);
            DestroyWindow(hwnd);
            0
        }
        WM_CLOSE => {
            match s.phase.get() {
                Phase::Installing => {} // never leave msiexec half-watched; it has its own Cancel
                Phase::Working => {
                    s.close_pending.set(true);
                    s.cancel.store(true, Ordering::SeqCst);
                    if !s.worker_running.get() {
                        DestroyWindow(hwnd);
                    }
                }
                Phase::Done => {
                    DestroyWindow(hwnd);
                }
            }
            0
        }
        WM_CTLCOLORSTATIC => {
            let hdc = wp as HDC;
            if s.dark.get() {
                SetTextColor(hdc, rgb(240, 240, 240));
                SetBkColor(hdc, rgb(32, 32, 32));
            } else {
                SetTextColor(hdc, GetSysColor(COLOR_WINDOWTEXT));
                SetBkColor(hdc, GetSysColor(COLOR_WINDOW));
            }
            s.bg.get() as LRESULT
        }
        WM_ERASEBKGND => {
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut r);
            FillRect(wp as HDC, &r, s.bg.get());
            1
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            let icon = s.icon.get();
            if !icon.is_null() {
                let dpi = GetDpiForWindow(hwnd).max(96);
                DrawIconEx(hdc, scale(24, dpi), scale(24, dpi), icon, scale(40, dpi), scale(40, dpi), 0, null_mut(), DI_NORMAL);
            }
            EndPaint(hwnd, &ps);
            0
        }
        WM_DPICHANGED => {
            let r = &*(lp as *const RECT);
            SetWindowPos(hwnd, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            make_fonts(&s, ((wp >> 16) & 0xffff) as u32);
            layout(&s);
            0
        }
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE => {
            apply_theme(&s);
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn child(parent: HWND, class: &str, label: &str, style: u32, id: usize) -> HWND {
    let c = wide(class);
    let t = wide(label);
    unsafe {
        CreateWindowExW(0, c.as_ptr(), t.as_ptr(), WS_CHILD | WS_VISIBLE | style, 0, 0, 10, 10, parent, id as HMENU, GetModuleHandleW(null()), null())
    }
}

pub fn run_window(log: Log) -> u32 {
    let log = Arc::new(log);
    unsafe {
        let icc = INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_PROGRESS_CLASS | ICC_STANDARD_CLASSES };
        InitCommonControlsEx(&icc);
        let hinst = GetModuleHandleW(null());
        let class = wide("SdriveSetupWindow");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: LoadIconW(hinst, 1 as _),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
            hIconSm: null_mut(),
        };
        RegisterClassExW(&wc);
        let state = std::rc::Rc::new(State {
            c: Cell::new(None),
            phase: Cell::new(Phase::Working),
            primary: Cell::new(Primary::Cancel),
            exit_code: Cell::new(0),
            font: Cell::new(null_mut()),
            title_font: Cell::new(null_mut()),
            bg: Cell::new(GetSysColorBrush(COLOR_WINDOW)),
            bg_owned: Cell::new(false),
            dark: Cell::new(false),
            icon: Cell::new(null_mut()),
            marquee: Cell::new(false),
            details: RefCell::new(String::new()),
            cancel: Arc::new(AtomicBool::new(false)),
            log: log.clone(),
            worker_running: Cell::new(false),
            close_pending: Cell::new(false),
        });
        ST.with(|x| *x.borrow_mut() = Some(state.clone()));

        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let title = wide(&text(Msg::WindowTitle, PRODUCT_NAME, &[]));
        let main = CreateWindowExW(WS_EX_CONTROLPARENT, class.as_ptr(), title.as_ptr(), style, CW_USEDEFAULT, CW_USEDEFAULT, 100, 100, null_mut(), null_mut(), hinst, null());
        if main.is_null() {
            return 1603;
        }
        let dpi = GetDpiForWindow(main).max(96);
        let mut r = RECT { left: 0, top: 0, right: scale(W, dpi), bottom: scale(H, dpi) };
        AdjustWindowRectExForDpi(&mut r, style, 0, WS_EX_CONTROLPARENT, dpi);
        let (ww, wh) = (r.right - r.left, r.bottom - r.top);
        // centre on the work area of the window's monitor
        let mon = MonitorFromWindow(main, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(mon, &mut mi);
        let wa = mi.rcWork;
        SetWindowPos(main, null_mut(), wa.left + (wa.right - wa.left - ww) / 2, wa.top + (wa.bottom - wa.top - wh) / 2, ww, wh, SWP_NOZORDER);

        let ctrls = Ctrls {
            main,
            title: child(main, "STATIC", &text(Msg::WindowTitle, PRODUCT_NAME, &[]), SS_NOPREFIX, 0),
            status: child(main, "STATIC", &text(Msg::Checking, PRODUCT_NAME, &[]), SS_NOPREFIX, 0),
            progress: child(main, "msctls_progress32", "", PBS_SMOOTH, 0),
            detail: child(main, "STATIC", "", SS_NOPREFIX | SS_ENDELLIPSIS, 0),
            primary: child(main, "BUTTON", &text(Msg::BtnCancel, PRODUCT_NAME, &[]), WS_TABSTOP | BS_DEFPUSHBUTTON as u32, ID_PRIMARY),
            copy: child(main, "BUTTON", &text(Msg::BtnCopyDetails, PRODUCT_NAME, &[]), WS_TABSTOP, ID_COPY),
            apps: child(main, "BUTTON", &text(Msg::BtnOpenApps, PRODUCT_NAME, &[]), WS_TABSTOP, ID_APPS),
        };
        state.c.set(Some(ctrls));
        show(ctrls.copy, false);
        show(ctrls.apps, false);
        SendMessageW(ctrls.progress, PBM_SETRANGE32, 0, 1000);
        make_fonts(&state, dpi);
        apply_theme(&state);
        layout(&state);
        set_marquee(&state, true);
        ShowWindow(main, SW_SHOWNORMAL);
        UpdateWindow(main);
        SetFocus(ctrls.primary);
        start_worker(&state, main);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if IsDialogMessageW(main, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let code = state.exit_code.get();
        ST.with(|x| *x.borrow_mut() = None);
        code
    }
}
