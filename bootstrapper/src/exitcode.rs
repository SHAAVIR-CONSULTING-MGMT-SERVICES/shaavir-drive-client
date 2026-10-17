//! msiexec exit codes -> what the person is told (D-160). Silent mode (/S) returns the msiexec code
//! itself, so IT tools see the standard values.
use crate::strings::Msg;

pub const ERROR_SUCCESS: u32 = 0;
pub const ERROR_INSTALL_USEREXIT: u32 = 1602;
pub const ERROR_INSTALL_ALREADY_RUNNING: u32 = 1618;
pub const ERROR_SUCCESS_REBOOT_INITIATED: u32 = 1641;
pub const ERROR_SUCCESS_REBOOT_REQUIRED: u32 = 3010;
/// ShellExecuteEx fails with ERROR_CANCELLED when the person says No at the UAC prompt.
pub const ERROR_CANCELLED: u32 = 1223;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Installed,
    InstalledRebootNeeded,
    Cancelled,
    Busy,
    Failed(u32),
}

pub fn classify(code: u32) -> Outcome {
    match code {
        ERROR_SUCCESS => Outcome::Installed,
        ERROR_SUCCESS_REBOOT_REQUIRED | ERROR_SUCCESS_REBOOT_INITIATED => Outcome::InstalledRebootNeeded,
        ERROR_INSTALL_USEREXIT | ERROR_CANCELLED => Outcome::Cancelled,
        ERROR_INSTALL_ALREADY_RUNNING => Outcome::Busy,
        other => Outcome::Failed(other),
    }
}

impl Outcome {
    pub fn message(self) -> Msg {
        match self {
            Outcome::Installed => Msg::Opening,
            Outcome::InstalledRebootNeeded => Msg::RebootNeeded,
            Outcome::Cancelled => Msg::InstallCancelled,
            Outcome::Busy => Msg::InstallerBusy,
            Outcome::Failed(_) => Msg::InstallFailed,
        }
    }
    pub fn success(self) -> bool {
        matches!(self, Outcome::Installed | Outcome::InstalledRebootNeeded)
    }
    /// "Try again" makes sense for these
    pub fn retryable(self) -> bool {
        matches!(self, Outcome::Cancelled | Outcome::Busy | Outcome::Failed(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping() {
        assert_eq!(classify(0), Outcome::Installed);
        assert_eq!(classify(3010), Outcome::InstalledRebootNeeded);
        assert_eq!(classify(1641), Outcome::InstalledRebootNeeded);
        assert_eq!(classify(1602), Outcome::Cancelled);
        assert_eq!(classify(1223), Outcome::Cancelled);
        assert_eq!(classify(1618), Outcome::Busy);
        assert_eq!(classify(1603), Outcome::Failed(1603));
        assert!(classify(3010).success() && !classify(3010).retryable());
        assert!(classify(1603).retryable() && !classify(1603).success());
        assert_eq!(classify(1618).message(), Msg::InstallerBusy);
        assert_eq!(classify(0).message(), Msg::Opening);
    }
}
