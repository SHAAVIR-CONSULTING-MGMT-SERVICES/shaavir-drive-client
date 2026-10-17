//! Four-part versions (a.b.c.d), as the MSI and the update feed use them.
use std::cmp::Ordering;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub [u32; 4]);

impl Version {
    /// Strict: exactly four dot-separated decimal numbers, no signs, no spaces, no leading '+'.
    pub fn parse_strict(s: &str) -> Option<Version> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 4 {
            return None;
        }
        let mut v = [0u32; 4];
        for (i, p) in parts.iter().enumerate() {
            if p.is_empty() || p.len() > 9 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            v[i] = p.parse().ok()?;
        }
        Some(Version(v))
    }

    /// Lenient, for what Windows reports for an installed product: 2 to 4 parts, missing parts are 0
    /// (MSI ProductVersion is major.minor.build; a 4th field may or may not be there).
    pub fn parse_installed(s: &str) -> Option<Version> {
        let parts: Vec<&str> = s.trim().split('.').collect();
        if parts.len() < 2 || parts.len() > 4 {
            return None;
        }
        let mut v = [0u32; 4];
        for (i, p) in parts.iter().enumerate() {
            if p.is_empty() || p.len() > 9 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            v[i] = p.parse().ok()?;
        }
        Some(Version(v))
    }

    /// MSI compares only the first three fields of ProductVersion; the 4th is ignored by Windows
    /// Installer, so two builds differing only there are "the same product version" to msiexec.
    pub fn cmp_msi(&self, other: &Version) -> Ordering {
        self.0[..3].cmp(&other.0[..3])
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0[0], self.0[1], self.0[2], self.0[3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict() {
        assert_eq!(Version::parse_strict("34.0.5.1"), Some(Version([34, 0, 5, 1])));
        for bad in ["34.0.5", "34.0.5.1.2", "34.0.5.x", "34..5.1", "+34.0.5.1", " 34.0.5.1", "34.0.5.-1", ""] {
            assert_eq!(Version::parse_strict(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn installed() {
        assert_eq!(Version::parse_installed("34.0.5"), Some(Version([34, 0, 5, 0])));
        assert_eq!(Version::parse_installed("34.0.5.2"), Some(Version([34, 0, 5, 2])));
        assert_eq!(Version::parse_installed("34"), None);
        assert_eq!(Version::parse_installed("a.b"), None);
    }

    #[test]
    fn ordering() {
        let a = Version([34, 0, 5, 1]);
        let b = Version([34, 0, 10, 0]);
        assert!(a < b, "numeric, not string, compare");
        assert!(Version([35, 0, 0, 0]) > b);
        assert_eq!(Version([34, 0, 5, 1]).cmp_msi(&Version([34, 0, 5, 9])), Ordering::Equal);
        assert_eq!(a.to_string(), "34.0.5.1");
    }
}
