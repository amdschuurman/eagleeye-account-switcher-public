//! Battle.net.config editing. The file is JSON with a nested `Client` object that holds
//! `SavedAccountNames` (which account Battle.net signs in with) and `AutoLogin`. We only ever touch
//! those two keys. Every other key, the key order, and the existing value type of `AutoLogin`
//! (it is the JSON bool `true` in newer builds, the string `"true"` in older ones) are preserved.
//! Output is 4-space indented like the original; writes are atomic.

use serde_json::Value;
use std::path::Path;

/// Parse the config text (order preserved via serde_json's `preserve_order` feature).
pub fn parse(text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|e| format!("Battle.net.config is not valid JSON: {e}"))
}

/// Serialize with 4-space indent, matching the original file's formatting.
pub fn serialize(config: &Value) -> Result<String, String> {
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
    use serde::Serialize;
    config.serialize(&mut ser).map_err(|e| format!("serialize failed: {e}"))?;
    String::from_utf8(buf).map_err(|e| format!("utf8 failed: {e}"))
}

/// Ensure `Client` is an object and return a mutable reference to it.
fn client_mut(config: &mut Value) -> Result<&mut serde_json::Map<String, Value>, String> {
    if !config.is_object() {
        return Err("Battle.net.config root is not an object".into());
    }
    let root = config.as_object_mut().unwrap();
    let entry = root.entry("Client").or_insert_with(|| Value::Object(Default::default()));
    entry
        .as_object_mut()
        .ok_or_else(|| "Battle.net.config Client is not an object".into())
}

/// Set `Client.SavedAccountNames` to `email` (always a JSON string). An empty string clears it,
/// which is how the add/save cycle forces Battle.net to show its login window.
pub fn set_saved_account_names(config: &mut Value, email: &str) -> Result<(), String> {
    let client = client_mut(config)?;
    // Replace the value in place so the key keeps its position; insert at the end only if new.
    client.insert("SavedAccountNames".into(), Value::String(email.to_string()));
    Ok(())
}

/// Set `Client.AutoLogin` to a true value, preserving the type already in the file. If the key is
/// absent it is added as the JSON bool `true`.
pub fn set_auto_login_true(config: &mut Value) -> Result<(), String> {
    let client = client_mut(config)?;
    let new = match client.get("AutoLogin") {
        Some(Value::String(_)) => Value::String("true".into()),
        Some(Value::Bool(_)) | None => Value::Bool(true),
        Some(other) => {
            // An unexpected type: keep its shape as far as possible rather than corrupt it.
            match other {
                Value::Number(_) => Value::Number(1.into()),
                _ => Value::Bool(true),
            }
        }
    };
    client.insert("AutoLogin".into(), new);
    Ok(())
}

/// Read the config from disk, or return an empty object if it does not exist.
pub fn read_or_default(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Default::default())),
        Err(e) => Err(format!("read Battle.net.config failed: {e}")),
    }
}

/// Write the config back atomically.
pub fn write(path: &Path, config: &Value) -> Result<(), String> {
    let text = serialize(config)?;
    crate::atomic::write(path, text.as_bytes())
}

/// Waits between attempts when Battle.net.config is locked (another program holds it open, an
/// antivirus scan, Battle.net itself mid-write). Six attempts in total, about 1.5 s worst case.
pub const LOCK_BACKOFF_MS: [u64; 5] = [50, 100, 200, 400, 800];

/// The error the user sees when every attempt failed.
pub const LOCKED_MESSAGE: &str =
    "Battle.net.config is locked by another program. Close Battle.net or that program and try again.";

/// Run `op` (a read-modify-write of the config) and retry it on failure with `LOCK_BACKOFF_MS`,
/// sleeping through `sleep`. After the last failed attempt, returns `LOCKED_MESSAGE`. A content
/// error (invalid JSON, wrong shape; those messages start with "Battle.net.config ") is not a lock
/// and is returned at once, unretried.
pub fn with_lock_retry(mut op: impl FnMut() -> Result<(), String>, sleep: &dyn Fn(u64)) -> Result<(), String> {
    let attempt = |op: &mut dyn FnMut() -> Result<(), String>| -> Option<Result<(), String>> {
        match op() {
            Ok(()) => Some(Ok(())),
            Err(e) if e.starts_with("Battle.net.config ") => Some(Err(e)),
            Err(_) => None,
        }
    };
    if let Some(done) = attempt(&mut op) {
        return done;
    }
    for wait in LOCK_BACKOFF_MS {
        sleep(wait);
        if let Some(done) = attempt(&mut op) {
            return done;
        }
    }
    Err(LOCKED_MESSAGE.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn lock_retry_backs_off_then_succeeds() {
        let slept = RefCell::new(Vec::new());
        let mut calls = 0;
        let r = with_lock_retry(
            || {
                calls += 1;
                if calls <= 3 { Err("rename failed".into()) } else { Ok(()) }
            },
            &|ms| slept.borrow_mut().push(ms),
        );
        assert!(r.is_ok());
        assert_eq!(calls, 4);
        assert_eq!(*slept.borrow(), vec![50, 100, 200]);
    }

    #[test]
    fn lock_retry_gives_a_clear_error_after_the_last_wait() {
        let slept = RefCell::new(Vec::new());
        let mut calls = 0;
        let r = with_lock_retry(
            || {
                calls += 1;
                Err("rename failed".into())
            },
            &|ms| slept.borrow_mut().push(ms),
        );
        assert_eq!(r.unwrap_err(), LOCKED_MESSAGE);
        assert_eq!(calls, 6);
        assert_eq!(*slept.borrow(), vec![50, 100, 200, 400, 800]);
    }

    #[test]
    fn lock_retry_does_not_retry_a_corrupt_file() {
        let slept = RefCell::new(Vec::new());
        let r = with_lock_retry(|| parse("{nope").map(|_| ()), &|ms| slept.borrow_mut().push(ms));
        assert!(r.unwrap_err().contains("not valid JSON"));
        assert!(slept.borrow().is_empty());
    }

    #[test]
    fn lock_retry_first_try_ok_never_sleeps() {
        let slept = RefCell::new(Vec::new());
        assert!(with_lock_retry(|| Ok(()), &|ms| slept.borrow_mut().push(ms)).is_ok());
        assert!(slept.borrow().is_empty());
    }

    // A config whose AutoLogin is the JSON bool, with several other keys around the ones we touch.
    const BOOL_CONFIG: &str = r#"{
    "Client": {
        "Install": {
            "DefaultInstallPath": "C:\\Games"
        },
        "SavedAccountNames": "old@example.com",
        "AutoLogin": false,
        "LastLoginRegion": "EU"
    },
    "Games": {
        "prometheus": {
            "LastPlayed": "123"
        }
    }
}"#;

    // Older style: AutoLogin is the string "false".
    const STRING_CONFIG: &str = r#"{
    "Client": {
        "SavedAccountNames": "old@example.com",
        "AutoLogin": "false"
    }
}"#;

    #[test]
    fn preserves_bool_type_and_other_keys_and_order() {
        let mut c = parse(BOOL_CONFIG).unwrap();
        set_saved_account_names(&mut c, "new@example.com").unwrap();
        set_auto_login_true(&mut c).unwrap();
        let out = serialize(&c).unwrap();

        // The value we set, with the SAME JSON type (bool, not "true").
        assert!(out.contains("\"SavedAccountNames\": \"new@example.com\""));
        assert!(out.contains("\"AutoLogin\": true"), "bool stays bool:\n{out}");
        assert!(!out.contains("\"AutoLogin\": \"true\""));

        // Every untouched key survives.
        assert!(out.contains("DefaultInstallPath"));
        assert!(out.contains("LastLoginRegion"));
        assert!(out.contains("prometheus"));
        assert!(out.contains("\"LastPlayed\": \"123\""));

        // Key order inside Client is unchanged: Install, SavedAccountNames, AutoLogin, LastLoginRegion.
        let install = out.find("\"Install\"").unwrap();
        let saved = out.find("\"SavedAccountNames\"").unwrap();
        let auto = out.find("\"AutoLogin\"").unwrap();
        let region = out.find("\"LastLoginRegion\"").unwrap();
        assert!(install < saved && saved < auto && auto < region, "order changed:\n{out}");

        // 4-space indent.
        assert!(out.contains("\n    \"Client\""), "4-space indent:\n{out}");
        assert!(out.contains("\n        \"SavedAccountNames\""));
    }

    #[test]
    fn preserves_string_type_of_auto_login() {
        let mut c = parse(STRING_CONFIG).unwrap();
        set_auto_login_true(&mut c).unwrap();
        let out = serialize(&c).unwrap();
        assert!(out.contains("\"AutoLogin\": \"true\""), "string stays string:\n{out}");
        assert!(!out.contains("\"AutoLogin\": true"));
    }

    #[test]
    fn empty_email_clears_saved_account() {
        let mut c = parse(BOOL_CONFIG).unwrap();
        set_saved_account_names(&mut c, "").unwrap();
        let out = serialize(&c).unwrap();
        assert!(out.contains("\"SavedAccountNames\": \"\""));
    }

    #[test]
    fn missing_client_is_created_with_bool_auto_login() {
        let mut c = parse("{}").unwrap();
        set_saved_account_names(&mut c, "x@y.com").unwrap();
        set_auto_login_true(&mut c).unwrap();
        let out = serialize(&c).unwrap();
        assert!(out.contains("\"Client\""));
        assert!(out.contains("\"AutoLogin\": true"));
    }

    #[test]
    fn roundtrip_is_stable_when_nothing_changes_type() {
        // Setting the same email twice must not change anything the second time.
        let mut c = parse(BOOL_CONFIG).unwrap();
        set_saved_account_names(&mut c, "new@example.com").unwrap();
        let first = serialize(&c).unwrap();
        let mut c2 = parse(&first).unwrap();
        set_saved_account_names(&mut c2, "new@example.com").unwrap();
        assert_eq!(first, serialize(&c2).unwrap());
    }
}
