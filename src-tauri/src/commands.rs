//! The Tauri command surface the UI depends on. Long-running commands (switch/add/save/import) run
//! on a blocking thread and emit progress events to the window; the rest are quick store reads and
//! writes. Commands return plain errors that never carry a token or an email.

use crate::engine::{Backend, Emitter, ImportSummary, SwitchResult};
use crate::ranks::{RankInfo, Role, RoleRank};
use crate::store::{AccountView, Category, Settings};
use std::collections::BTreeMap;
use serde_json::Value;
use std::sync::Arc;
use tauri::{AppHandle, State};

/// Emits progress events to the window.
struct AppEmitter {
    app: AppHandle,
}

impl Emitter for AppEmitter {
    fn emit(&self, event: &str, payload: Value) {
        use tauri::Emitter as _;
        let _ = self.app.emit(event, payload);
    }
}

#[tauri::command]
pub fn list_accounts(state: State<'_, Arc<Backend>>) -> Result<Vec<AccountView>, String> {
    // Accounts saved before Phase 2 (or before their first sign-in on this PC) get their BattleTag
    // here. Skipped while a switch/add/save writes accounts.json.
    if !state.busy.load(std::sync::atomic::Ordering::SeqCst) {
        let _ = crate::battletag::fill_missing(&state.paths);
    }
    with_category(&state, state.list_accounts()?)
}

/// Attach the category (meta.json) to engine accounts.
fn with_category(be: &Backend, accounts: Vec<crate::store::Account>) -> Result<Vec<AccountView>, String> {
    let meta = crate::store::load_meta(&be.paths.meta_json())?;
    Ok(crate::store::with_categories(accounts, &meta))
}

/// One account as the UI sees it, after a sign-in captured it: BattleTag filled, ranks refreshing.
fn signed_in_view(app: &AppHandle, be: &Backend, acc: crate::store::Account) -> Result<AccountView, String> {
    crate::battletag::after_sign_in(app, &acc.code);
    let code = acc.code.clone();
    let fresh = be.list_accounts()?.into_iter().find(|a| a.code == code).unwrap_or(acc);
    Ok(with_category(be, vec![fresh])?.remove(0))
}

#[tauri::command]
pub fn current_account(state: State<'_, Arc<Backend>>) -> Result<Option<String>, String> {
    state.current_account()
}

#[tauri::command]
pub async fn switch_account(
    code: String,
    app: AppHandle,
    state: State<'_, Arc<Backend>>,
) -> Result<SwitchResult, String> {
    let be = state.inner().clone();
    let handle = app.clone();
    let code_for_ranks = code.clone();
    let presence = crate::presence::busy(&handle, crate::presence::Doing::Switching);
    let result = tauri::async_runtime::spawn_blocking(move || {
        let emit = AppEmitter { app };
        be.switch_account(&code, &emit)
    })
    .await
    .map_err(|_| "switch task failed".to_string())?;
    drop(presence);
    // closeAfterSwitch: hide to the tray or exit after a successful switch.
    crate::tray::after_switch(&handle, &result);
    // Ranks: store the BattleTag and refresh the account just signed in.
    if result.status == "done" {
        crate::battletag::after_sign_in(&handle, &code_for_ranks);
        recapture_later(state.inner().clone(), code_for_ranks);
    }
    Ok(result)
}

#[tauri::command]
pub async fn add_account(app: AppHandle, state: State<'_, Arc<Backend>>) -> Result<AccountView, String> {
    let be = state.inner().clone();
    let handle = app.clone();
    let _presence = crate::presence::busy(&handle, crate::presence::Doing::Adding);
    let acc = tauri::async_runtime::spawn_blocking(move || {
        let emit = AppEmitter { app };
        be.add_account(&emit)
    })
    .await
    .map_err(|_| "add task failed".to_string())??;
    signed_in_view(&handle, &state, acc)
}

#[tauri::command]
pub fn cancel_add(state: State<'_, Arc<Backend>>) {
    state.cancel_add();
}

#[tauri::command]
pub async fn save_current(app: AppHandle, state: State<'_, Arc<Backend>>) -> Result<AccountView, String> {
    let be = state.inner().clone();
    let handle = app.clone();
    let _presence = crate::presence::busy(&handle, crate::presence::Doing::Adding);
    let acc = tauri::async_runtime::spawn_blocking(move || {
        let emit = AppEmitter { app };
        be.save_current(&emit)
    })
    .await
    .map_err(|_| "save task failed".to_string())??;
    signed_in_view(&handle, &state, acc)
}

#[tauri::command]
pub async fn import_tcno(
    skip: Option<Vec<String>>,
    app: AppHandle,
    state: State<'_, Arc<Backend>>,
) -> Result<ImportSummary, String> {
    let be = state.inner().clone();
    let skip = skip.unwrap_or_default();
    let handle = app.clone();
    let summary = tauri::async_runtime::spawn_blocking(move || {
        let emit = AppEmitter { app };
        be.import_tcno(&skip, &emit)
    })
    .await
    .map_err(|_| "import task failed".to_string())?;
    // The saved account count in the Discord activity.
    crate::presence::refresh(&handle);
    summary
}

#[tauri::command]
pub fn tcno_available(state: State<'_, Arc<Backend>>) -> u32 {
    state.tcno_available()
}

#[tauri::command]
pub fn rename_account(code: String, label: String, state: State<'_, Arc<Backend>>) -> Result<(), String> {
    state.rename_account(&code, &label)
}

#[tauri::command]
pub fn remove_account(code: String, app: AppHandle, state: State<'_, Arc<Backend>>) -> Result<(), String> {
    state.remove_account(&code)?;
    crate::presence::refresh(&app);
    // Phase 2 data of the account goes with it.
    let _ = crate::ranks::forget(&state.paths, &code);
    let path = state.paths.meta_json();
    if let Ok(mut meta) = crate::store::load_meta(&path) {
        if meta.categories.remove(&code).is_some() {
            let _ = crate::store::save_meta(&path, &meta);
        }
    }
    Ok(())
}

#[tauri::command]
pub fn reorder_accounts(codes: Vec<String>, state: State<'_, Arc<Backend>>) -> Result<(), String> {
    state.reorder_accounts(&codes)
}

#[tauri::command]
pub fn get_settings(state: State<'_, Arc<Backend>>) -> Settings {
    state.get_settings()
}

#[tauri::command]
pub fn set_settings(settings: Settings, app: AppHandle, state: State<'_, Arc<Backend>>) -> Result<(), String> {
    state.set_settings(&settings)?;
    // "Discord Rich Presence" applies at once.
    crate::presence::refresh(&app);
    Ok(())
}

// ---- Phase 2: Overwatch ranks and categories (DESIGN.md section 3) ------------------------------

/// The BattleTag the store holds for an account.
fn battletag_of(be: &Backend, code: &str) -> Result<Option<String>, String> {
    Ok(be.list_accounts()?.into_iter().find(|a| a.code == code).and_then(|a| a.battletag))
}

/// Cached ranks of one account (no network). `null` when there is no BattleTag and no manual value.
#[tauri::command]
pub fn get_ranks(code: String, state: State<'_, Arc<Backend>>) -> Result<Option<RankInfo>, String> {
    let tag = battletag_of(&state, &code)?;
    crate::ranks::get_ranks(&state.paths, &code, tag.as_deref())
}

/// Cached ranks of every account, keyed by code (no network).
#[tauri::command]
pub fn get_all_ranks(state: State<'_, Arc<Backend>>) -> Result<BTreeMap<String, RankInfo>, String> {
    let targets = crate::ranks::targets(&state.paths, None)?;
    crate::ranks::get_all_ranks(&state.paths, &targets)
}

/// Refresh one account (`code`) or all (`null`) in the background; `ranks-updated {code}` follows
/// per account. Respects the 1 h cache unless `force` is true.
#[tauri::command]
pub fn refresh_ranks(code: Option<String>, force: Option<bool>, app: AppHandle, state: State<'_, Arc<Backend>>) {
    crate::ranks::spawn_refresh(app, state.paths.clone(), code, force.unwrap_or(false));
}

/// Set the manual rank of one role, or clear it with `null`.
#[tauri::command]
pub fn set_rank_override(
    code: String,
    role: String,
    rank: Option<RoleRank>,
    app: AppHandle,
    state: State<'_, Arc<Backend>>,
) -> Result<(), String> {
    let role = Role::parse(&role).ok_or("role must be tank, damage, support or open")?;
    crate::ranks::set_override(&state.paths, &code, role, rank)?;
    use tauri::Emitter as _;
    let _ = app.emit("ranks-updated", serde_json::json!({ "code": code }));
    Ok(())
}

#[tauri::command]
pub fn set_category(code: String, category: Category, state: State<'_, Arc<Backend>>) -> Result<(), String> {
    let path = state.paths.meta_json();
    let mut meta = crate::store::load_meta(&path)?;
    meta.categories.insert(code, category);
    crate::store::save_meta(&path, &meta)
}

/// The local icon folder (`tier_<tier>.png`, `div_<n>.png`, `role_<role>.svg`). The first call
/// fetches whatever is missing (once; present files are never fetched again), then returns.
#[tauri::command]
pub async fn icon_dir(state: State<'_, Arc<Backend>>) -> Result<String, String> {
    let dir = state.paths.icons_dir();
    let specs = crate::icons::specs()?;
    if !crate::icons::complete(&dir, &specs) {
        let d = dir.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            if let Ok(http) = crate::overfast::ReqwestHttp::new() {
                crate::icons::ensure(&d, &http, &specs);
            }
        })
        .await;
    }
    Ok(crate::icons::dir_string(&dir))
}

/// Account codes sorted by rank, highest first, unranked last, ties in manual order. `role` is
/// tank, damage, support or open; `null` sorts by each account's best role.
#[tauri::command]
pub fn sort_by_rank(role: Option<String>, state: State<'_, Arc<Backend>>) -> Result<Vec<String>, String> {
    let role = match role.as_deref() {
        None => None,
        Some(r) => Some(Role::parse(r).ok_or("role must be tank, damage, support or open")?),
    };
    let targets = crate::ranks::targets(&state.paths, None)?;
    let infos = crate::ranks::get_all_ranks(&state.paths, &targets)?;
    let codes: Vec<String> = state.list_accounts()?.into_iter().map(|a| a.code).collect();
    Ok(crate::ranks::sort_codes(&codes, &infos, role))
}

/// Blizzard's token security checkpoint can rotate the token seconds after the sign-in line: save it
/// again 5 s and 30 s after a switch (seen in testing).
pub fn recapture_later(be: Arc<Backend>, code: String) {
    std::thread::spawn(move || {
        for wait in [5u64, 25] {
            std::thread::sleep(std::time::Duration::from_secs(wait));
            be.recapture_token(&code);
        }
    });
}

// ---- Optional updates (updater.rs): offered, never forced ---------------------------------------------------------

/// The update state as it is now (the UI asks once on load, then follows `update-state`).
#[tauri::command]
pub fn update_state(updater: State<'_, Arc<crate::updater::Updater>>) -> crate::updater::UpdateState {
    updater.state()
}

/// Check for an update now ("Check for updates"). Only records the answer; installs nothing.
#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<crate::updater::UpdateState, String> {
    Ok(crate::updater::check(&app).await)
}

/// "Update now": download the update the last check found, wait for a running switch, install and restart.
/// Returns at once; progress arrives as `update-state` events.
#[tauri::command]
pub fn install_update(app: AppHandle) -> Result<(), String> {
    crate::updater::spawn_install(&app)
}
