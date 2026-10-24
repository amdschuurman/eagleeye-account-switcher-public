//! Reading TcNo Account Switcher's Battle.net login cache. Each account is a folder under
//! `%APPDATA%\TcNo Account Switcher\LoginCache\BattleNet\<label>\` with an `email.json`. TcNo has
//! written that file in a few shapes over its versions (a bare JSON string, an object, or an array),
//! so the parser accepts all three. We never read TcNo's token files, only the email and the folder
//! name (used as the account label).

use serde_json::Value;
use std::path::Path;

/// One account found in TcNo's cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TcnoAccount {
    /// The TcNo folder name, used as the imported account's label.
    pub label: String,
    pub email: String,
}

/// Pull an email out of any shape TcNo's `email.json` has used:
/// - a bare string: `"player@example.com"`
/// - an array: `["player@example.com", ...]`
/// - an object: `{"email": "player@example.com"}` (any key; the first email-looking string wins).
pub fn parse_email_json(text: &str) -> Option<String> {
    let v: Value = serde_json::from_str(text).ok()?;
    email_from_value(&v)
}

fn looks_like_email(s: &str) -> bool {
    let s = s.trim();
    s.contains('@') && !s.contains(char::is_whitespace) && s.len() >= 3
}

fn email_from_value(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if looks_like_email(s) => Some(s.trim().to_string()),
        Value::String(_) => None,
        Value::Array(items) => items.iter().find_map(email_from_value),
        Value::Object(map) => {
            // Prefer a conventional key, then fall back to any email-looking string value.
            for key in ["email", "Email", "AccountEmail", "accountEmail", "username", "Username"] {
                if let Some(Value::String(s)) = map.get(key) {
                    if looks_like_email(s) {
                        return Some(s.trim().to_string());
                    }
                }
            }
            map.values().find_map(email_from_value)
        }
        _ => None,
    }
}

/// Every TcNo Battle.net account on disk. A missing cache directory is an empty list.
pub fn find_accounts(tcno_battlenet_dir: &Path) -> Vec<TcnoAccount> {
    let entries = match std::fs::read_dir(tcno_battlenet_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let label = entry.file_name().to_string_lossy().to_string();
        let email_file = entry.path().join("email.json");
        if let Ok(text) = std::fs::read_to_string(&email_file) {
            if let Some(email) = parse_email_json(&text) {
                out.push(TcnoAccount { label, email });
            }
        }
    }
    // Stable order by label so imports and the found-count are deterministic.
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_bare_string() {
        assert_eq!(parse_email_json("\"a@b.com\""), Some("a@b.com".to_string()));
    }

    #[test]
    fn parses_an_array() {
        assert_eq!(parse_email_json("[\"first@b.com\", \"second@b.com\"]"), Some("first@b.com".to_string()));
        // An array whose first element is not an email falls through to the next.
        assert_eq!(parse_email_json("[\"notanemail\", \"real@b.com\"]"), Some("real@b.com".to_string()));
    }

    #[test]
    fn parses_an_object_by_key_and_by_value() {
        assert_eq!(parse_email_json(r#"{"email":"x@y.com"}"#), Some("x@y.com".to_string()));
        assert_eq!(parse_email_json(r#"{"AccountEmail":"z@y.com","other":1}"#), Some("z@y.com".to_string()));
        // No conventional key, but an email-looking value exists.
        assert_eq!(parse_email_json(r#"{"whatever":"found@y.com"}"#), Some("found@y.com".to_string()));
    }

    #[test]
    fn rejects_shapes_with_no_email() {
        assert_eq!(parse_email_json("\"nope\""), None);
        assert_eq!(parse_email_json("123"), None);
        assert_eq!(parse_email_json("{}"), None);
        assert_eq!(parse_email_json("not json"), None);
    }

    #[test]
    fn finds_accounts_in_a_cache_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (folder, content) in [("Alice", "\"alice@b.com\""), ("Bob", r#"{"email":"bob@b.com"}"#)] {
            let f = root.join(folder);
            std::fs::create_dir_all(&f).unwrap();
            std::fs::write(f.join("email.json"), content).unwrap();
        }
        // A folder with no email.json is skipped.
        std::fs::create_dir_all(root.join("Empty")).unwrap();

        let found = find_accounts(root);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], TcnoAccount { label: "Alice".into(), email: "alice@b.com".into() });
        assert_eq!(found[1], TcnoAccount { label: "Bob".into(), email: "bob@b.com".into() });
    }

    #[test]
    fn missing_dir_is_empty() {
        assert!(find_accounts(Path::new("/no/such/tcno/dir")).is_empty());
    }
}
