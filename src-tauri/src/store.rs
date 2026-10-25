//! The non-secret store: `accounts.json` (the account list the UI shows) and `settings.json`. No
//! tokens or raw emails live here; the email is kept masked (`pi***@g***.com`). Both files are read
//! leniently (missing = defaults) and written atomically.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// We hold a token for this account; a switch logs it in with no password.
    Ready,
    /// No usable token (import without login, token from another Windows user, rotated out): the
    /// user logs in once and we capture the token.
    NeedsLogin,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// The 8-hex Battle.net account code. This is the id.
    pub code: String,
    /// User-facing name for the account.
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub battletag: Option<String>,
    /// Masked email, e.g. `pi***@g***.com`. Never the full address.
    pub email_masked: String,
    pub order: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_used: Option<u64>,
    pub status: Status,
    pub added_at: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// The Battle.net install folder. `None` = use the registry/default.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub battlenet_path: Option<String>,
    #[serde(default)]
    pub close_after_switch: bool,
    #[serde(default)]
    pub minimize_to_tray: bool,
    /// Base URL of the OverFast API the rank lookups use (no trailing slash needed).
    #[serde(default = "default_overfast_url")]
    pub overfast_url: String,
    /// Show the app in Discord (Rich Presence) while its window is open. On by default.
    #[serde(default = "default_true")]
    pub discord_presence: bool,
}

fn default_true() -> bool {
    true
}

/// The public OverFast instance.
pub const DEFAULT_OVERFAST_URL: &str = "https://overfast-api.tekrop.fr";

fn default_overfast_url() -> String {
    DEFAULT_OVERFAST_URL.to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            battlenet_path: None,
            close_after_switch: false,
            minimize_to_tray: false,
            overfast_url: default_overfast_url(),
            discord_presence: true,
        }
    }
}

impl Settings {
    /// The OverFast base URL to use: the setting, or the public instance when it is blank.
    pub fn overfast_base(&self) -> String {
        let u = self.overfast_url.trim().trim_end_matches('/');
        if u.is_empty() { DEFAULT_OVERFAST_URL.to_string() } else { u.to_string() }
    }
}

/// The group an account is shown under. Kept in `meta.json` (not `accounts.json`) so the engine's
/// account writes never drop it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Shown under the Main tab.
    Main,
    /// Shown under the Smurf tab (the default for a new account).
    #[default]
    Smurf,
}

/// Per-account metadata the engine does not own.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    #[serde(default)]
    pub categories: std::collections::BTreeMap<String, Category>,
}

impl Meta {
    pub fn category(&self, code: &str) -> Category {
        self.categories.get(code).copied().unwrap_or_default()
    }
}

pub fn load_meta(path: &Path) -> Result<Meta, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("meta.json is unreadable: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Meta::default()),
        Err(_) => Err("could not read meta.json".into()),
    }
}

/// An imported account moves from its placeholder code to its real code on first sign-in: carry its
/// category along (seen in testing: the category stayed behind under the placeholder).
pub fn move_category(path: &Path, from: &str, to: &str) -> Result<(), String> {
    let mut meta = load_meta(path)?;
    if let Some(cat) = meta.categories.remove(from) {
        meta.categories.insert(to.to_string(), cat);
        save_meta(path, &meta)?;
    }
    Ok(())
}

pub fn save_meta(path: &Path, meta: &Meta) -> Result<(), String> {
    let text = serde_json::to_string_pretty(meta).map_err(|_| "could not encode meta.json".to_string())?;
    crate::atomic::write(path, text.as_bytes())
}

/// What the UI receives for an account: the engine's `Account` plus its `category`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    #[serde(flatten)]
    pub account: Account,
    pub category: Category,
}

/// Attach each account's category (default Smurf).
pub fn with_categories(accounts: Vec<Account>, meta: &Meta) -> Vec<AccountView> {
    accounts
        .into_iter()
        .map(|a| {
            let category = meta.category(&a.code);
            AccountView { account: a, category }
        })
        .collect()
}

/// Mask an email as `pi***@g***.com`: the first two characters of the local part, then the first
/// character of the domain, then the top-level part after the last dot.
pub fn mask_email(email: &str) -> String {
    let email = email.trim();
    let Some((local, domain)) = email.split_once('@') else {
        // Not an address shape: mask all but the first char.
        let head: String = email.chars().take(1).collect();
        return format!("{head}***");
    };
    let local_head: String = local.chars().take(2).collect();
    let domain_head: String = domain.chars().take(1).collect();
    let tld = domain.rsplit_once('.').map(|(_, t)| t).unwrap_or("");
    if tld.is_empty() {
        format!("{local_head}***@{domain_head}***")
    } else {
        format!("{local_head}***@{domain_head}***.{tld}")
    }
}

pub fn load_accounts(path: &Path) -> Result<Vec<Account>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("accounts.json is unreadable: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(_) => Err("could not read accounts.json".into()),
    }
}

pub fn save_accounts(path: &Path, accounts: &[Account]) -> Result<(), String> {
    let text = serde_json::to_string_pretty(accounts).map_err(|_| "could not encode accounts.json".to_string())?;
    crate::atomic::write(path, text.as_bytes())
}

pub fn load_settings(path: &Path) -> Result<Settings, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("settings.json is unreadable: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(_) => Err("could not read settings.json".into()),
    }
}

pub fn save_settings(path: &Path, settings: &Settings) -> Result<(), String> {
    let text = serde_json::to_string_pretty(settings).map_err(|_| "could not encode settings.json".to_string())?;
    crate::atomic::write(path, text.as_bytes())
}

/// The next `order` value (max + 1, or 0 when empty) so a new account lands at the end.
pub fn next_order(accounts: &[Account]) -> u32 {
    accounts.iter().map(|a| a.order).max().map_or(0, |m| m + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_emails() {
        assert_eq!(mask_email("carol@gmail.com"), "ca***@g***.com");
        assert_eq!(mask_email("alice@outlook.co.uk"), "al***@o***.uk");
        assert_eq!(mask_email("a@b.io"), "a***@b***.io");
        // No dot in the domain.
        assert_eq!(mask_email("x@localhost"), "x***@l***");
        // Not an email.
        assert_eq!(mask_email("weird"), "w***");
    }

    #[test]
    fn status_serializes_snake_case() {
        assert_eq!(serde_json::to_string(&Status::NeedsLogin).unwrap(), "\"needs_login\"");
        assert_eq!(serde_json::to_string(&Status::Ready).unwrap(), "\"ready\"");
    }

    #[test]
    fn accounts_roundtrip_through_disk_with_camel_case() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts.json");
        let accs = vec![Account {
            code: "12ab34cd".into(),
            label: "Alice".into(),
            battletag: Some("Alice#1234".into()),
            email_masked: "ni***@g***.com".into(),
            order: 0,
            last_used: Some(1_700_000_000_000),
            status: Status::Ready,
            added_at: 1_699_000_000_000,
        }];
        save_accounts(&path, &accs).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"emailMasked\""));
        assert!(text.contains("\"lastUsed\""));
        assert!(text.contains("\"addedAt\""));
        assert_eq!(load_accounts(&path).unwrap(), accs);
    }

    #[test]
    fn missing_files_give_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_accounts(&dir.path().join("none.json")).unwrap().is_empty());
        assert_eq!(load_settings(&dir.path().join("none.json")).unwrap(), Settings::default());
    }

    #[test]
    fn settings_default_overfast_url_and_old_files_still_load() {
        assert_eq!(Settings::default().overfast_url, DEFAULT_OVERFAST_URL);
        // A settings.json written before Phase 2 has no overfastUrl: the default fills in.
        let s: Settings = serde_json::from_str(r#"{"closeAfterSwitch":true}"#).unwrap();
        assert_eq!(s.overfast_url, DEFAULT_OVERFAST_URL);
        assert!(s.close_after_switch);
        let custom = Settings { overfast_url: " http://overfast.invalid:8080/ ".into(), ..Default::default() };
        assert_eq!(custom.overfast_base(), "http://overfast.invalid:8080");
        let blank = Settings { overfast_url: "  ".into(), ..Default::default() };
        assert_eq!(blank.overfast_base(), DEFAULT_OVERFAST_URL);
        assert!(serde_json::to_string(&Settings::default()).unwrap().contains("\"overfastUrl\""));
    }

    #[test]
    fn discord_presence_defaults_on_and_old_files_still_load() {
        assert!(Settings::default().discord_presence);
        // A settings.json written before the Discord presence setting existed: it defaults to on.
        let s: Settings = serde_json::from_str(r#"{"closeAfterSwitch":true,"minimizeToTray":true}"#).unwrap();
        assert!(s.discord_presence);
        let off: Settings = serde_json::from_str(r#"{"discordPresence":false}"#).unwrap();
        assert!(!off.discord_presence);
        assert!(serde_json::to_string(&Settings::default()).unwrap().contains("\"discordPresence\":true"));
    }

    #[test]
    fn categories_default_to_smurf_and_flatten_into_the_account() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.json");
        assert_eq!(load_meta(&path).unwrap(), Meta::default());
        let mut meta = Meta::default();
        meta.categories.insert("aaaa0001".into(), Category::Main);
        save_meta(&path, &meta).unwrap();
        let meta = load_meta(&path).unwrap();
        let acc = |code: &str| Account {
            code: code.into(),
            label: "A".into(),
            battletag: None,
            email_masked: "a***@b***.c".into(),
            order: 0,
            last_used: None,
            status: Status::Ready,
            added_at: 1,
        };
        let views = with_categories(vec![acc("aaaa0001"), acc("bbbb0002")], &meta);
        assert_eq!(views[0].category, Category::Main);
        assert_eq!(views[1].category, Category::Smurf);
        let v = serde_json::to_value(&views[1]).unwrap();
        assert_eq!(v["category"], "smurf");
        assert_eq!(v["code"], "bbbb0002");
        assert_eq!(v["emailMasked"], "a***@b***.c");
    }

    #[test]
    fn next_order_increments() {
        assert_eq!(next_order(&[]), 0);
    }
}

#[cfg(test)]
mod move_category_tests {
    use super::*;

    #[test]
    fn category_follows_the_account_to_its_real_code() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("meta.json");
        let mut m = Meta::default();
        m.categories.insert("tcno-dana".into(), Category::Main);
        save_meta(&p, &m).unwrap();
        move_category(&p, "tcno-dana", "1a2b3c4d").unwrap();
        let m = load_meta(&p).unwrap();
        assert_eq!(m.category("1a2b3c4d"), Category::Main);
        assert!(!m.categories.contains_key("tcno-dana"));
        // Nothing to move is not an error.
        move_category(&p, "tcno-none", "aaaa0001").unwrap();
    }
}
