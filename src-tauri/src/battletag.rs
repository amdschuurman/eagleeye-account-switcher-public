//! The BattleTag of each account, from Battle.net's own cache `%LOCALAPPDATA%\Battle.net\CachedData.db`.
//!
//! Mapping (verified read-only on a real file, 2026-10-23, 23 `login_cache` rows): the 8-hex account
//! code used in `Roaming\Battle.net\<code>.config` and in the log line `account: 1:0:<code>` is the
//! lower 32 bits of the Battle.net account id written in hex without leading zeros, and
//! `login_cache.account_id_lo` holds that same number as an integer (`account_id_hi` is the constant
//! region/kind prefix `0x0100000000000000`). So `code -> u32::from_str_radix(code, 16) == account_id_lo`.
//! 11 of the 25 `.config` codes have a cache row and all 11 match this way, as do both codes in the
//! newest log; the other 14 (stale or non-account configs) match no row by any key, FNV-1a of the
//! id, tag or login name included. An account never signed in on this PC simply has no BattleTag yet.
//!
//! The database is opened with `immutable=1` and read-only flags: Battle.net may hold it open, and
//! we never take a lock or write to it. Only `battle_tag` is read; the `name` column (the login
//! email) is never selected.

use crate::paths::Paths;
use crate::store;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

/// The account id low word for an account code (`1a2b3c4d` -> 0x1a2b3c4d). `None` for placeholder
/// codes (TcNo imports not yet signed in) and anything that is not hex.
pub fn code_to_id_lo(code: &str) -> Option<u32> {
    let c = code.trim();
    if c.is_empty() || c.len() > 8 || !c.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(c, 16).ok()
}

/// A `file:` URI for SQLite with `immutable=1`. Windows paths get forward slashes and a leading `/`
/// before the drive letter; characters SQLite treats specially in a URI are percent-encoded.
fn sqlite_uri(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let mut enc = String::with_capacity(raw.len() + 16);
    for ch in raw.chars() {
        match ch {
            '%' => enc.push_str("%25"),
            '?' => enc.push_str("%3f"),
            '#' => enc.push_str("%23"),
            ' ' => enc.push_str("%20"),
            c => enc.push(c),
        }
    }
    let lead = if enc.as_bytes().get(1) == Some(&b':') { "/" } else { "" };
    format!("file:{lead}{enc}?immutable=1&mode=ro")
}

/// Open the cache read-only without taking any lock.
fn open_readonly(db: &Path) -> Result<Connection, String> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    Connection::open_with_flags(sqlite_uri(db), flags).map_err(|_| "could not open Battle.net's CachedData.db".into())
}

/// The BattleTag (`Name#1234`) of one account, or `None` when the cache has no row for it (never
/// signed in on this PC) or the file is missing.
pub fn lookup(db: &Path, code: &str) -> Result<Option<String>, String> {
    let Some(lo) = code_to_id_lo(code) else { return Ok(None) };
    if !db.exists() {
        return Ok(None);
    }
    let conn = open_readonly(db)?;
    let mut stmt = conn
        .prepare(
            "SELECT battle_tag FROM login_cache WHERE account_id_lo = ?1 AND battle_tag <> '' \
             ORDER BY environment",
        )
        .map_err(|_| "CachedData.db has no readable login_cache".to_string())?;
    let mut rows = stmt.query([lo as i64]).map_err(|_| "login_cache query failed".to_string())?;
    match rows.next().map_err(|_| "login_cache read failed".to_string())? {
        Some(row) => {
            let tag: String = row.get(0).map_err(|_| "login_cache row unreadable".to_string())?;
            Ok(is_battletag(&tag).then_some(tag))
        }
        None => Ok(None),
    }
}

/// `Name#1234` shape check (name non-empty, 4 or more digits after the `#`).
pub fn is_battletag(tag: &str) -> bool {
    match tag.rsplit_once('#') {
        Some((name, num)) => !name.is_empty() && num.len() >= 4 && num.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

/// The OverFast player id for a BattleTag: `Name#1234` -> `Name-1234`.
pub fn to_player_id(tag: &str) -> Option<String> {
    let (name, num) = tag.rsplit_once('#')?;
    is_battletag(tag).then(|| format!("{name}-{num}"))
}

/// ENGINE HOOK. Look up `code`'s BattleTag and store it on the account in `accounts.json` when it is
/// new or changed. Returns the BattleTag (stored or already present). Safe to call after every
/// successful switch, add or save: one small read-only query, one write only when the value changed.
pub fn fill_battletag(paths: &Paths, code: &str) -> Result<Option<String>, String> {
    let Some(tag) = lookup(&paths.battlenet_cached_db(), code)? else {
        return Ok(None);
    };
    let path = paths.accounts_json();
    let mut accounts = store::load_accounts(&path)?;
    let Some(acc) = accounts.iter_mut().find(|a| a.code == code) else {
        return Ok(Some(tag));
    };
    if acc.battletag.as_deref() != Some(tag.as_str()) {
        acc.battletag = Some(tag.clone());
        store::save_accounts(&path, &accounts)?;
    }
    Ok(Some(tag))
}

/// Fill every account that has no BattleTag yet (one database open, one write at most). Returns how
/// many were filled. Used lazily by `list_accounts` so accounts added before Phase 2 get theirs.
pub fn fill_missing(paths: &Paths) -> Result<usize, String> {
    let path = paths.accounts_json();
    let mut accounts = store::load_accounts(&path)?;
    let db = paths.battlenet_cached_db();
    if !db.exists() || accounts.iter().all(|a| a.battletag.is_some() || code_to_id_lo(&a.code).is_none()) {
        return Ok(0);
    }
    let mut filled = 0;
    for acc in accounts.iter_mut().filter(|a| a.battletag.is_none()) {
        if let Ok(Some(tag)) = lookup(&db, &acc.code) {
            acc.battletag = Some(tag);
            filled += 1;
        }
    }
    if filled > 0 {
        store::save_accounts(&path, &accounts)?;
    }
    Ok(filled)
}

/// ENGINE HOOK (Tauri side). Call after a sign-in succeeded (switch "done", add, save): finds the
/// signed-in code (from the newest log, falling back to `code_hint`, so a TcNo placeholder that just
/// got its real code works too), stores its BattleTag, then refreshes that account's ranks on a
/// background thread (forced) and emits `ranks-updated`. Never blocks the caller on the network.
pub fn after_sign_in<R: tauri::Runtime>(app: &tauri::AppHandle<R>, code_hint: &str) {
    use tauri::Manager;
    let be = app.state::<std::sync::Arc<crate::engine::Backend>>().inner().clone();
    let code = be
        .current_account()
        .ok()
        .flatten()
        .filter(|c| code_to_id_lo(c).is_some())
        .unwrap_or_else(|| code_hint.to_string());
    if code_hint.starts_with("tcno-") && code != code_hint {
        let _ = crate::store::move_category(&be.paths.meta_json(), code_hint, &code);
    }
    if !be.busy.load(std::sync::atomic::Ordering::SeqCst) {
        let _ = fill_battletag(&be.paths, &code);
    }
    crate::ranks::spawn_refresh(app.clone(), be.paths.clone(), Some(code), true);
}

#[cfg(test)]
pub mod test_support {
    use rusqlite::Connection;
    use std::path::Path;

    /// A fixture `CachedData.db` with Battle.net's real `login_cache` schema and fake rows.
    pub fn make_cache_db(path: &Path, rows: &[(u32, &str, &str)]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE login_cache (name TEXT NOT NULL, environment TEXT NOT NULL, battle_tag TEXT NOT NULL, \
             account_id_hi INT NOT NULL, account_id_lo INT NOT NULL, connected_environments TEXT NOT NULL DEFAULT '', \
             UNIQUE(account_id_hi, account_id_lo, environment) ON CONFLICT REPLACE)",
        )
        .unwrap();
        for (lo, env, tag) in rows {
            conn.execute(
                "INSERT INTO login_cache VALUES ('fake@example.invalid', ?1, ?2, 72057594037927936, ?3, 'EU,KR,US,XX')",
                rusqlite::params![env, tag, *lo as i64],
            )
            .unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::make_cache_db;
    use super::*;
    use crate::store::{Account, Status};
    use std::path::PathBuf;

    #[test]
    fn code_maps_to_account_id_lo() {
        assert_eq!(code_to_id_lo("1a2b3c4d"), Some(0x1a2b3c4d));
        // Codes are written without leading zeros (7 chars happen).
        assert_eq!(code_to_id_lo("5e6f708"), Some(0x05e6f708));
        assert_eq!(code_to_id_lo("tcno-Alice"), None);
        assert_eq!(code_to_id_lo(""), None);
        assert_eq!(code_to_id_lo("123456789"), None);
    }

    #[test]
    fn battletag_shapes() {
        assert!(is_battletag("TeKrop#2217"));
        assert!(is_battletag("Ärik#4821"));
        assert!(!is_battletag("NoHash"));
        assert!(!is_battletag("#1234"));
        assert!(!is_battletag("Name#12"));
        assert_eq!(to_player_id("TeKrop#2217").as_deref(), Some("TeKrop-2217"));
        assert_eq!(to_player_id("bad"), None);
    }

    #[test]
    fn uri_handles_windows_paths_and_spaces() {
        assert_eq!(
            sqlite_uri(&PathBuf::from(r"C:\Users\A B\AppData\Local\Battle.net\CachedData.db")), // check-private: ok (fake path)
            "file:/C:/Users/A%20B/AppData/Local/Battle.net/CachedData.db?immutable=1&mode=ro"
        );
        assert_eq!(sqlite_uri(&PathBuf::from("/tmp/x#y/c.db")), "file:/tmp/x%23y/c.db?immutable=1&mode=ro");
    }

    #[test]
    fn lookup_finds_the_row_by_code() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("dir with space").join("CachedData.db");
        make_cache_db(
            &db,
            &[(0x1a2b3c4d, "eu.actual.battle.net", "Alice#4821"), (0x5e6f708, "eu.actual.battle.net", "Bob#7350")],
        );
        assert_eq!(lookup(&db, "1a2b3c4d").unwrap().as_deref(), Some("Alice#4821"));
        assert_eq!(lookup(&db, "5e6f708").unwrap().as_deref(), Some("Bob#7350"));
        assert_eq!(lookup(&db, "deadbeef").unwrap(), None);
        assert_eq!(lookup(&db, "tcno-x").unwrap(), None);
        assert_eq!(lookup(&dir.path().join("missing.db"), "1a2b3c4d").unwrap(), None);
    }

    fn account(code: &str, tag: Option<&str>) -> Account {
        Account {
            code: code.into(),
            label: "T".into(),
            battletag: tag.map(Into::into),
            email_masked: "t***@e***.invalid".into(),
            order: 0,
            last_used: None,
            status: Status::Ready,
            added_at: 1,
        }
    }

    #[test]
    fn fill_writes_the_tag_into_accounts_json() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().join("app"), dir.path().join("local"));
        make_cache_db(&paths.battlenet_cached_db(), &[(0xaaaa0001, "eu.actual.battle.net", "Alice#1234")]);
        store::save_accounts(
            &paths.accounts_json(),
            &[account("aaaa0001", None), account("bbbb0002", None), account("tcno-x", None)],
        )
        .unwrap();

        assert_eq!(fill_battletag(&paths, "aaaa0001").unwrap().as_deref(), Some("Alice#1234"));
        let accs = store::load_accounts(&paths.accounts_json()).unwrap();
        assert_eq!(accs[0].battletag.as_deref(), Some("Alice#1234"));
        assert_eq!(accs[1].battletag, None);
        assert_eq!(fill_battletag(&paths, "bbbb0002").unwrap(), None);

        // fill_missing: nothing more to find for bbbb0002; an added row is picked up.
        assert_eq!(fill_missing(&paths).unwrap(), 0);
        let db = paths.battlenet_cached_db();
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "INSERT INTO login_cache VALUES ('fake@example.invalid', 'eu.actual.battle.net', 'Smurf#9999', 72057594037927936, ?1, '')",
            [0xbbbb0002u32 as i64],
        )
        .unwrap();
        drop(conn);
        assert_eq!(fill_missing(&paths).unwrap(), 1);
        let accs = store::load_accounts(&paths.accounts_json()).unwrap();
        assert_eq!(accs[1].battletag.as_deref(), Some("Smurf#9999"));
    }
}
