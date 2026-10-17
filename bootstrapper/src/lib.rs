//! Download stub for the branded desktop client (D-160).
//!
//! Everything that decides something lives in these portable modules and is unit-tested on any OS;
//! `src/win/` only talks to Windows (WinHTTP, the window, the registry, msiexec).
//!
//! Flow (src/app.rs): read the machine -> fetch `latest.json` + `latest.json.sig` from the pinned update
//! host -> verify the minisign signature with the compiled-in key -> pick the installer for this
//! architecture -> decide (plan.rs) -> download with resume and retries, checking size and SHA-256 ->
//! run msiexec elevated -> open the app.

pub mod app;
pub mod args;
pub mod download;
pub mod exitcode;
pub mod manifest;
pub mod minisign;
pub mod plan;
pub mod strings;
pub mod tenant;
pub mod version;

/// Human-readable size and time for the progress line ("12.3 of 98.0 MB · about 40 s left").
pub fn progress_line(done: u64, total: u64, bytes_per_sec: f64) -> String {
    let mb = |b: u64| b as f64 / 1_000_000.0;
    let mut line = format!("{:.1} of {:.1} MB", mb(done), mb(total));
    if bytes_per_sec > 1.0 && total > done {
        let secs = ((total - done) as f64 / bytes_per_sec).ceil() as u64;
        let left = if secs < 60 {
            format!("about {secs} s left")
        } else if secs < 3600 {
            format!("about {} min left", secs.div_ceil(60))
        } else {
            format!("about {} h {} min left", secs / 3600, (secs % 3600) / 60)
        };
        line.push_str(" \u{00b7} ");
        line.push_str(&left);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::progress_line;

    #[test]
    fn progress_text() {
        assert_eq!(progress_line(0, 98_000_000, 0.0), "0.0 of 98.0 MB");
        assert_eq!(progress_line(12_300_000, 98_000_000, 2_000_000.0), "12.3 of 98.0 MB \u{00b7} about 43 s left");
        assert_eq!(progress_line(0, 98_000_000, 500_000.0), "0.0 of 98.0 MB \u{00b7} about 4 min left");
        assert_eq!(progress_line(98_000_000, 98_000_000, 5_000_000.0), "98.0 of 98.0 MB");
    }
}
