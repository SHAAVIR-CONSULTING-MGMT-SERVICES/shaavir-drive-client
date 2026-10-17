//! latest.json: the signed release manifest the stub reads (D-161). Written by
//! client-build/update-feed/gen-update-feed.py; schema 1:
//! {
//!   "schema": 1,
//!   "product": "Example Drive",
//!   "version": "34.0.5.1",
//!   "channel": "stable",
//!   "released": "2026-10-18",
//!   "min_windows_build": 19045,
//!   "release_notes_url": "https://www.example.com/drive/download",
//!   "files": {
//!     "x64":   { "url": "https://updates.example.com/client/ExampleDrive-34.0.5.1-x64.msi",
//!                "size": 123456789, "sha256": "<64 lower-case hex>" },
//!     "arm64": null
//!   }
//! }
//! Everything is checked here, before anything is downloaded: the product name, the version format,
//! the host and path of every URL (https, the pinned host, /client/<name>.msi, no query, no userinfo,
//! no port), the size range and the hash format. Unknown keys are refused so a typo cannot slip through.
use crate::version::Version;
use serde::Deserialize;

pub const MAX_MSI_BYTES: u64 = 2_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X64,
    Arm64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installer {
    pub url: String,
    /// Path on the update host, e.g. "/client/ShaavirDrive-34.0.5.1-x64.msi"
    pub path: String,
    pub file_name: String,
    pub size: u64,
    pub sha256: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub product: String,
    pub version: Version,
    pub channel: String,
    pub min_windows_build: u32,
    pub release_notes_url: String,
    pub x64: Installer,
    pub arm64: Option<Installer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    url: String,
    size: u64,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFiles {
    x64: RawFile,
    #[serde(default)]
    arm64: Option<RawFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    schema: u32,
    product: String,
    version: String,
    channel: String,
    released: String,
    min_windows_build: u32,
    release_notes_url: String,
    files: RawFiles,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    Json(String),
    Schema(u32),
    Product(String),
    Version(String),
    Channel(String),
    Url(String),
    Size(u64),
    Hash(String),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Json(e) => write!(f, "manifest is not valid: {e}"),
            ManifestError::Schema(s) => write!(f, "manifest schema {s} is not supported"),
            ManifestError::Product(p) => write!(f, "manifest is for another product ({p})"),
            ManifestError::Version(v) => write!(f, "manifest version is not a.b.c.d ({v})"),
            ManifestError::Channel(c) => write!(f, "manifest channel {c} is not supported"),
            ManifestError::Url(u) => write!(f, "installer address is not allowed ({u})"),
            ManifestError::Size(s) => write!(f, "installer size {s} is out of range"),
            ManifestError::Hash(h) => write!(f, "installer checksum is malformed ({h})"),
        }
    }
}

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// The only URL shape accepted: https://<host>/client/<File-Name>.msi
pub fn check_url(url: &str, host: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://")?;
    let (h, path) = rest.split_once('/')?;
    if h != host {
        return None; // also refuses userinfo (a@b), ports (b:443) and upper-case variants
    }
    let name = path.strip_prefix("client/")?;
    let ok_name = !name.is_empty()
        && name.len() <= 100
        && name.ends_with(".msi")
        && name.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
        && !name.contains("..");
    if !ok_name {
        return None;
    }
    Some((format!("/{path}"), name.to_string()))
}

fn installer(raw: RawFile, host: &str) -> Result<Installer, ManifestError> {
    let (path, file_name) = check_url(&raw.url, host).ok_or_else(|| ManifestError::Url(raw.url.clone()))?;
    if raw.size == 0 || raw.size > MAX_MSI_BYTES {
        return Err(ManifestError::Size(raw.size));
    }
    let sha256 = parse_hex32(&raw.sha256).ok_or_else(|| ManifestError::Hash(raw.sha256.clone()))?;
    Ok(Installer { url: raw.url, path, file_name, size: raw.size, sha256 })
}

/// Parses and checks the manifest. `bytes` must be the exact bytes whose signature was verified.
pub fn parse(bytes: &[u8], expected_product: &str, host: &str) -> Result<Manifest, ManifestError> {
    let raw: Raw = serde_json::from_slice(bytes).map_err(|e| ManifestError::Json(e.to_string()))?;
    if raw.schema != 1 {
        return Err(ManifestError::Schema(raw.schema));
    }
    if raw.product != expected_product {
        return Err(ManifestError::Product(raw.product));
    }
    let version = Version::parse_strict(&raw.version).ok_or(ManifestError::Version(raw.version.clone()))?;
    if raw.channel != "stable" {
        return Err(ManifestError::Channel(raw.channel));
    }
    let _ = raw.released; // informational; the signature's trusted comment carries the signing time
    let x64 = installer(raw.files.x64, host)?;
    let arm64 = raw.files.arm64.map(|f| installer(f, host)).transpose()?;
    Ok(Manifest {
        product: raw.product,
        version,
        channel: raw.channel,
        min_windows_build: raw.min_windows_build,
        release_notes_url: raw.release_notes_url,
        x64,
        arm64,
    })
}

impl Manifest {
    pub fn installer_for(&self, arch: Arch) -> Option<&Installer> {
        match arch {
            Arch::X64 => Some(&self.x64),
            Arch::Arm64 => self.arm64.as_ref(),
        }
    }
}

#[cfg(test)]
pub(crate) fn sample(version: &str, host: &str) -> String {
    format!(
        r#"{{
  "schema": 1,
  "product": "Example Drive",
  "version": "{version}",
  "channel": "stable",
  "released": "2026-10-18",
  "min_windows_build": 19045,
  "release_notes_url": "https://www.example.com/drive/download",
  "files": {{
    "x64": {{"url": "https://{host}/client/ExampleDrive-{version}-x64.msi", "size": 1000, "sha256": "{h}"}},
    "arm64": null
  }}
}}"#,
        h = "ab".repeat(32)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    const H: &str = "updates.example.com";

    #[test]
    fn good() {
        let m = parse(sample("34.0.5.1", H).as_bytes(), "Example Drive", H).unwrap();
        assert_eq!(m.version, Version([34, 0, 5, 1]));
        assert_eq!(m.x64.path, "/client/ExampleDrive-34.0.5.1-x64.msi");
        assert_eq!(m.x64.file_name, "ExampleDrive-34.0.5.1-x64.msi");
        assert_eq!(m.x64.sha256, [0xab; 32]);
        assert!(m.installer_for(Arch::Arm64).is_none());
        assert!(m.installer_for(Arch::X64).is_some());
    }

    #[test]
    fn other_product_or_schema() {
        let s = sample("34.0.5.1", H);
        assert!(matches!(parse(s.as_bytes(), "Other Drive", H), Err(ManifestError::Product(_))));
        let s2 = s.replace("\"schema\": 1", "\"schema\": 2");
        assert!(matches!(parse(s2.as_bytes(), "Example Drive", H), Err(ManifestError::Schema(2))));
        let s3 = s.replace("\"stable\"", "\"beta\"");
        assert!(matches!(parse(s3.as_bytes(), "Example Drive", H), Err(ManifestError::Channel(_))));
    }

    #[test]
    fn unknown_key_refused() {
        let s = sample("34.0.5.1", H).replace("\"schema\": 1,", "\"schema\": 1, \"extra\": true,");
        assert!(matches!(parse(s.as_bytes(), "Example Drive", H), Err(ManifestError::Json(_))));
    }

    #[test]
    fn urls() {
        let ok = |u: &str| check_url(u, H).is_some();
        assert!(ok("https://updates.example.com/client/ExampleDrive-34.0.5.1-x64.msi"));
        for bad in [
            "http://updates.example.com/client/a.msi",
            "https://evil.com/client/a.msi",
            "https://updates.example.com.evil.com/client/a.msi",
            "https://user@updates.example.com/client/a.msi",
            "https://updates.example.com:8443/client/a.msi",
            "https://UPDATES.example.com/client/a.msi",
            "https://updates.example.com/client/a.exe",
            "https://updates.example.com/client/a.msi?x=1",
            "https://updates.example.com/client/../a.msi",
            "https://updates.example.com/client/sub/a.msi",
            "https://updates.example.com/other/a.msi",
            "https://updates.example.com/client/.msi",
            "https://updates.example.com/client/a%2f.msi",
        ] {
            assert!(!ok(bad), "{bad}");
        }
    }

    #[test]
    fn size_and_hash() {
        let s = sample("34.0.5.1", H);
        let z = s.replace("\"size\": 1000", "\"size\": 0");
        assert!(matches!(parse(z.as_bytes(), "Example Drive", H), Err(ManifestError::Size(0))));
        let big = s.replace("\"size\": 1000", "\"size\": 3000000000");
        assert!(matches!(parse(big.as_bytes(), "Example Drive", H), Err(ManifestError::Size(_))));
        let upper = s.replace(&"ab".repeat(32), &"AB".repeat(32));
        assert!(matches!(parse(upper.as_bytes(), "Example Drive", H), Err(ManifestError::Hash(_))));
        let short = s.replace(&"ab".repeat(32), "abcd");
        assert!(matches!(parse(short.as_bytes(), "Example Drive", H), Err(ManifestError::Hash(_))));
    }

    #[test]
    fn bad_version() {
        let s = sample("34.0.5", H);
        assert!(matches!(parse(s.as_bytes(), "Example Drive", H), Err(ManifestError::Version(_)) | Err(ManifestError::Url(_))));
    }

    #[test]
    fn arm64_entry_checked_too() {
        let s = sample("34.0.5.1", H).replace(
            "\"arm64\": null",
            "\"arm64\": {\"url\": \"https://evil.com/client/a.msi\", \"size\": 1, \"sha256\": \"00\"}",
        );
        assert!(parse(s.as_bytes(), "Example Drive", H).is_err());
    }
}
