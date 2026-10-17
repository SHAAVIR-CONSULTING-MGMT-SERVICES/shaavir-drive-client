//! minisign signature check (Ed25519 over BLAKE2b-512 of the file, "ED" algorithm), D-161.
//!
//! Public key (one base64 line, the second line of minisign.pub):  "Ed" | key id (8) | key (32)
//! Signature file (minisign -S -x latest.json.sig):
//!   untrusted comment: <ignored>
//!   base64("ED" | key id (8) | Ed25519(BLAKE2b-512(file)) (64))
//!   trusted comment: <text>
//!   base64(Ed25519(signature (64) | text))
//! Only the pre-hashed "ED" form is accepted (minisign's default); the legacy "Ed" form signs the raw
//! file and is refused, so a signature can never be read in two ways. Both signatures must verify.
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use blake2::{Blake2b512, Digest};
use ed25519_dalek::{Signature, VerifyingKey};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigError {
    BadKey,
    BadSignatureFile,
    LegacyAlgorithm,
    WrongKey,
    Invalid,
    InvalidTrustedComment,
}

pub struct PublicKey {
    key_id: [u8; 8],
    key: VerifyingKey,
}

impl PublicKey {
    /// Accepts the bare base64 line or the whole minisign.pub file.
    pub fn parse(text: &str) -> Result<PublicKey, SigError> {
        let line = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))
            .ok_or(SigError::BadKey)?;
        let raw = STANDARD.decode(line).map_err(|_| SigError::BadKey)?;
        if raw.len() != 42 || &raw[..2] != b"Ed" {
            return Err(SigError::BadKey);
        }
        let mut key_id = [0u8; 8];
        key_id.copy_from_slice(&raw[2..10]);
        let mut k = [0u8; 32];
        k.copy_from_slice(&raw[10..42]);
        let key = VerifyingKey::from_bytes(&k).map_err(|_| SigError::BadKey)?;
        Ok(PublicKey { key_id, key })
    }

    pub fn key_id_hex(&self) -> String {
        // minisign prints the key id as the little-endian u64 in upper-case hex
        let mut id = self.key_id;
        id.reverse();
        id.iter().map(|b| format!("{b:02X}")).collect()
    }
}

/// Verifies `sig_text` (the .sig/.minisig file) over `message`. Returns the trusted comment on success.
pub fn verify(key: &PublicKey, message: &[u8], sig_text: &str) -> Result<String, SigError> {
    let lines: Vec<&str> = sig_text.lines().map(|l| l.trim_end_matches('\r')).collect();
    if lines.len() < 4 || !lines[0].starts_with("untrusted comment:") {
        return Err(SigError::BadSignatureFile);
    }
    let sig_raw = STANDARD.decode(lines[1].trim()).map_err(|_| SigError::BadSignatureFile)?;
    if sig_raw.len() != 74 {
        return Err(SigError::BadSignatureFile);
    }
    match &sig_raw[..2] {
        b"ED" => {}
        b"Ed" => return Err(SigError::LegacyAlgorithm),
        _ => return Err(SigError::BadSignatureFile),
    }
    if sig_raw[2..10] != key.key_id {
        return Err(SigError::WrongKey);
    }
    let sig_bytes: [u8; 64] = sig_raw[10..74].try_into().unwrap();
    let digest = Blake2b512::digest(message);
    key.key
        .verify_strict(&digest, &Signature::from_bytes(&sig_bytes))
        .map_err(|_| SigError::Invalid)?;

    let trusted = lines[2].strip_prefix("trusted comment: ").ok_or(SigError::BadSignatureFile)?;
    let global = STANDARD.decode(lines[3].trim()).map_err(|_| SigError::BadSignatureFile)?;
    let global: [u8; 64] = global.as_slice().try_into().map_err(|_| SigError::BadSignatureFile)?;
    let mut signed = Vec::with_capacity(64 + trusted.len());
    signed.extend_from_slice(&sig_bytes);
    signed.extend_from_slice(trusted.as_bytes());
    key.key
        .verify_strict(&signed, &Signature::from_bytes(&global))
        .map_err(|_| SigError::InvalidTrustedComment)?;
    Ok(trusted.to_string())
}

#[cfg(test)]
pub(crate) mod testkit {
    //! Produces minisign-format keys and signatures for tests (the real ones come from minisign).
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    pub struct TestSigner {
        pub sk: SigningKey,
        pub key_id: [u8; 8],
    }

    impl TestSigner {
        pub fn new(seed: u8) -> TestSigner {
            TestSigner { sk: SigningKey::from_bytes(&[seed; 32]), key_id: [seed, 1, 2, 3, 4, 5, 6, 7] }
        }
        pub fn public_line(&self) -> String {
            let mut raw = b"Ed".to_vec();
            raw.extend_from_slice(&self.key_id);
            raw.extend_from_slice(self.sk.verifying_key().as_bytes());
            STANDARD.encode(raw)
        }
        pub fn sign(&self, msg: &[u8], trusted: &str) -> String {
            let sig = self.sk.sign(&Blake2b512::digest(msg));
            let mut raw = b"ED".to_vec();
            raw.extend_from_slice(&self.key_id);
            raw.extend_from_slice(&sig.to_bytes());
            let mut g = sig.to_bytes().to_vec();
            g.extend_from_slice(trusted.as_bytes());
            let gsig = self.sk.sign(&g);
            format!(
                "untrusted comment: signature from test key\n{}\ntrusted comment: {}\n{}\n",
                STANDARD.encode(raw),
                trusted,
                STANDARD.encode(gsig.to_bytes())
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::TestSigner;
    use super::*;

    /// Made by an independent implementation (Python `cryptography`/OpenSSL Ed25519 + hashlib BLAKE2b,
    /// minisign file layout); client-build/tests checks the same vector with verify-manifest.py.
    const XKEY: &str = "RWQBI0VniavN7wOhB7/zzhC+HXDdGOdLwJln5NYwm6UNXx3chmQSVTG4";
    const XSIG: &str = "untrusted comment: signature from minisign secret key\nRUQBI0VniavN71/o3xU2uHNFSW+WIyHY5upjqqScCF53E8oJbJHFhv5G+nUflrY8Ij1BL2QE1k+9JJPbYSYXB79qxBXjjR7d/AU=\ntrusted comment: timestamp:1760000000\tfile:latest.json\thashed\n9z+RoI84HXAy2ifYHqaLvELwB4VZ6NCjGK5tVH6lWGrNU79B+LK1smf1ssYfCbnMpsNbKiU8klYcdTvCAC9wAQ==\n";

    #[test]
    fn independent_implementation_vector() {
        let key = PublicKey::parse(XKEY).unwrap();
        assert_eq!(key.key_id_hex(), "EFCDAB8967452301");
        assert_eq!(verify(&key, b"{\"schema\":1}\n", XSIG).unwrap(), "timestamp:1760000000\tfile:latest.json\thashed");
        assert_eq!(verify(&key, b"{\"schema\":2}\n", XSIG), Err(SigError::Invalid));
    }

    #[test]
    fn good_signature() {
        let s = TestSigner::new(7);
        let key = PublicKey::parse(&s.public_line()).unwrap();
        let sig = s.sign(b"payload", "timestamp:1 file:latest.json");
        assert_eq!(verify(&key, b"payload", &sig).unwrap(), "timestamp:1 file:latest.json");
    }

    #[test]
    fn whole_pub_file_accepted() {
        let s = TestSigner::new(7);
        let file = format!("untrusted comment: minisign public key ABC\n{}\n", s.public_line());
        assert!(PublicKey::parse(&file).is_ok());
    }

    #[test]
    fn tampered_message() {
        let s = TestSigner::new(7);
        let key = PublicKey::parse(&s.public_line()).unwrap();
        let sig = s.sign(b"payload", "t");
        assert_eq!(verify(&key, b"payloaD", &sig), Err(SigError::Invalid));
    }

    #[test]
    fn wrong_key() {
        let a = TestSigner::new(7);
        let b = TestSigner::new(9);
        let key_b = PublicKey::parse(&b.public_line()).unwrap();
        assert_eq!(verify(&key_b, b"x", &a.sign(b"x", "t")), Err(SigError::WrongKey));
        // same key id, different key: the signature itself must fail
        let mut c = TestSigner::new(9);
        c.key_id = a.key_id;
        let key_c = PublicKey::parse(&c.public_line()).unwrap();
        assert_eq!(verify(&key_c, b"x", &a.sign(b"x", "t")), Err(SigError::Invalid));
    }

    #[test]
    fn tampered_trusted_comment() {
        let s = TestSigner::new(7);
        let key = PublicKey::parse(&s.public_line()).unwrap();
        let sig = s.sign(b"x", "timestamp:1").replace("timestamp:1", "timestamp:2");
        assert_eq!(verify(&key, b"x", &sig), Err(SigError::InvalidTrustedComment));
    }

    #[test]
    fn legacy_and_garbage_refused() {
        let s = TestSigner::new(7);
        let key = PublicKey::parse(&s.public_line()).unwrap();
        let sig = s.sign(b"x", "t");
        let mut lines: Vec<String> = sig.lines().map(String::from).collect();
        let mut raw = STANDARD.decode(&lines[1]).unwrap();
        raw[1] = b'd';
        lines[1] = STANDARD.encode(&raw);
        assert_eq!(verify(&key, b"x", &lines.join("\n")), Err(SigError::LegacyAlgorithm));
        assert_eq!(verify(&key, b"x", "nonsense"), Err(SigError::BadSignatureFile));
        assert_eq!(verify(&key, b"x", ""), Err(SigError::BadSignatureFile));
        assert!(PublicKey::parse("not base64!").is_err());
        assert!(PublicKey::parse("").is_err());
    }

    #[test]
    fn crlf_signature_file() {
        let s = TestSigner::new(7);
        let key = PublicKey::parse(&s.public_line()).unwrap();
        let sig = s.sign(b"x", "t").replace('\n', "\r\n");
        assert!(verify(&key, b"x", &sig).is_ok());
    }
}
