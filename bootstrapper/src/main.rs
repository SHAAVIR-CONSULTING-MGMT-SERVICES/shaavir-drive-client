//! Entry point. The Windows program is in src/win/; elsewhere this binary only explains itself.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    std::process::exit(win::run() as i32);
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This setup program runs on Windows only.");
    std::process::exit(1);
}
