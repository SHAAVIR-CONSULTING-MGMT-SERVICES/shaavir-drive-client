//! What to do on this machine, decided before anything is downloaded (D-160). Pure: the Windows layer
//! gathers the facts, this module decides, and the tests pin every branch.
use crate::manifest::{Arch, Manifest};
use crate::version::Version;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineArch {
    X64,
    Arm64,
    X86,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub windows_build: u32,
    pub arch: MachineArch,
    /// The stock (unbranded) desktop client of the upstream project, by display name
    pub stock_client: Option<String>,
    /// Our product's installed version (by MSI upgrade code), if any
    pub installed: Option<Version>,
    /// Another installation is running (Global\_MSIExecute is held)
    pub msi_busy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Windows too old for this release
    WindowsTooOld { have: u32, need: u32 },
    /// 32-bit Windows or an architecture the release does not ship
    UnsupportedArch(MachineArch),
    /// The stock client must be removed first (doc 05 §2: both cannot run side by side)
    RemoveStockClient(String),
    /// Another install is running; try again later
    InstallerBusy,
    /// The release is older than the stub's floor: a replayed old manifest (D-161)
    ManifestTooOld { manifest: Version, floor: Version },
    /// Installed copy is the same or newer: just open it (never downgrade, doc 05 §4.8)
    OpenInstalled { installed: Version },
    /// Download and install this arch's installer
    Install { arch: Arch, upgrade_from: Option<Version> },
}

pub fn decide(f: &Facts, m: &Manifest, floor: Version) -> Decision {
    if m.version < floor {
        return Decision::ManifestTooOld { manifest: m.version, floor };
    }
    if f.windows_build < m.min_windows_build {
        return Decision::WindowsTooOld { have: f.windows_build, need: m.min_windows_build };
    }
    let arch = match f.arch {
        MachineArch::X64 => Arch::X64,
        MachineArch::Arm64 if m.arm64.is_some() => Arch::Arm64,
        other => return Decision::UnsupportedArch(other),
    };
    if let Some(installed) = f.installed {
        if installed >= m.version {
            return Decision::OpenInstalled { installed };
        }
    }
    if let Some(name) = &f.stock_client {
        return Decision::RemoveStockClient(name.clone());
    }
    if f.msi_busy {
        return Decision::InstallerBusy;
    }
    Decision::Install { arch, upgrade_from: f.installed }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{parse, sample};

    const H: &str = "updates.example.com";
    fn m(v: &str) -> Manifest {
        parse(sample(v, H).as_bytes(), "Example Drive", H).unwrap()
    }
    fn facts() -> Facts {
        Facts { windows_build: 22631, arch: MachineArch::X64, stock_client: None, installed: None, msi_busy: false }
    }
    const FLOOR: Version = Version([34, 0, 5, 1]);

    #[test]
    fn fresh_install() {
        assert_eq!(decide(&facts(), &m("34.0.5.1"), FLOOR), Decision::Install { arch: Arch::X64, upgrade_from: None });
    }

    #[test]
    fn upgrade() {
        let f = Facts { installed: Some(Version([34, 0, 5, 1])), ..facts() };
        assert_eq!(
            decide(&f, &m("34.0.6.1"), FLOOR),
            Decision::Install { arch: Arch::X64, upgrade_from: Some(Version([34, 0, 5, 1])) }
        );
    }

    #[test]
    fn same_or_newer_installed_just_opens() {
        let f = Facts { installed: Some(Version([34, 0, 5, 1])), ..facts() };
        assert_eq!(decide(&f, &m("34.0.5.1"), FLOOR), Decision::OpenInstalled { installed: Version([34, 0, 5, 1]) });
        let f = Facts { installed: Some(Version([35, 0, 0, 1])), ..facts() };
        assert!(matches!(decide(&f, &m("34.0.5.1"), FLOOR), Decision::OpenInstalled { .. }), "never a downgrade");
    }

    #[test]
    fn replayed_old_manifest() {
        assert_eq!(
            decide(&facts(), &m("34.0.4.9"), FLOOR),
            Decision::ManifestTooOld { manifest: Version([34, 0, 4, 9]), floor: FLOOR }
        );
    }

    #[test]
    fn old_windows() {
        let f = Facts { windows_build: 19044, ..facts() };
        assert_eq!(decide(&f, &m("34.0.5.1"), FLOOR), Decision::WindowsTooOld { have: 19044, need: 19045 });
    }

    #[test]
    fn arch() {
        let f = Facts { arch: MachineArch::Arm64, ..facts() };
        assert_eq!(decide(&f, &m("34.0.5.1"), FLOOR), Decision::UnsupportedArch(MachineArch::Arm64));
        let f = Facts { arch: MachineArch::X86, ..facts() };
        assert_eq!(decide(&f, &m("34.0.5.1"), FLOOR), Decision::UnsupportedArch(MachineArch::X86));
    }

    #[test]
    fn stock_client_blocks_install_but_not_open() {
        let f = Facts { stock_client: Some("Nextcloud".into()), ..facts() };
        assert_eq!(decide(&f, &m("34.0.5.1"), FLOOR), Decision::RemoveStockClient("Nextcloud".into()));
        let f = Facts { stock_client: Some("Nextcloud".into()), installed: Some(Version([34, 0, 5, 1])), ..facts() };
        assert!(matches!(decide(&f, &m("34.0.5.1"), FLOOR), Decision::OpenInstalled { .. }));
    }

    #[test]
    fn busy() {
        let f = Facts { msi_busy: true, ..facts() };
        assert_eq!(decide(&f, &m("34.0.5.1"), FLOOR), Decision::InstallerBusy);
    }
}
