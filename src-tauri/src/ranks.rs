//! Overwatch ranks per account: the OverFast profile cache, manual overrides, and the merge the UI
//! reads (DESIGN.md section 3, "Phase 2 command contract").
//!
//! `ranks.json` in the store holds, per account code, the last profile lookup (`entries`) and the
//! manual values per role (`overrides`). Reads (`get_ranks`, `get_all_ranks`) never touch the
//! network. A refresh fetches an account at most once per hour (10 minutes after a failure, and for a
//! BattleTag OverFast does not know yet, not before OverFast's own recheck time) unless forced, and
//! emits `ranks-updated {code}` per account it wrote, or `ranks-error {code, error}` when the refresh
//! itself could not run.
//!
//! Player icons: a lookup's `avatar` URL is kept per entry (a lookup without one keeps the previous URL), the image is
//! fetched once into `icons/avatars` after the lookup, and `RankInfo.avatarFile` names the local file only while it is
//! on disk. A failed download never fails the rank refresh; the next refresh tries again.
//!
//! Manual wins until a newer season: an override remembers the season that was current when it was
//! set; when the profile reports a later season the override is dropped.

use crate::overfast::{self, Http, Lookup, ProfileRoles};
use crate::paths::Paths;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

/// A profile entry younger than this is not fetched again unless forced.
pub const FRESH_MS: u64 = 60 * 60 * 1000;
/// After a failed lookup, wait this long before asking again (no retry storm).
pub const ERROR_RETRY_MS: u64 = 10 * 60 * 1000;

/// Competitive tiers, lowest to highest. Emerald sits between Platinum and Diamond (live since the
/// 2026 rank rework: the profile shows `Rank_EmeraldTier`, OverFast reports `emerald`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Bronze,
    Silver,
    Gold,
    Platinum,
    Emerald,
    Diamond,
    Master,
    Grandmaster,
    Champion,
}

impl Tier {
    pub const ALL: [Tier; 9] = [
        Tier::Bronze,
        Tier::Silver,
        Tier::Gold,
        Tier::Platinum,
        Tier::Emerald,
        Tier::Diamond,
        Tier::Master,
        Tier::Grandmaster,
        Tier::Champion,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Bronze => "bronze",
            Tier::Silver => "silver",
            Tier::Gold => "gold",
            Tier::Platinum => "platinum",
            Tier::Emerald => "emerald",
            Tier::Diamond => "diamond",
            Tier::Master => "master",
            Tier::Grandmaster => "grandmaster",
            Tier::Champion => "champion",
        }
    }

    pub fn parse(s: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|t| t.as_str() == s)
    }
}

/// A tier plus its division (1 is the top of the tier, 5 the bottom).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierDiv {
    pub tier: Tier,
    pub division: u8,
}

impl TierDiv {
    /// A number that orders ranks: Bronze 5 = 1 ... Champion 1 = 45.
    pub fn value(self) -> u32 {
        (self.tier as u32) * 5 + (6 - u32::from(self.division.clamp(1, 5)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Tank,
    Damage,
    Support,
    Open,
}

impl Role {
    pub const ALL: [Role; 4] = [Role::Tank, Role::Damage, Role::Support, Role::Open];

    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "tank" => Some(Role::Tank),
            "damage" => Some(Role::Damage),
            "support" => Some(Role::Support),
            "open" => Some(Role::Open),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Profile,
    Manual,
}

/// One role's rank as the UI shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleRank {
    pub tier: Tier,
    pub division: u8,
    pub source: Source,
    #[serde(default)]
    pub placement: bool,
    /// Only when `placement` is true.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub predicted: Option<TierDiv>,
}

impl RoleRank {
    fn from_profile(td: TierDiv) -> Self {
        RoleRank { tier: td.tier, division: td.division, source: Source::Profile, placement: false, predicted: None }
    }

    /// The rank used for sorting: the predicted rank while in placements (none = unranked), else
    /// the rank itself.
    pub fn effective(&self) -> Option<TierDiv> {
        if self.placement {
            self.predicted
        } else {
            Some(TierDiv { tier: self.tier, division: self.division })
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roles {
    pub tank: Option<RoleRank>,
    pub damage: Option<RoleRank>,
    pub support: Option<RoleRank>,
    pub open: Option<RoleRank>,
}

impl Roles {
    pub fn get(&self, role: Role) -> Option<&RoleRank> {
        match role {
            Role::Tank => self.tank.as_ref(),
            Role::Damage => self.damage.as_ref(),
            Role::Support => self.support.as_ref(),
            Role::Open => self.open.as_ref(),
        }
    }

    fn slot(&mut self, role: Role) -> &mut Option<RoleRank> {
        match role {
            Role::Tank => &mut self.tank,
            Role::Damage => &mut self.damage,
            Role::Support => &mut self.support,
            Role::Open => &mut self.open,
        }
    }
}

/// What the last lookup of an account said (`ok` also when nothing was looked up yet).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RankStatus {
    #[default]
    Ok,
    Private,
    NotFound,
    Error,
}

/// `get_ranks` / `get_all_ranks` payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RankInfo {
    pub battletag: String,
    pub season: Option<u32>,
    pub updated_at: Option<u64>,
    /// Last attempt, successful or not (ms).
    pub checked_at: Option<u64>,
    pub status: RankStatus,
    /// For `notFound`: when OverFast looks for the player again (ms).
    pub recheck_at: Option<u64>,
    pub private_profile: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub roles: Roles,
    /// The player icon's file name in the icon folder's `avatars` subfolder, only while it is on disk.
    pub avatar_file: Option<String>,
}

// ---- the store file ---------------------------------------------------------------------------

/// The last profile lookup of one account.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub battletag: String,
    #[serde(default)]
    pub season: Option<u32>,
    /// Last successful lookup (ms).
    #[serde(default)]
    pub updated_at: Option<u64>,
    /// Last attempt, successful or not (ms).
    #[serde(default)]
    pub attempted_at: u64,
    #[serde(default)]
    pub private_profile: bool,
    #[serde(default)]
    pub not_found: bool,
    /// For `not_found`: when OverFast looks for the player again (ms).
    #[serde(default)]
    pub recheck_at: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub tank: Option<TierDiv>,
    #[serde(default)]
    pub damage: Option<TierDiv>,
    #[serde(default)]
    pub support: Option<TierDiv>,
    #[serde(default)]
    pub open: Option<TierDiv>,
    /// The player icon URL from the last lookup that had one.
    #[serde(default)]
    pub avatar_url: Option<String>,
}

impl Entry {
    fn profile(&self, role: Role) -> Option<TierDiv> {
        match role {
            Role::Tank => self.tank,
            Role::Damage => self.damage,
            Role::Support => self.support,
            Role::Open => self.open,
        }
    }
}

/// A manual value for one role, with the season it was set in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Override {
    pub rank: RoleRank,
    #[serde(default)]
    pub season: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankStore {
    /// Highest season any profile reported (lets the UI grey a stale one).
    #[serde(default)]
    pub newest_season: Option<u32>,
    #[serde(default)]
    pub entries: BTreeMap<String, Entry>,
    #[serde(default)]
    pub overrides: BTreeMap<String, BTreeMap<Role, Override>>,
}

pub fn load_store(path: &Path) -> Result<RankStore, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("ranks.json is unreadable: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RankStore::default()),
        Err(_) => Err("could not read ranks.json".into()),
    }
}

pub fn save_store(path: &Path, store: &RankStore) -> Result<(), String> {
    let text = serde_json::to_string_pretty(store).map_err(|_| "could not encode ranks.json".to_string())?;
    crate::atomic::write(path, text.as_bytes())
}

/// Serialises every read-modify-write of `ranks.json` within this process.
static STORE_LOCK: Mutex<()> = Mutex::new(());
/// Codes with a network lookup in flight (a second refresh of the same code is skipped).
static IN_FLIGHT: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn with_store<T>(paths: &Paths, f: impl FnOnce(&mut RankStore) -> T) -> Result<T, String> {
    let _g = STORE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let path = paths.ranks_json();
    let mut store = load_store(&path)?;
    let out = f(&mut store);
    save_store(&path, &store)?;
    Ok(out)
}

fn read_store(paths: &Paths) -> Result<RankStore, String> {
    let _g = STORE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    load_store(&paths.ranks_json())
}

// ---- merge ------------------------------------------------------------------------------------

/// Is the manual value still in force against this profile season?
fn override_holds(ov: &Override, profile_season: Option<u32>) -> bool {
    match (ov.season, profile_season) {
        (Some(set), Some(now)) => now <= set,
        _ => true,
    }
}

/// The merged view of one account. `None` when there is nothing to show (no BattleTag known and no
/// manual value).
pub fn rank_info(store: &RankStore, code: &str, battletag: Option<&str>) -> Option<RankInfo> {
    let tag = battletag.unwrap_or("");
    // A cached entry for a different BattleTag (account renamed its tag) is not shown.
    let entry = store.entries.get(code).filter(|e| !tag.is_empty() && e.battletag == tag);
    let overrides = store.overrides.get(code);
    if tag.is_empty() && overrides.map_or(true, |o| o.is_empty()) {
        return None;
    }
    let season = entry.and_then(|e| e.season);
    let mut roles = Roles::default();
    for role in Role::ALL {
        let manual = overrides.and_then(|o| o.get(&role)).filter(|ov| override_holds(ov, season));
        *roles.slot(role) = match manual {
            Some(ov) => Some(ov.rank.clone()),
            None => entry.and_then(|e| e.profile(role)).map(RoleRank::from_profile),
        };
    }
    // The newest attempt decides: a failed retry is an error even over an older not-found.
    let status = match entry {
        Some(e) if e.error.is_some() => RankStatus::Error,
        Some(e) if e.not_found => RankStatus::NotFound,
        Some(e) if e.private_profile => RankStatus::Private,
        _ => RankStatus::Ok,
    };
    Some(RankInfo {
        battletag: tag.to_string(),
        season,
        updated_at: entry.and_then(|e| e.updated_at),
        checked_at: entry.map(|e| e.attempted_at).filter(|t| *t > 0),
        status,
        recheck_at: entry.filter(|e| e.not_found).and_then(|e| e.recheck_at),
        private_profile: entry.map_or(false, |e| e.private_profile),
        error: entry.and_then(|e| e.error.clone()),
        roles,
        avatar_file: entry.and_then(|e| e.avatar_url.as_deref()).and_then(crate::avatars::file_name),
    })
}

/// Drop `avatar_file` when that image is not (yet) on disk, so the UI shows the letter instead of a broken image.
fn with_present_avatar(paths: &Paths, mut info: RankInfo) -> RankInfo {
    let dir = paths.avatars_dir();
    if info.avatar_file.as_ref().is_some_and(|f| !dir.join(f).is_file()) {
        info.avatar_file = None;
    }
    info
}

/// Merged views of every account (code -> info), for the grid badges and sorting.
pub fn all_rank_infos(store: &RankStore, accounts: &[(String, Option<String>)]) -> BTreeMap<String, RankInfo> {
    accounts
        .iter()
        .filter_map(|(code, tag)| rank_info(store, code, tag.as_deref()).map(|i| (code.clone(), i)))
        .collect()
}

pub fn get_ranks(paths: &Paths, code: &str, battletag: Option<&str>) -> Result<Option<RankInfo>, String> {
    Ok(rank_info(&read_store(paths)?, code, battletag).map(|i| with_present_avatar(paths, i)))
}

pub fn get_all_ranks(paths: &Paths, accounts: &[(String, Option<String>)]) -> Result<BTreeMap<String, RankInfo>, String> {
    let all = all_rank_infos(&read_store(paths)?, accounts);
    Ok(all.into_iter().map(|(code, info)| (code, with_present_avatar(paths, info))).collect())
}

// ---- ordering ---------------------------------------------------------------------------------

/// Sort value of one role (0 = unranked). Higher is better.
pub fn role_value(r: Option<&RoleRank>) -> u32 {
    r.and_then(RoleRank::effective).map_or(0, TierDiv::value)
}

/// Sort value of an account: one role, or the best of the four when `role` is `None`.
pub fn account_value(info: Option<&RankInfo>, role: Option<Role>) -> u32 {
    let Some(info) = info else { return 0 };
    match role {
        Some(r) => role_value(info.roles.get(r)),
        None => Role::ALL.iter().map(|r| role_value(info.roles.get(*r))).max().unwrap_or(0),
    }
}

/// Highest rank first, unranked last (stable for equal ranks, so manual order breaks ties).
pub fn compare_desc(a: u32, b: u32) -> Ordering {
    b.cmp(&a)
}

/// Account codes ordered highest rank first (by `role`, or each account's best role when `None`),
/// unranked last; equal ranks keep the given (manual) order.
pub fn sort_codes(codes_in_order: &[String], infos: &BTreeMap<String, RankInfo>, role: Option<Role>) -> Vec<String> {
    let mut v: Vec<(u32, &String)> =
        codes_in_order.iter().map(|c| (account_value(infos.get(c), role), c)).collect();
    v.sort_by(|a, b| compare_desc(a.0, b.0));
    v.into_iter().map(|(_, c)| c.clone()).collect()
}

// ---- mutations --------------------------------------------------------------------------------

fn validate(td: TierDiv) -> Result<(), String> {
    if (1..=5).contains(&td.division) {
        Ok(())
    } else {
        Err("division must be 1 to 5".into())
    }
}

/// Set (or clear with `None`) the manual value of one role.
pub fn set_override(paths: &Paths, code: &str, role: Role, rank: Option<RoleRank>) -> Result<(), String> {
    let rank = match rank {
        Some(mut r) => {
            validate(TierDiv { tier: r.tier, division: r.division })?;
            if let Some(p) = r.predicted {
                validate(p)?;
            }
            r.source = Source::Manual;
            if !r.placement {
                r.predicted = None;
            }
            Some(r)
        }
        None => None,
    };
    with_store(paths, |s| {
        let season = s.entries.get(code).and_then(|e| e.season).or(s.newest_season);
        let per = s.overrides.entry(code.to_string()).or_default();
        match rank {
            Some(r) => {
                per.insert(role, Override { rank: r, season });
            }
            None => {
                per.remove(&role);
            }
        }
        if per.is_empty() {
            s.overrides.remove(code);
        }
    })
}

/// Drop everything stored for an account (it was removed).
pub fn forget(paths: &Paths, code: &str) -> Result<(), String> {
    with_store(paths, |s| {
        s.entries.remove(code);
        s.overrides.remove(code);
    })
}

/// Should this account be looked up now?
pub fn needs_refresh(entry: Option<&Entry>, battletag: &str, now: u64, force: bool) -> bool {
    let Some(e) = entry else { return true };
    if force || e.battletag != battletag {
        return true;
    }
    if e.error.is_some() {
        return now.saturating_sub(e.attempted_at) >= ERROR_RETRY_MS;
    }
    // OverFast answers its cached 404 until its own recheck time, so asking sooner is wasted.
    if e.not_found {
        return now >= (e.attempted_at + ERROR_RETRY_MS).max(e.recheck_at.unwrap_or(0));
    }
    now.saturating_sub(e.attempted_at) >= FRESH_MS
}

/// Write one lookup result into the store: profile data, season bookkeeping, and dropping manual
/// values that a newer season has overtaken.
pub fn apply_lookup(store: &mut RankStore, code: &str, battletag: &str, result: Result<Lookup, String>, now: u64) {
    let prev = store.entries.get(code).filter(|e| e.battletag == battletag).cloned();
    let entry = match result {
        Ok(Lookup::Found { season, roles, private_profile, avatar_url }) => {
            let ProfileRoles { tank, damage, support, open } = roles;
            Entry {
                battletag: battletag.to_string(),
                season,
                updated_at: Some(now),
                attempted_at: now,
                private_profile,
                not_found: false,
                recheck_at: None,
                error: None,
                tank,
                damage,
                support,
                open,
                avatar_url: avatar_url.or_else(|| prev.as_ref().and_then(|p| p.avatar_url.clone())),
            }
        }
        // Nothing was read, so there is no successful lookup time; `attempted_at` says when we asked.
        Ok(Lookup::NotFound { recheck_at }) => Entry {
            battletag: battletag.to_string(),
            attempted_at: now,
            not_found: true,
            recheck_at,
            avatar_url: prev.as_ref().and_then(|p| p.avatar_url.clone()),
            ..Default::default()
        },
        // A failure keeps the last good data and records the error.
        Err(msg) => {
            let mut e = prev.unwrap_or_else(|| Entry { battletag: battletag.to_string(), ..Default::default() });
            e.attempted_at = now;
            e.error = Some(msg);
            e
        }
    };
    if let Some(season) = entry.season {
        store.newest_season = Some(store.newest_season.map_or(season, |n| n.max(season)));
        if let Some(per) = store.overrides.get_mut(code) {
            per.retain(|_, ov| override_holds(ov, Some(season)));
            // A manual value set before any season was known belongs to this season.
            for ov in per.values_mut() {
                ov.season.get_or_insert(season);
            }
            if per.is_empty() {
                store.overrides.remove(code);
            }
        }
    }
    store.entries.insert(code.to_string(), entry);
}

struct InFlightGuard(String);
impl Drop for InFlightGuard {
    fn drop(&mut self) {
        let mut g = IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(set) = g.as_mut() {
            set.remove(&self.0);
        }
    }
}

fn claim(code: &str) -> Option<InFlightGuard> {
    let mut g = IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner());
    let set = g.get_or_insert_with(HashSet::new);
    set.insert(code.to_string()).then(|| InFlightGuard(code.to_string()))
}

/// Refresh the given accounts (code, BattleTag) one after another. Accounts without a BattleTag are
/// skipped. `on_updated(code)` fires after each account whose entry was written. `pause_ms` is slept
/// between network lookups to stay polite to a shared instance. Returns how many were looked up.
pub fn refresh(
    paths: &Paths,
    http: &dyn Http,
    base: &str,
    accounts: &[(String, Option<String>)],
    force: bool,
    now: &dyn Fn() -> u64,
    pause_ms: u64,
    on_updated: &dyn Fn(&str),
) -> Result<usize, String> {
    let mut looked_up = 0;
    for (code, tag) in accounts {
        let Some(tag) = tag.as_deref().filter(|t| crate::battletag::is_battletag(t)) else { continue };
        let due = {
            let store = read_store(paths)?;
            needs_refresh(store.entries.get(code), tag, now(), force)
        };
        if !due {
            continue;
        }
        let Some(_flight) = claim(code) else {
            // A forced refresh of one account still answers, so the UI reloads instead of waiting.
            if force && accounts.len() == 1 {
                on_updated(code);
            }
            continue;
        };
        if looked_up > 0 && pause_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(pause_ms));
        }
        let result = overfast::lookup(http, base, tag);
        looked_up += 1;
        let t = now();
        let avatar = with_store(paths, |s| {
            apply_lookup(s, code, tag, result, t);
            s.entries.get(code).and_then(|e| e.avatar_url.clone())
        })?;
        // The player icon is extra: a failed download leaves the letter and is tried again next refresh.
        if let Some(url) = avatar {
            let _ = crate::avatars::ensure(&paths.avatars_dir(), http, &url);
        }
        on_updated(code);
    }
    Ok(looked_up)
}

/// The (code, BattleTag) pairs to refresh: one account, or every account when `code` is `None`.
pub fn targets(paths: &Paths, code: Option<&str>) -> Result<Vec<(String, Option<String>)>, String> {
    let accounts = crate::store::load_accounts(&paths.accounts_json())?;
    Ok(accounts
        .into_iter()
        .filter(|a| code.map_or(true, |c| a.code == c))
        .map(|a| (a.code, a.battletag))
        .collect())
}

/// Run a refresh on a background thread against the configured OverFast URL and emit
/// `ranks-updated {code}` to the window for every account written. When the refresh itself cannot run
/// (no HTTP client, unreadable store) it emits `ranks-error {code, error}` instead (`code` null for all).
/// Returns at once.
pub fn spawn_refresh<R: tauri::Runtime>(app: tauri::AppHandle<R>, paths: Paths, code: Option<String>, force: bool) {
    use tauri::Emitter as _;
    std::thread::spawn(move || {
        let run = || -> Result<usize, String> {
            let http = overfast::ReqwestHttp::new()?;
            let base = crate::store::load_settings(&paths.settings_json()).unwrap_or_default().overfast_base();
            let list = targets(&paths, code.as_deref())?;
            let emit = |c: &str| {
                let _ = app.emit("ranks-updated", serde_json::json!({ "code": c }));
            };
            refresh(&paths, &http, &base, &list, force, &now_ms, 250, &emit)
        };
        if let Err(error) = run() {
            let _ = app.emit("ranks-error", serde_json::json!({ "code": code, "error": format!("Could not refresh ranks: {error}") }));
        }
    });
}

/// Milliseconds since the UNIX epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overfast::test_support::FakeHttp;
    use std::sync::atomic::{AtomicU64, Ordering as AO};

    const BASE: &str = "https://of.test";
    const TEKROP: &[u8] = include_bytes!("fixtures/overfast_summary_tekrop.json");
    const AVATAR_PNG: &[u8] = b"\x89PNG\r\n\x1a\ninvented avatar";
    const AVATAR_A: &str = "https://d15f34w2p8l1cc.cloudfront.net/overwatch/aaaa000000000000000000000000000000000000000000000000000000000001.png";
    const AVATAR_B: &str = "https://d15f34w2p8l1cc.cloudfront.net/overwatch/bbbb000000000000000000000000000000000000000000000000000000000002.png";

    fn td(tier: Tier, division: u8) -> TierDiv {
        TierDiv { tier, division }
    }

    fn manual(tier: Tier, division: u8) -> RoleRank {
        RoleRank { tier, division, source: Source::Profile, placement: false, predicted: None }
    }

    fn paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::new(dir.path().join("app"), dir.path().join("local"));
        (dir, p)
    }

    #[test]
    fn ordering_champion_1_highest_bronze_5_lowest_unranked_last() {
        assert_eq!(td(Tier::Bronze, 5).value(), 1);
        assert_eq!(td(Tier::Champion, 1).value(), 45);
        assert!(td(Tier::Gold, 1).value() > td(Tier::Gold, 2).value());
        assert!(td(Tier::Platinum, 5).value() > td(Tier::Gold, 1).value());
        assert!(td(Tier::Emerald, 5).value() > td(Tier::Platinum, 1).value());
        assert!(td(Tier::Diamond, 5).value() > td(Tier::Emerald, 1).value());
        // Every tier+division has its own value.
        let mut all: Vec<u32> =
            Tier::ALL.iter().flat_map(|t| (1..=5).map(move |d| td(*t, d).value())).collect();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 45);

        let mut vals = vec![0, td(Tier::Gold, 3).value(), td(Tier::Champion, 1).value(), 0, td(Tier::Bronze, 5).value()];
        vals.sort_by(|a, b| compare_desc(*a, *b));
        assert_eq!(vals, vec![45, td(Tier::Gold, 3).value(), 1, 0, 0]);

        // Placement sorts by the prediction, or as unranked without one.
        let mut p = manual(Tier::Gold, 1);
        p.placement = true;
        assert_eq!(role_value(Some(&p)), 0);
        p.predicted = Some(td(Tier::Master, 2));
        assert_eq!(role_value(Some(&p)), td(Tier::Master, 2).value());
        assert_eq!(role_value(None), 0);
    }

    #[test]
    fn account_value_by_role_or_best() {
        let info = RankInfo {
            avatar_file: None,
            battletag: "A#1234".into(),
            season: Some(22),
            updated_at: None,
            checked_at: None,
            status: RankStatus::Ok,
            recheck_at: None,
            private_profile: false,
            error: None,
            roles: Roles { tank: Some(manual(Tier::Gold, 2)), support: Some(manual(Tier::Diamond, 4)), ..Default::default() },
        };
        assert_eq!(account_value(Some(&info), None), td(Tier::Diamond, 4).value());
        assert_eq!(account_value(Some(&info), Some(Role::Tank)), td(Tier::Gold, 2).value());
        assert_eq!(account_value(Some(&info), Some(Role::Damage)), 0);
        assert_eq!(account_value(None, None), 0);
    }

    #[test]
    fn sort_codes_by_best_or_role_keeps_manual_order_for_ties() {
        let info = |tank: Option<RoleRank>, support: Option<RoleRank>| RankInfo {
            avatar_file: None,
            battletag: "A#1234".into(),
            season: Some(22),
            updated_at: None,
            checked_at: None,
            status: RankStatus::Ok,
            recheck_at: None,
            private_profile: false,
            error: None,
            roles: Roles { tank, support, ..Default::default() },
        };
        let mut infos = BTreeMap::new();
        infos.insert("a".to_string(), info(Some(manual(Tier::Gold, 1)), None));
        infos.insert("b".to_string(), info(None, Some(manual(Tier::Champion, 2))));
        infos.insert("c".to_string(), info(Some(manual(Tier::Gold, 1)), None));
        let order: Vec<String> = ["d", "a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(sort_codes(&order, &infos, None), vec!["b", "a", "c", "d"]);
        assert_eq!(sort_codes(&order, &infos, Some(Role::Tank)), vec!["a", "c", "d", "b"]);
    }

    #[test]
    fn rank_info_serializes_to_the_contract_shape() {
        let mut store = RankStore::default();
        apply_lookup(
            &mut store,
            "aaaa0001",
            "TeKrop#2217",
            Ok(Lookup::Found {
                season: Some(22),
                roles: ProfileRoles { support: Some(td(Tier::Silver, 4)), ..Default::default() },
                private_profile: false,
                avatar_url: None,
            }),
            5000,
        );
        let info = rank_info(&store, "aaaa0001", Some("TeKrop#2217")).unwrap();
        let v = serde_json::to_value(&info).unwrap();
        assert_eq!(v["battletag"], "TeKrop#2217");
        assert_eq!(v["season"], 22);
        assert_eq!(v["updatedAt"], 5000);
        assert_eq!(v["privateProfile"], false);
        assert!(v.get("error").is_none());
        assert_eq!(v["roles"]["tank"], serde_json::Value::Null);
        assert_eq!(v["roles"]["support"]["tier"], "silver");
        assert_eq!(v["roles"]["support"]["division"], 4);
        assert_eq!(v["roles"]["support"]["source"], "profile");
        assert_eq!(v["roles"]["support"]["placement"], false);
        assert!(v["roles"]["support"].get("predicted").is_none());
        assert_eq!(v["avatarFile"], serde_json::Value::Null, "no avatar in the lookup");
        // RoleRank from the UI parses (predicted optional, placement optional).
        let r: RoleRank = serde_json::from_str(
            r#"{"tier":"grandmaster","division":3,"source":"manual","placement":true,"predicted":{"tier":"champion","division":5}}"#,
        )
        .unwrap();
        assert_eq!(r.predicted, Some(td(Tier::Champion, 5)));
    }

    #[test]
    fn no_battletag_and_no_override_is_null() {
        let store = RankStore::default();
        assert_eq!(rank_info(&store, "x", None), None);
        // A known BattleTag with nothing fetched yet is an empty info.
        let i = rank_info(&store, "x", Some("A#1234")).unwrap();
        assert_eq!(i.season, None);
        assert_eq!(i.roles, Roles::default());
    }

    #[test]
    fn manual_wins_until_a_newer_season() {
        let (_d, p) = paths();
        let found = |season, support| {
            Ok(Lookup::Found {
                season: Some(season),
                roles: ProfileRoles { support: Some(support), ..Default::default() },
                private_profile: false,
                avatar_url: None,
            })
        };
        with_store(&p, |s| apply_lookup(s, "c1", "A#1234", found(22, td(Tier::Silver, 4)), 1)).unwrap();
        let mut pl = manual(Tier::Platinum, 2);
        pl.placement = true;
        pl.predicted = Some(td(Tier::Platinum, 2));
        set_override(&p, "c1", Role::Support, Some(pl)).unwrap();

        let i = get_ranks(&p, "c1", Some("A#1234")).unwrap().unwrap();
        let sup = i.roles.support.unwrap();
        assert_eq!(sup.source, Source::Manual);
        assert!(sup.placement);
        assert_eq!(sup.predicted, Some(td(Tier::Platinum, 2)));

        // Same season again: manual still wins.
        with_store(&p, |s| apply_lookup(s, "c1", "A#1234", found(22, td(Tier::Silver, 3)), 2)).unwrap();
        assert_eq!(get_ranks(&p, "c1", Some("A#1234")).unwrap().unwrap().roles.support.unwrap().source, Source::Manual);

        // Newer season: the profile takes over and the override is dropped from the store.
        with_store(&p, |s| apply_lookup(s, "c1", "A#1234", found(23, td(Tier::Gold, 5)), 3)).unwrap();
        let i = get_ranks(&p, "c1", Some("A#1234")).unwrap().unwrap();
        assert_eq!(i.roles.support.unwrap(), RoleRank::from_profile(td(Tier::Gold, 5)));
        assert!(read_store(&p).unwrap().overrides.is_empty());
        assert_eq!(read_store(&p).unwrap().newest_season, Some(23));
    }

    #[test]
    fn override_before_any_season_adopts_the_first_one() {
        let (_d, p) = paths();
        set_override(&p, "c1", Role::Tank, Some(manual(Tier::Master, 1))).unwrap();
        let i = get_ranks(&p, "c1", None).unwrap().unwrap();
        assert_eq!(i.roles.tank.as_ref().unwrap().source, Source::Manual);
        with_store(&p, |s| {
            apply_lookup(
                s,
                "c1",
                "A#1234",
                Ok(Lookup::Found { season: Some(22), roles: ProfileRoles::default(), private_profile: false, avatar_url: None }),
                1,
            )
        })
        .unwrap();
        // Still manual in season 22, and now pinned to it.
        let st = read_store(&p).unwrap();
        assert_eq!(st.overrides["c1"][&Role::Tank].season, Some(22));
        assert_eq!(get_ranks(&p, "c1", Some("A#1234")).unwrap().unwrap().roles.tank.unwrap().tier, Tier::Master);
        // Clearing removes it.
        set_override(&p, "c1", Role::Tank, None).unwrap();
        assert!(read_store(&p).unwrap().overrides.is_empty());
    }

    #[test]
    fn override_validation_and_predicted_only_with_placement() {
        let (_d, p) = paths();
        assert!(set_override(&p, "c1", Role::Open, Some(manual(Tier::Gold, 0))).is_err());
        assert!(set_override(&p, "c1", Role::Open, Some(manual(Tier::Gold, 6))).is_err());
        let mut r = manual(Tier::Gold, 2);
        r.predicted = Some(td(Tier::Gold, 1));
        set_override(&p, "c1", Role::Open, Some(r)).unwrap();
        let st = read_store(&p).unwrap();
        assert_eq!(st.overrides["c1"][&Role::Open].rank.predicted, None);
        assert_eq!(st.overrides["c1"][&Role::Open].rank.source, Source::Manual);
    }

    #[test]
    fn refresh_policy() {
        let e = Entry { battletag: "A#1234".into(), attempted_at: 1_000, ..Default::default() };
        assert!(needs_refresh(None, "A#1234", 0, false));
        assert!(!needs_refresh(Some(&e), "A#1234", 1_000 + FRESH_MS - 1, false));
        assert!(needs_refresh(Some(&e), "A#1234", 1_000 + FRESH_MS, false));
        assert!(needs_refresh(Some(&e), "A#1234", 1_001, true), "force");
        assert!(needs_refresh(Some(&e), "B#1234", 1_001, false), "BattleTag changed");
        let err = Entry { error: Some("x".into()), ..e.clone() };
        assert!(!needs_refresh(Some(&err), "A#1234", 1_000 + ERROR_RETRY_MS - 1, false));
        assert!(needs_refresh(Some(&err), "A#1234", 1_000 + ERROR_RETRY_MS, false));

        // Not found: 10 minutes, or OverFast's own recheck time when that is later.
        let nf = Entry { not_found: true, ..e.clone() };
        assert!(!needs_refresh(Some(&nf), "A#1234", 1_000 + ERROR_RETRY_MS - 1, false));
        assert!(needs_refresh(Some(&nf), "A#1234", 1_000 + ERROR_RETRY_MS, false));
        let later = 1_000 + 67 * 60 * 1000;
        let nf = Entry { not_found: true, recheck_at: Some(later), ..e.clone() };
        assert!(!needs_refresh(Some(&nf), "A#1234", 1_000 + ERROR_RETRY_MS, false));
        assert!(!needs_refresh(Some(&nf), "A#1234", later - 1, false));
        assert!(needs_refresh(Some(&nf), "A#1234", later, false));
        assert!(needs_refresh(Some(&nf), "A#1234", 1_001, true), "force");
        // An earlier recheck time does not shorten the 10 minutes.
        let nf = Entry { not_found: true, recheck_at: Some(2_000), ..e };
        assert!(!needs_refresh(Some(&nf), "A#1234", 3_000, false));
    }

    #[test]
    fn not_found_stores_the_recheck_time_and_maps_to_its_status() {
        let mut store = RankStore::default();
        apply_lookup(&mut store, "c1", "Kestrel#2187", Ok(Lookup::NotFound { recheck_at: Some(9_000_000) }), 5_000);
        assert_eq!(store.entries["c1"].recheck_at, Some(9_000_000));
        let i = rank_info(&store, "c1", Some("Kestrel#2187")).unwrap();
        assert_eq!(i.status, RankStatus::NotFound);
        assert_eq!(i.recheck_at, Some(9_000_000));
        assert_eq!(i.checked_at, Some(5_000));
        assert_eq!(i.updated_at, None, "nothing was read");
        assert_eq!(i.error, None);
        let v = serde_json::to_value(&i).unwrap();
        assert_eq!(v["status"], "notFound");
        assert_eq!(v["recheckAt"], 9_000_000);
        assert_eq!(v["checkedAt"], 5_000);

        // A failed retry: error, keeps the stored entry.
        apply_lookup(&mut store, "c1", "Kestrel#2187", Err("could not reach the rank service".into()), 6_000);
        let i = rank_info(&store, "c1", Some("Kestrel#2187")).unwrap();
        assert_eq!(i.status, RankStatus::Error);
        assert_eq!(i.error.as_deref(), Some("could not reach the rank service"));
        assert_eq!(i.checked_at, Some(6_000));

        // Found later: ok, recheck time gone.
        apply_lookup(
            &mut store,
            "c1",
            "Kestrel#2187",
            Ok(Lookup::Found { season: Some(22), roles: ProfileRoles::default(), private_profile: false, avatar_url: None }),
            7_000,
        );
        let i = rank_info(&store, "c1", Some("Kestrel#2187")).unwrap();
        assert_eq!((i.status, i.recheck_at, i.error), (RankStatus::Ok, None, None));

        apply_lookup(
            &mut store,
            "c1",
            "Kestrel#2187",
            Ok(Lookup::Found { season: None, roles: ProfileRoles::default(), private_profile: true, avatar_url: None }),
            8_000,
        );
        let v = serde_json::to_value(rank_info(&store, "c1", Some("Kestrel#2187")).unwrap()).unwrap();
        assert_eq!(v["status"], "private");

        // Nothing looked up yet: ok and never checked.
        let i = rank_info(&RankStore::default(), "c2", Some("Kestrel#2187")).unwrap();
        assert_eq!((i.status, i.checked_at), (RankStatus::Ok, None));
    }

    #[test]
    fn a_forced_refresh_skipped_by_an_in_flight_lookup_still_answers() {
        let (_d, p) = paths();
        let http = FakeHttp::default();
        let seen = Mutex::new(Vec::new());
        let emit = |c: &str| seen.lock().unwrap().push(c.to_string());
        let one = vec![("eeee0005".to_string(), Some("Kestrel#2187".to_string()))];
        let _busy = claim("eeee0005").unwrap();
        assert_eq!(refresh(&p, &http, BASE, &one, true, &|| 1, 0, &emit).unwrap(), 0);
        assert_eq!(*seen.lock().unwrap(), vec!["eeee0005"]);
        assert_eq!(http.calls(), 0);
        // Not forced: skipped quietly.
        assert_eq!(refresh(&p, &http, BASE, &one, false, &|| 1, 0, &emit).unwrap(), 0);
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn refresh_fetches_caches_and_emits() {
        let (_d, p) = paths();
        let avatar = crate::overfast::parse_summary(TEKROP).unwrap().avatar.unwrap();
        let http = FakeHttp::default()
            .with(&format!("{BASE}/players/TeKrop-2217/summary"), 200, TEKROP)
            .with(&avatar, 200, AVATAR_PNG)
            .with(&format!("{BASE}/players/Gone-1234/summary"), 404, b"{}");
        let clock = AtomicU64::new(10_000);
        let now = || clock.load(AO::SeqCst);
        let seen = Mutex::new(Vec::new());
        let emit = |c: &str| seen.lock().unwrap().push(c.to_string());
        let accounts = vec![
            ("aaaa0001".to_string(), Some("TeKrop#2217".to_string())),
            ("bbbb0002".to_string(), Some("Gone#1234".to_string())),
            ("cccc0003".to_string(), None),
            ("dddd0004".to_string(), Some("Offline#1234".to_string())),
        ];
        assert_eq!(refresh(&p, &http, BASE, &accounts, false, &now, 0, &emit).unwrap(), 3);
        assert_eq!(*seen.lock().unwrap(), vec!["aaaa0001", "bbbb0002", "dddd0004"]);
        // 404 plus a search that finds nothing: one summary and one search request. Plus the avatar, once.
        assert_eq!(http.calls(), 5);

        let all = get_all_ranks(&p, &accounts).unwrap();
        assert_eq!(all["aaaa0001"].season, Some(22));
        assert_eq!(all["aaaa0001"].roles.support.as_ref().unwrap().tier, Tier::Silver);
        assert_eq!(all["bbbb0002"].status, RankStatus::NotFound);
        assert_eq!(all["bbbb0002"].error, None, "not found is a status, not an error text");
        assert_eq!(all["bbbb0002"].checked_at, Some(10_000));
        assert!(all["dddd0004"].error.is_some());
        assert_eq!(all["dddd0004"].status, RankStatus::Error);
        assert!(!all.contains_key("cccc0003"), "no BattleTag, no override");
        let file = all["aaaa0001"].avatar_file.clone().expect("avatar on disk");
        assert_eq!(std::fs::read(p.avatars_dir().join(&file)).unwrap(), AVATAR_PNG);
        assert_eq!(all["bbbb0002"].avatar_file, None);

        // Within the hour nothing is fetched again; the failed and the not-found one wait 10 minutes.
        let calls = http.calls();
        assert_eq!(refresh(&p, &http, BASE, &accounts, false, &now, 0, &emit).unwrap(), 0);
        clock.store(10_000 + ERROR_RETRY_MS, AO::SeqCst);
        assert_eq!(refresh(&p, &http, BASE, &accounts, false, &now, 0, &emit).unwrap(), 2);
        // Forced: all three again.
        assert_eq!(refresh(&p, &http, BASE, &accounts, true, &now, 0, &emit).unwrap(), 3);
        assert_eq!(http.calls(), calls + 3 + 4, "the avatar on disk is not fetched again");

        // A failure keeps the last good data.
        let offline = FakeHttp::default();
        refresh(&p, &offline, BASE, &accounts[..1], true, &now, 0, &emit).unwrap();
        let i = get_ranks(&p, "aaaa0001", Some("TeKrop#2217")).unwrap().unwrap();
        assert_eq!(i.roles.support.unwrap().tier, Tier::Silver);
        assert!(i.error.is_some());
        assert_eq!(i.avatar_file.as_deref(), Some(file.as_str()), "a failure keeps the avatar");

        forget(&p, "aaaa0001").unwrap();
        assert!(!read_store(&p).unwrap().entries.contains_key("aaaa0001"));
    }

    #[test]
    fn a_changed_battletag_hides_the_old_entry() {
        let mut store = RankStore::default();
        apply_lookup(
            &mut store,
            "c1",
            "Old#1234",
            Ok(Lookup::Found {
                season: Some(22),
                roles: ProfileRoles { tank: Some(td(Tier::Gold, 1)), ..Default::default() },
                private_profile: false,
                avatar_url: None,
            }),
            1,
        );
        let i = rank_info(&store, "c1", Some("New#5678")).unwrap();
        assert_eq!(i.roles.tank, None);
        assert_eq!(i.season, None);
    }
    #[test]
    fn a_lookup_without_avatar_keeps_the_previous_one() {
        let found = |avatar: Option<&str>| {
            Ok(Lookup::Found { season: Some(22), roles: ProfileRoles::default(), private_profile: false, avatar_url: avatar.map(str::to_string) })
        };
        let mut store = RankStore::default();
        apply_lookup(&mut store, "c1", "Kestrel#2187", found(Some(AVATAR_A)), 1);
        assert_eq!(store.entries["c1"].avatar_url.as_deref(), Some(AVATAR_A));
        apply_lookup(&mut store, "c1", "Kestrel#2187", found(None), 2);
        assert_eq!(store.entries["c1"].avatar_url.as_deref(), Some(AVATAR_A), "kept when the refresh has none");
        apply_lookup(&mut store, "c1", "Kestrel#2187", Ok(Lookup::NotFound { recheck_at: None }), 3);
        assert_eq!(store.entries["c1"].avatar_url.as_deref(), Some(AVATAR_A), "kept over a not-found answer");
        apply_lookup(&mut store, "c1", "Kestrel#2187", Err("offline".into()), 4);
        assert_eq!(store.entries["c1"].avatar_url.as_deref(), Some(AVATAR_A), "kept over a failure");
        apply_lookup(&mut store, "c1", "Kestrel#2187", found(Some(AVATAR_B)), 5);
        assert_eq!(store.entries["c1"].avatar_url.as_deref(), Some(AVATAR_B), "a new avatar replaces it");
        // A different BattleTag on the same account starts clean.
        apply_lookup(&mut store, "c1", "Nimbus#1934", found(None), 6);
        assert_eq!(store.entries["c1"].avatar_url, None);
        // Older ranks.json files without the field still load.
        let old: Entry = serde_json::from_str(r#"{"battletag":"Kestrel#2187","attemptedAt":1}"#).unwrap();
        assert_eq!(old.avatar_url, None);
    }

    #[test]
    fn avatar_file_only_while_on_disk_and_downloads_never_break_a_refresh() {
        let (_d, p) = paths();
        let summary = |avatar: &str| format!(r#"{{"avatar":{avatar},"competitive":{{"pc":{{"tank":{{"division":"gold","tier":2}},"season":22}}}}}}"#);
        let with_avatar = summary(&format!("\"{AVATAR_A}\""));
        let without = summary("null");
        let accounts = vec![
            ("avtr0001".to_string(), Some("Kestrel#2187".to_string())),
            ("avtr0002".to_string(), Some("Nimbus#1934".to_string())),
        ];
        let now = || 1_000u64;
        // The avatar host fails (HTML instead of PNG): ranks still land, no file, no avatar shown.
        let http = FakeHttp::default()
            .with(&format!("{BASE}/players/Kestrel-2187/summary"), 200, with_avatar.as_bytes())
            .with(&format!("{BASE}/players/Nimbus-1934/summary"), 200, without.as_bytes())
            .with(AVATAR_A, 200, b"<html>busy</html>");
        assert_eq!(refresh(&p, &http, BASE, &accounts, true, &now, 0, &|_| {}).unwrap(), 2);
        let all = get_all_ranks(&p, &accounts).unwrap();
        assert_eq!(all["avtr0001"].roles.tank.as_ref().unwrap().tier, Tier::Gold);
        assert_eq!(all["avtr0001"].avatar_file, None, "not on disk, so the letter shows");
        assert_eq!(all["avtr0002"].avatar_file, None, "no avatar at all");
        assert!(!p.avatars_dir().join("aaaa000000000000000000000000000000000000000000000000000000000001.png").exists());
        // The next refresh tries again and succeeds.
        let http = FakeHttp::default()
            .with(&format!("{BASE}/players/Kestrel-2187/summary"), 200, with_avatar.as_bytes())
            .with(AVATAR_A, 200, AVATAR_PNG);
        refresh(&p, &http, BASE, &accounts[..1], true, &now, 0, &|_| {}).unwrap();
        let i = get_ranks(&p, "avtr0001", Some("Kestrel#2187")).unwrap().unwrap();
        assert_eq!(i.avatar_file.as_deref(), Some("aaaa000000000000000000000000000000000000000000000000000000000001.png"));
        assert_eq!(serde_json::to_value(&i).unwrap()["avatarFile"], i.avatar_file.clone().unwrap());
    }
}
