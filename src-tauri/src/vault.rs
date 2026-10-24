//! The token vault: one encrypted `<code>.bin` per account. The plaintext is a small JSON record
//! holding the account email, the UnifiedAuth value name, the token value (base64) and when it was
//! captured. Encryption is DPAPI (`CryptProtectData`, CurrentUser scope, app-specific entropy) on
//! Windows; tests inject a fake protector. Nothing here ever logs or prints the contents.

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// App-specific entropy mixed into DPAPI so a blob is only decryptable by this app for this user. It is a fixed
/// label, not a secret; changing it makes every saved account unreadable, so it stays as the first builds set it.
#[cfg(windows)]
const ENTROPY: &[u8] = b"ro.eagleeye.accountswitcher/vault/v1";

/// The plaintext record stored (encrypted) in a vault file. `Debug` is redacted on purpose: a stray
/// `{:?}` must never print the email or the token.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct VaultEntry {
    pub email: String,
    /// The UnifiedAuth registry value name (8 hex chars; differs from the account code).
    pub ua_name: String,
    /// The UnifiedAuth value bytes, base64 encoded.
    pub ua_value: String,
    /// Milliseconds since the epoch when this token was captured.
    pub captured_at: u64,
}

impl std::fmt::Debug for VaultEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultEntry")
            .field("email", &"<redacted>")
            .field("ua_name", &self.ua_name)
            .field("ua_value", &format_args!("<{} chars redacted>", self.ua_value.len()))
            .field("captured_at", &self.captured_at)
            .finish()
    }
}

impl VaultEntry {
    pub fn new(email: String, ua_name: String, ua_value_bytes: &[u8], captured_at: u64) -> Self {
        VaultEntry {
            email,
            ua_name,
            ua_value: base64::engine::general_purpose::STANDARD.encode(ua_value_bytes),
            captured_at,
        }
    }

    /// The raw token bytes (decoded from base64).
    pub fn value_bytes(&self) -> Result<Vec<u8>, String> {
        base64::engine::general_purpose::STANDARD
            .decode(self.ua_value.as_bytes())
            .map_err(|_| "vault token is not valid base64".to_string())
    }
}

/// Reversible protection of a byte blob. Errors never contain the plaintext.
pub trait Protector: Send + Sync {
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, String>;
    fn unprotect(&self, blob: &[u8]) -> Result<Vec<u8>, String>;
}

/// Write one account's vault file atomically.
pub fn write_entry(path: &Path, protector: &dyn Protector, entry: &VaultEntry) -> Result<(), String> {
    let plain = serde_json::to_vec(entry).map_err(|_| "could not encode vault record".to_string())?;
    let blob = protector.protect(&plain)?;
    crate::atomic::write(path, &blob)
}

/// Read one account's vault file. A decrypt failure (for example a blob from another Windows user)
/// is reported as an error without any content.
pub fn read_entry(path: &Path, protector: &dyn Protector) -> Result<VaultEntry, String> {
    let blob = std::fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "no saved token for this account".to_string()
        } else {
            "could not read the saved token".to_string()
        }
    })?;
    let plain = protector.unprotect(&blob)?;
    serde_json::from_slice(&plain).map_err(|_| "saved token is unreadable".to_string())
}

/// Remove a vault file (used when an account is removed). Missing is fine.
pub fn remove_entry(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("could not remove the saved token".to_string()),
    }
}

/// The real DPAPI protector.
#[cfg(windows)]
pub struct DpapiProtector;

#[cfg(windows)]
impl Protector for DpapiProtector {
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
        dpapi::crypt(plain, true)
    }
    fn unprotect(&self, blob: &[u8]) -> Result<Vec<u8>, String> {
        dpapi::crypt(blob, false)
    }
}

#[cfg(windows)]
mod dpapi {
    use super::ENTROPY;
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }

    /// Encrypt (`encrypt = true`) or decrypt the data with CurrentUser DPAPI and our entropy.
    pub fn crypt(data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
        let input = blob(data);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: input and entropy point at slices alive for the whole call; `out` is written by the
        // API and its buffer is freed with LocalFree below.
        let res = unsafe {
            if encrypt {
                CryptProtectData(&input, None, Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            } else {
                CryptUnprotectData(&input, None, Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            }
        };
        if res.is_err() {
            // No content in the message, only the operation.
            return Err(if encrypt { "could not protect the token".into() } else { "could not read the saved token".into() });
        }
        // SAFETY: on success `out.pbData` points at `out.cbData` bytes owned by the system.
        let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
        // SAFETY: free the system-allocated output buffer.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(out.pbData as *mut core::ffi::c_void)));
        }
        Ok(bytes)
    }
}

/// Dev builds off Windows have no DPAPI. This passthrough stores the record as-is so a developer can
/// run the UI on Linux; it is NOT used in any shipped (Windows) build and provides no protection.
#[cfg(not(windows))]
pub struct DevProtector;

#[cfg(not(windows))]
impl Protector for DevProtector {
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
        Ok(plain.to_vec())
    }
    fn unprotect(&self, blob: &[u8]) -> Result<Vec<u8>, String> {
        Ok(blob.to_vec())
    }
}

#[cfg(test)]
pub mod test_support {
    use super::Protector;

    /// A reversible fake "protector" (XOR + a marker prefix) so the vault roundtrip is testable on
    /// Linux. Not cryptographic; used only in tests.
    pub struct XorProtector;

    const KEY: u8 = 0x5a;
    const MARK: &[u8] = b"FAKE";

    impl Protector for XorProtector {
        fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
            let mut v = MARK.to_vec();
            v.extend(plain.iter().map(|b| b ^ KEY));
            Ok(v)
        }
        fn unprotect(&self, blob: &[u8]) -> Result<Vec<u8>, String> {
            let body = blob
                .strip_prefix(MARK)
                .ok_or_else(|| "could not read the saved token".to_string())?;
            Ok(body.iter().map(|b| b ^ KEY).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::XorProtector;
    use super::*;

    #[test]
    fn roundtrips_through_a_fake_protector() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("12ab34cd.bin");
        let p = XorProtector;
        let token = [0u8, 1, 2, 250, 251, 255, 128];
        let entry = VaultEntry::new("ca***@g***.com".into(), "12ab34cd".into(), &token, 1_700_000_000_000);
        write_entry(&path, &p, &entry).unwrap();

        // The file on disk is NOT the plaintext (encrypted at rest).
        let raw = std::fs::read(&path).unwrap();
        assert!(!raw.windows(token.len()).any(|w| w == token), "token bytes stored in the clear");

        let back = read_entry(&path, &p).unwrap();
        assert_eq!(back, entry);
        assert_eq!(back.value_bytes().unwrap(), token);
    }

    #[test]
    fn debug_output_never_shows_email_or_token() {
        let e = VaultEntry::new("secret.person@example.com".into(), "12ab34cd".into(), &[0xde, 0xad, 0xbe, 0xef], 1);
        let shown = format!("{e:?}");
        assert!(!shown.contains("secret.person"), "{shown}");
        assert!(!shown.contains(&e.ua_value), "{shown}");
        assert!(shown.contains("redacted"));
    }

    #[test]
    fn a_blob_we_cannot_decrypt_errors_without_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.bin");
        std::fs::write(&path, b"garbage-from-another-user").unwrap();
        let err = read_entry(&path, &XorProtector).unwrap_err();
        assert!(!err.contains("garbage"));
    }

    #[test]
    fn missing_file_is_a_clean_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = read_entry(&dir.path().join("none.bin"), &XorProtector).unwrap_err();
        assert!(err.contains("no saved token"));
    }
}
