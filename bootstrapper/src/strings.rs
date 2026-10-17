//! Every text the stub shows, in one table (English first; add a language by adding a column).
//! `{product}` is replaced with the tenant's product name, `{0}`/`{1}` with values.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    WindowTitle,
    Checking,
    Downloading,
    Verifying,
    WaitingForPermission,
    Installing,
    Opening,
    Done,
    RebootNeeded,
    InstallCancelled,
    InstallerBusy,
    InstallFailed,
    WindowsTooOld,
    UnsupportedArch,
    RemoveStockClient,
    AlreadyCurrent,
    NetworkFailed,
    ManifestInvalid,
    DownloadCorrupt,
    NoReleaseKey,
    DiskFull,
    NeedsElevation,
    BtnCancel,
    BtnTryAgain,
    BtnCopyDetails,
    BtnClose,
    BtnOpenApps,
    DetailsCopied,
    ConfirmCancel,
}

pub fn en(m: Msg) -> &'static str {
    match m {
        Msg::WindowTitle => "{product} Setup",
        Msg::Checking => "Checking for the latest version\u{2026}",
        Msg::Downloading => "Downloading {product}\u{2026}",
        Msg::Verifying => "Checking the download\u{2026}",
        Msg::WaitingForPermission => "Windows will ask for permission to install. Choose Yes.",
        Msg::Installing => "Installing\u{2026}",
        Msg::Opening => "Opening {product}",
        Msg::Done => "{product} is installed. Open it from the Start menu.",
        Msg::RebootNeeded => "{product} is installed. Restart your computer to finish.",
        Msg::InstallCancelled => "Installation was cancelled. Nothing was changed.",
        Msg::InstallerBusy => "Another installation is running. Wait until it finishes, then try again.",
        Msg::InstallFailed => "Installation did not finish (error {0}). Try again, or copy the details for support.",
        Msg::WindowsTooOld => "{product} needs Windows 10 version 22H2 or Windows 11. Update Windows, then try again.",
        Msg::UnsupportedArch => "{product} for Windows needs a 64-bit Intel or AMD PC. Use {product} in your browser instead.",
        Msg::RemoveStockClient => "The {0} desktop app is installed. Remove it first (Apps > Installed apps), then run this setup again. Your files stay on the server.",
        Msg::AlreadyCurrent => "You already have the latest {product} ({0}).",
        Msg::NetworkFailed => "Could not reach the download server. Check your internet connection, then try again.",
        Msg::ManifestInvalid => "The download information could not be verified, so nothing was installed. Try again later or contact support.",
        Msg::DownloadCorrupt => "The download was damaged or changed, so it was not installed. Try again.",
        Msg::NoReleaseKey => "This is a test build of the setup program. It cannot install anything.",
        Msg::DiskFull => "There is not enough free disk space. Free some space, then try again.",
        Msg::NeedsElevation => "Silent install needs an administrator command prompt.",
        Msg::BtnCancel => "Cancel",
        Msg::BtnTryAgain => "Try again",
        Msg::BtnCopyDetails => "Copy details",
        Msg::BtnClose => "Close",
        Msg::BtnOpenApps => "Open Installed apps",
        Msg::DetailsCopied => "Details copied.",
        Msg::ConfirmCancel => "Stop the download?",
    }
}

/// Fills `{product}`, `{0}` and `{1}`.
pub fn text(m: Msg, product: &str, args: &[&str]) -> String {
    let mut s = en(m).replace("{product}", product);
    for (i, a) in args.iter().enumerate() {
        s = s.replace(&format!("{{{i}}}"), a);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill() {
        assert_eq!(text(Msg::Downloading, "Example Drive", &[]), "Downloading Example Drive\u{2026}");
        assert_eq!(text(Msg::InstallFailed, "X", &["1603"]), "Installation did not finish (error 1603). Try again, or copy the details for support.");
        assert_eq!(text(Msg::AlreadyCurrent, "X", &["34.0.5.1"]), "You already have the latest X (34.0.5.1).");
    }

    #[test]
    fn no_unfilled_tokens_in_static_messages() {
        for m in [Msg::Checking, Msg::Installing, Msg::BtnCancel, Msg::Done] {
            assert!(!text(m, "X", &[]).contains('{'));
        }
    }
}
