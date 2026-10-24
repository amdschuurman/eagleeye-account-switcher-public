//! The `HKCU\Software\Blizzard Entertainment\Battle.net\UnifiedAuth` key. Each value name is an
//! 8-hex account code; each value is the account's "keep me logged in" token (a ~262-byte blob that
//! rotates on every login). We read the whole key, write one value, and diff two snapshots to find
//! which account's token changed. The trait lets tests run a fake map on Linux. Token bytes are never
//! logged.

use std::collections::BTreeMap;

/// The registry path we operate on (relative to HKCU).
#[cfg_attr(not(windows), allow(dead_code))]
pub const UNIFIED_AUTH_SUBKEY: &str = r"Software\Blizzard Entertainment\Battle.net\UnifiedAuth";

/// A snapshot of UnifiedAuth: value name -> raw bytes.
pub type AuthValues = BTreeMap<String, Vec<u8>>;

/// Read and write the UnifiedAuth values. Implementations never log the bytes.
pub trait Registry: Send + Sync {
    /// Every value under UnifiedAuth. An absent key is an empty map, not an error.
    fn read_unified_auth(&self) -> Result<AuthValues, String>;
    /// Write (create or overwrite) one REG_BINARY value under UnifiedAuth.
    fn write_unified_auth(&self, name: &str, value: &[u8]) -> Result<(), String>;
}

/// Which value name is new or changed between `before` and `after`. This is how Add/Save-current
/// find the token that belongs to the account that just logged in.
///
/// Returns the single changed/added name. If more than one changed (should not happen for one login)
/// the caller is told it was ambiguous; if none changed, `None`.
pub fn changed_value(before: &AuthValues, after: &AuthValues) -> ChangedValue {
    let mut changed: Vec<&String> = Vec::new();
    for (name, val) in after {
        match before.get(name) {
            Some(old) if old == val => {}
            _ => changed.push(name),
        }
    }
    match changed.as_slice() {
        [] => ChangedValue::None,
        [one] => ChangedValue::One((*one).clone()),
        many => ChangedValue::Many(many.iter().map(|s| (*s).clone()).collect()),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ChangedValue {
    None,
    One(String),
    Many(Vec<String>),
}

/// The Battle.net install folder from HKLM's uninstall key (32-bit view), if present. `None` off
/// Windows or when the key is absent, so the caller falls back to the default path.
pub fn hklm_battlenet_install() -> Option<String> {
    #[cfg(windows)]
    {
        win::hklm_install_location()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// The real registry.
#[cfg(windows)]
pub struct WinRegistry;

#[cfg(windows)]
impl Registry for WinRegistry {
    fn read_unified_auth(&self) -> Result<AuthValues, String> {
        win::read_values(UNIFIED_AUTH_SUBKEY)
    }
    fn write_unified_auth(&self, name: &str, value: &[u8]) -> Result<(), String> {
        win::write_value(UNIFIED_AUTH_SUBKEY, name, value)
    }
}

#[cfg(windows)]
mod win {
    use super::AuthValues;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegEnumValueW, RegOpenKeyExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_BINARY, REG_OPTION_NON_VOLATILE,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: a key this module opened and owns.
            unsafe { let _ = RegCloseKey(self.0); }
        }
    }

    pub fn read_values(subkey: &str) -> Result<AuthValues, String> {
        let sub = wide(subkey);
        let mut hkey = HKEY::default();
        // SAFETY: hkey is written on success; on failure we return before using it.
        let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr()), None, KEY_READ, &mut hkey) };
        if rc != ERROR_SUCCESS {
            // Key absent (fresh Battle.net, or never signed in): no values.
            return Ok(AuthValues::new());
        }
        let key = Key(hkey);
        let mut out = AuthValues::new();
        let mut index = 0u32;
        loop {
            let mut name_buf = vec![0u16; 256];
            let mut name_len = name_buf.len() as u32;
            // First call with no data buffer to learn the size.
            let mut data_len = 0u32;
            // SAFETY: buffers sized by the lengths passed; the API respects them.
            let rc = unsafe {
                RegEnumValueW(
                    key.0,
                    index,
                    Some(windows::core::PWSTR(name_buf.as_mut_ptr())),
                    &mut name_len,
                    None,
                    None,
                    None,
                    Some(&mut data_len),
                )
            };
            if rc != ERROR_SUCCESS {
                break; // ERROR_NO_MORE_ITEMS or any other stop.
            }
            let mut data = vec![0u8; data_len as usize];
            let mut data_len2 = data_len;
            let mut name_len2 = name_buf.len() as u32;
            // SAFETY: name_buf and data are sized to the lengths passed.
            let rc2 = unsafe {
                RegEnumValueW(
                    key.0,
                    index,
                    Some(windows::core::PWSTR(name_buf.as_mut_ptr())),
                    &mut name_len2,
                    None,
                    None,
                    Some(data.as_mut_ptr()),
                    Some(&mut data_len2),
                )
            };
            if rc2 != ERROR_SUCCESS {
                break;
            }
            data.truncate(data_len2 as usize);
            let name = String::from_utf16_lossy(&name_buf[..name_len2 as usize]);
            out.insert(name, data);
            index += 1;
        }
        Ok(out)
    }

    /// Read Battle.net's `InstallLocation` from the 32-bit uninstall key.
    pub fn hklm_install_location() -> Option<String> {
        use windows::Win32::System::Registry::{
            RegQueryValueExW, HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY,
        };
        let sub = wide(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Battle.net");
        let mut hkey = HKEY::default();
        // SAFETY: hkey is written on success; we return before using it on failure.
        let rc = unsafe {
            RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(sub.as_ptr()), None, KEY_READ | KEY_WOW64_32KEY, &mut hkey)
        };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let key = Key(hkey);
        let name = wide("InstallLocation");
        let mut size: u32 = 0;
        // SAFETY: first call with no buffer learns the byte length.
        let rc = unsafe {
            RegQueryValueExW(key.0, PCWSTR(name.as_ptr()), None, None, None, Some(&mut size))
        };
        if rc != ERROR_SUCCESS || size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        let mut size2 = size;
        // SAFETY: buf holds `size` bytes; the API writes at most that many.
        let rc = unsafe {
            RegQueryValueExW(key.0, PCWSTR(name.as_ptr()), None, None, Some(buf.as_mut_ptr()), Some(&mut size2))
        };
        if rc != ERROR_SUCCESS {
            return None;
        }
        // REG_SZ is UTF-16; drop the trailing NUL and any trailing separator.
        let u16s: Vec<u16> = buf[..size2 as usize]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let s = String::from_utf16_lossy(&u16s);
        let s = s.trim_end_matches('\0').trim_end_matches('\\').to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    pub fn write_value(subkey: &str, name: &str, value: &[u8]) -> Result<(), String> {
        let sub = wide(subkey);
        let mut hkey = HKEY::default();
        // SAFETY: hkey written on success.
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(sub.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut hkey,
                None,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err("could not open the account token store".into());
        }
        let key = Key(hkey);
        let name_w = wide(name);
        // SAFETY: value is a slice alive for the call; name is NUL-terminated.
        let rc = unsafe { RegSetValueExW(key.0, PCWSTR(name_w.as_ptr()), None, REG_BINARY, Some(value)) };
        if rc != ERROR_SUCCESS {
            return Err("could not write the account token".into());
        }
        Ok(())
    }
}

/// Dev builds off Windows: an empty, writable in-memory store so the app starts.
#[cfg(not(windows))]
pub struct DevRegistry {
    values: std::sync::Mutex<AuthValues>,
}

#[cfg(not(windows))]
impl Default for DevRegistry {
    fn default() -> Self {
        DevRegistry { values: std::sync::Mutex::new(AuthValues::new()) }
    }
}

#[cfg(not(windows))]
impl Registry for DevRegistry {
    fn read_unified_auth(&self) -> Result<AuthValues, String> {
        Ok(self.values.lock().unwrap().clone())
    }
    fn write_unified_auth(&self, name: &str, value: &[u8]) -> Result<(), String> {
        self.values.lock().unwrap().insert(name.to_string(), value.to_vec());
        Ok(())
    }
}

#[cfg(test)]
pub mod test_support {
    use super::{AuthValues, Registry};
    use std::sync::Mutex;

    /// An in-memory UnifiedAuth for tests.
    pub struct FakeRegistry {
        pub values: Mutex<AuthValues>,
    }

    impl FakeRegistry {
        pub fn new(values: AuthValues) -> Self {
            FakeRegistry { values: Mutex::new(values) }
        }
        pub fn empty() -> Self {
            FakeRegistry::new(AuthValues::new())
        }
    }

    impl Registry for FakeRegistry {
        fn read_unified_auth(&self) -> Result<AuthValues, String> {
            Ok(self.values.lock().unwrap().clone())
        }
        fn write_unified_auth(&self, name: &str, value: &[u8]) -> Result<(), String> {
            self.values.lock().unwrap().insert(name.to_string(), value.to_vec());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &[u8])]) -> AuthValues {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_vec())).collect()
    }

    #[test]
    fn diff_picks_the_single_changed_value() {
        let before = map(&[("aaaa0001", &[1, 2, 3]), ("bbbb0002", &[9, 9])]);
        // aaaa0001's token rotated.
        let after = map(&[("aaaa0001", &[4, 5, 6]), ("bbbb0002", &[9, 9])]);
        assert_eq!(changed_value(&before, &after), ChangedValue::One("aaaa0001".into()));
    }

    #[test]
    fn diff_picks_a_newly_added_value() {
        let before = map(&[("aaaa0001", &[1])]);
        let after = map(&[("aaaa0001", &[1]), ("cccc0003", &[7, 7, 7])]);
        assert_eq!(changed_value(&before, &after), ChangedValue::One("cccc0003".into()));
    }

    #[test]
    fn diff_reports_none_and_many() {
        let same = map(&[("aaaa0001", &[1])]);
        assert_eq!(changed_value(&same, &same), ChangedValue::None);
        let before = map(&[("a", &[1]), ("b", &[2])]);
        let after = map(&[("a", &[9]), ("b", &[8])]);
        match changed_value(&before, &after) {
            ChangedValue::Many(v) => assert_eq!(v.len(), 2),
            other => panic!("expected Many, got {other:?}"),
        }
    }
}
