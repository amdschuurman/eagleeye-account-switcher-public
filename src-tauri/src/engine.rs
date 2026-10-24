//! The switch engine: the state machine that closes Battle.net, restores an account's token, starts
//! Battle.net again and verifies the sign-in from the log. All platform effects (registry, process
//! control, clock, DPAPI) are behind traits, so the whole flow runs under unit tests on Linux with
//! fakes in a tempdir. One switch at a time (a busy flag). Nothing here logs a token or an email.

use crate::clock::Clock;
use crate::paths::{self, Paths};
use crate::procs::Procs;
use crate::registry::{self, ChangedValue, Registry};
use crate::store::{self, Account, Settings, Status};
use crate::vault::{self, Protector, VaultEntry};
use crate::{config, logwatch};
use serde::Serialize;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

/// How long we watch the log for a sign-in during a switch.
const VERIFY_MS: u64 = 20_000;
/// How long Add waits for the user to log in by hand.
const ADD_TIMEOUT_MS: u64 = 180_000;
/// How long save-current / import's auto-login waits.
const CAPTURE_TIMEOUT_MS: u64 = 30_000;
/// Poll interval while watching the log.
const POLL_MS: u64 = 500;
/// How long we wait for the token to show up in the registry after the sign-in line.
const TOKEN_WAIT_MS: u64 = 5_000;
/// The login window must be seen on this many consecutive polls before we call it (Battle.net can
/// flash a small window for a moment while it auto-logs in).
const LOGIN_STREAK: u32 = 2;
/// Battle.net.exe must be gone on this many consecutive polls before the wait counts as "closed".
const GONE_STREAK: u32 = 2;
/// Extra time a watch allows while Battle.net updates itself mid-wait (it restarts afterwards).
const UPDATE_GRACE_MS: u64 = 120_000;
/// Shown when Battle.net is updating itself.
pub const UPDATING_MESSAGE: &str = "Battle.net is updating, try again in a moment.";
/// Prefix of the placeholder code an imported account carries until its first real sign-in.
const PLACEHOLDER_PREFIX: &str = "tcno-";

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SwitchResult {
    pub ok: bool,
    /// "done" | "needs_login" | "error".
    pub status: String,
    pub ms: u64,
    pub message: String,
}

impl SwitchResult {
    fn done(ms: u64) -> Self {
        SwitchResult { ok: true, status: "done".into(), ms, message: String::new() }
    }
    fn needs_login(ms: u64, message: impl Into<String>) -> Self {
        SwitchResult { ok: false, status: "needs_login".into(), ms, message: message.into() }
    }
    fn error(ms: u64, message: impl Into<String>) -> Self {
        SwitchResult { ok: false, status: "error".into(), ms, message: message.into() }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub imported: u32,
    pub needs_login: u32,
    pub skipped: u32,
}

/// Sink for progress events. Tauri emits them to the window; tests capture them.
pub trait Emitter: Send + Sync {
    fn emit(&self, event: &str, payload: serde_json::Value);
}

/// An emitter that drops everything (used where no UI is attached).
#[cfg(test)]
pub struct NoopEmitter;
#[cfg(test)]
impl Emitter for NoopEmitter {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}
}

pub struct Backend {
    pub paths: Paths,
    pub registry: Box<dyn Registry>,
    pub procs: Box<dyn Procs>,
    pub clock: Box<dyn Clock>,
    pub protector: Box<dyn Protector>,
    /// True while a switch/add/save/import is running: one at a time.
    pub busy: AtomicBool,
    /// Set by `cancel_add` to abort an in-flight Add.
    pub cancel: AtomicBool,
}

/// How a log watch ended.
#[derive(Debug, PartialEq)]
enum WatchEnd {
    /// The sign-in line for this code appeared.
    SignedIn(String),
    /// Battle.net is showing its login window (or the log says so).
    LoginShown,
    /// Battle.net.exe went away (the user closed the login window).
    Closed,
    Cancelled,
    TimedOut,
}

/// What a watch looks for besides the sign-in line.
#[derive(Clone, Copy, Default)]
struct WatchOpts {
    /// End with `LoginShown` as soon as the login window is seen.
    detect_login: bool,
    /// End with `Closed` when Battle.net.exe, once seen, is gone.
    detect_close: bool,
    /// End with `Cancelled` when `cancel_add` was called.
    honour_cancel: bool,
}

/// Releases the busy flag when a run ends, however it ends.
struct BusyGuard<'a>(&'a AtomicBool);
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Backend {
    pub fn new(
        paths: Paths,
        registry: Box<dyn Registry>,
        procs: Box<dyn Procs>,
        clock: Box<dyn Clock>,
        protector: Box<dyn Protector>,
    ) -> Self {
        Backend {
            paths,
            registry,
            procs,
            clock,
            protector,
            busy: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
        }
    }

    /// Take the busy flag; `None` means a run is already in progress.
    fn take_busy(&self) -> Option<BusyGuard<'_>> {
        match self.busy.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => Some(BusyGuard(&self.busy)),
            Err(_) => None,
        }
    }

    fn settings(&self) -> Settings {
        store::load_settings(&self.paths.settings_json()).unwrap_or_default()
    }

    fn accounts(&self) -> Result<Vec<Account>, String> {
        store::load_accounts(&self.paths.accounts_json())
    }

    fn save_accounts(&self, accounts: &[Account]) -> Result<(), String> {
        store::save_accounts(&self.paths.accounts_json(), accounts)
    }

    /// The Battle.net install folder: settings override, else the registry, else the default.
    fn battlenet_dir(&self) -> String {
        self.settings()
            .battlenet_path
            .filter(|p| !p.trim().is_empty())
            .or_else(registry::hklm_battlenet_install)
            .unwrap_or_else(|| paths::default_battlenet_install().to_string())
    }

    // ---- list / current / small mutations -------------------------------------------------

    pub fn list_accounts(&self) -> Result<Vec<Account>, String> {
        let mut a = self.accounts()?;
        a.sort_by_key(|x| x.order);
        Ok(a)
    }

    /// The signed-in account code from the newest log, or `None` if Battle.net is not signed in.
    pub fn current_account(&self) -> Result<Option<String>, String> {
        logwatch::current_code(&self.paths.battlenet_logs_dir())
    }

    pub fn rename_account(&self, code: &str, label: &str) -> Result<(), String> {
        let mut a = self.accounts()?;
        let acc = a.iter_mut().find(|x| x.code == code).ok_or("no such account")?;
        acc.label = label.to_string();
        self.save_accounts(&a)
    }

    pub fn remove_account(&self, code: &str) -> Result<(), String> {
        let mut a = self.accounts()?;
        let before = a.len();
        a.retain(|x| x.code != code);
        // Only a code we saved ourselves ever reaches the vault path; anything else is refused, not deleted.
        if a.len() == before {
            return Err("No such account.".into());
        }
        self.save_accounts(&a)?;
        vault::remove_entry(&self.paths.vault_file(code))
    }

    pub fn reorder_accounts(&self, codes: &[String]) -> Result<(), String> {
        let mut a = self.accounts()?;
        for (i, code) in codes.iter().enumerate() {
            if let Some(acc) = a.iter_mut().find(|x| &x.code == code) {
                acc.order = i as u32;
            }
        }
        self.save_accounts(&a)
    }

    pub fn get_settings(&self) -> Settings {
        self.settings()
    }

    pub fn set_settings(&self, s: &Settings) -> Result<(), String> {
        store::save_settings(&self.paths.settings_json(), s)
    }

    pub fn tcno_available(&self) -> u32 {
        crate::tcno::find_accounts(&self.paths.tcno_battlenet_dir()).len() as u32
    }

    /// Is Battle.net updating itself? Keyed on (1) a running Battle.net setup/update process
    /// (`procs::is_update_process`) and (2) a self-update line at the end of a Battle.net or Agent log
    /// written in the last two minutes (`logwatch::update_hint`). Best effort: neither signal was
    /// observed live yet; game updates inside Agent.exe do not count.
    fn update_in_progress(&self) -> bool {
        if self.procs.battlenet_updating() {
            return true;
        }
        let mut files = logwatch::read_log_files(&self.paths.battlenet_logs_dir()).unwrap_or_default();
        if let Some(pd) = std::env::var_os("ProgramData") {
            let agent = std::path::PathBuf::from(pd).join("Battle.net").join("Agent");
            files.extend(logwatch::read_agent_logs(&agent));
        }
        logwatch::update_hint(&files, self.clock.now_ms())
    }

    /// Why a run cannot start right now (Battle.net is updating), if anything. A running game is not
    /// a refusal any more: the run closes it itself (`close_running_game`).
    fn blocked_reason(&self) -> Option<String> {
        if self.update_in_progress() {
            return Some(UPDATING_MESSAGE.into());
        }
        None
    }

    /// Close a running Blizzard game before Battle.net is touched. `on_closing` gets the game's label
    /// once, before the close starts, so the UI can say what it is doing. Afterwards the game list is
    /// read again: anything still running is an error the user can act on.
    fn close_running_game(&self, on_closing: impl FnOnce(&str)) -> Result<(), String> {
        let Some(game) = self.procs.running_game() else { return Ok(()) };
        on_closing(&game);
        let _ = self.procs.close_games();
        match self.procs.running_game() {
            None => Ok(()),
            Some(left) => Err(format!("Could not close {left}. Close it and try again.")),
        }
    }

    /// Full emails (trimmed, lowercase) of every saved account, read from the vault. An account whose
    /// vault cannot be read (another Windows user, corrupt file) contributes nothing.
    fn saved_emails(&self, accounts: &[Account]) -> Vec<String> {
        accounts
            .iter()
            .filter_map(|a| vault::read_entry(&self.paths.vault_file(&a.code), self.protector.as_ref()).ok())
            .map(|e| e.email.trim().to_lowercase())
            .filter(|e| !e.is_empty())
            .collect()
    }

    // ---- the switch -----------------------------------------------------------------------

    fn progress(&self, emit: &dyn Emitter, code: &str, stage: &str) {
        emit.emit("switch-progress", json!({"code": code, "stage": stage, "ms": self.clock.elapsed_ms()}));
    }

    pub fn switch_account(&self, code: &str, emit: &dyn Emitter) -> SwitchResult {
        let Some(_guard) = self.take_busy() else {
            return SwitchResult::error(0, "A switch is already running.");
        };
        let start = self.clock.elapsed_ms();

        if let Some(msg) = self.blocked_reason() {
            self.progress(emit, code, "error");
            return SwitchResult::error(self.clock.elapsed_ms() - start, msg);
        }

        let accounts = match self.accounts() {
            Ok(a) => a,
            Err(e) => return SwitchResult::error(self.clock.elapsed_ms() - start, e),
        };
        if !accounts.iter().any(|a| a.code == code) {
            return SwitchResult::error(self.clock.elapsed_ms() - start, "No such account.");
        }
        // A fresh run: an earlier cancel must not abort this one.
        self.cancel.store(false, Ordering::SeqCst);

        let bn_dir = self.battlenet_dir();
        let logs = self.paths.battlenet_logs_dir();
        let config_path = self.paths.battlenet_config();
        // Restored if the user closes the login window instead of logging in.
        let previous_names = self.current_saved_names(&config_path);

        // Already signed in as the target? Bring Battle.net forward and finish (a running game is
        // left alone: nothing about the session changes).
        let bn_running = self.procs.battlenet_running(&bn_dir);
        let mut signed_in = None;
        if bn_running {
            if let Ok(Some(cur)) = logwatch::current_code(&logs) {
                if cur == code {
                    if !self.procs.focus_battlenet(&bn_dir) {
                        // No visible window (Battle.net sits in its own tray): starting the launcher
                        // again hands off to the running instance, which shows its window.
                        let _ = self.procs.start_battlenet(&paths::battlenet_launcher_exe(&bn_dir).to_string_lossy());
                    }
                    self.touch_last_used(code);
                    self.progress(emit, code, "done");
                    return SwitchResult::done(self.clock.elapsed_ms() - start);
                }
                signed_in = Some(cur);
            }
        }

        // A game still running (or lingering for a few seconds after the user closed it) is closed
        // here, before Battle.net goes down.
        let closed = self.close_running_game(|game| {
            emit.emit(
                "switch-progress",
                json!({"code": code, "stage": "closing_game", "game": game, "ms": self.clock.elapsed_ms()}),
            );
        });
        if let Err(e) = closed {
            self.progress(emit, code, "error");
            return SwitchResult::error(self.clock.elapsed_ms() - start, e);
        }

        self.progress(emit, code, "closing");
        if bn_running {
            if let Some(cur) = signed_in {
                // Keep the currently signed-in account's token fresh before we drop it.
                self.refresh_token(&cur);
            }
            if let Err(e) = self.procs.close_battlenet(&bn_dir) {
                self.progress(emit, code, "error");
                return SwitchResult::error(self.clock.elapsed_ms() - start, e);
            }
        }

        // What token do we hold for the target?
        let vault_path = self.paths.vault_file(code);
        let entry = vault::read_entry(&vault_path, self.protector.as_ref()).ok();
        let has_token = entry.as_ref().is_some_and(|e| !e.ua_value.is_empty() && !e.ua_name.is_empty());

        // The email to prefill (from the vault; never stored in the clear elsewhere).
        let email = entry.as_ref().map(|e| e.email.clone());

        let launch_ms = self.clock.now_ms();
        self.progress(emit, code, "restoring");

        // Restore the token (ready accounts only).
        if has_token {
            let e = entry.as_ref().unwrap();
            match e.value_bytes() {
                Ok(bytes) => {
                    if let Err(err) = self.registry.write_unified_auth(&e.ua_name, &bytes) {
                        self.progress(emit, code, "error");
                        return SwitchResult::error(self.clock.elapsed_ms() - start, err);
                    }
                }
                Err(err) => return SwitchResult::error(self.clock.elapsed_ms() - start, err),
            }
        }

        // Point Battle.net at this account.
        if let Err(e) = self.write_login_config(&config_path, email.as_deref().unwrap_or("")) {
            self.progress(emit, code, "error");
            return SwitchResult::error(self.clock.elapsed_ms() - start, e);
        }

        // Snapshot AFTER our restore and before launch, so only a token Battle.net writes counts as new.
        let before = self.registry.read_unified_auth().unwrap_or_default();

        self.progress(emit, code, "launching");
        if let Err(e) = self.procs.start_battlenet(&paths::battlenet_launcher_exe(&bn_dir).to_string_lossy()) {
            self.progress(emit, code, "error");
            return SwitchResult::error(self.clock.elapsed_ms() - start, e);
        }

        if has_token {
            self.progress(emit, code, "verifying");
            let opts = WatchOpts { detect_login: true, ..Default::default() };
            if let WatchEnd::SignedIn(_) = self.watch(&logs, &bn_dir, launch_ms, VERIFY_MS, Some(code), opts) {
                // Capture the rotated token and mark ready.
                self.capture_rotated(code, email.as_deref());
                self.touch_last_used(code);
                self.set_status(code, Status::Ready);
                self.progress(emit, code, "done");
                return SwitchResult::done(self.clock.elapsed_ms() - start);
            }
        }

        // Needs login: Battle.net shows its login window with the email pre-filled. Tell the UI, then
        // keep watching while the user logs in once (cancel_add aborts the wait), and capture the token
        // exactly like Add does.
        self.set_status(code, Status::NeedsLogin);
        self.progress(emit, code, "needs_login");
        let opts = WatchOpts { detect_close: true, honour_cancel: true, ..Default::default() };
        let waited = match self.watch(&logs, &bn_dir, launch_ms, ADD_TIMEOUT_MS, None, opts) {
            WatchEnd::SignedIn(found) => Ok(found),
            end @ (WatchEnd::Closed | WatchEnd::Cancelled) => {
                // Put Battle.net back on the account it was set to before this switch.
                if let Some(names) = &previous_names {
                    let _ = self.write_saved_names_only(&config_path, names);
                }
                let msg = if end == WatchEnd::Closed {
                    "Battle.net was closed before you logged in. Nothing changed."
                } else {
                    "Cancelled. Nothing changed."
                };
                return SwitchResult::needs_login(self.clock.elapsed_ms() - start, msg);
            }
            _ => Err("Timed out waiting for sign-in.".to_string()),
        };
        let captured = waited.and_then(|found| self.capture_after(&before, found, &config_path));
        match captured {
            Ok(cap) => {
                let signed_in = cap.code.clone();
                let label = accounts.iter().find(|a| a.code == code).map(|a| a.label.clone());
                if signed_in != code && is_placeholder(code) {
                    // An imported account whose real code we only learn now: move it to the real code.
                    let _ = self.remove_account(code);
                }
                // The target's email only belongs to the capture when the target is what signed in.
                let same = signed_in == code || is_placeholder(code);
                match self.store_captured(cap, if same { email } else { None }) {
                    Ok(_) => {
                        if signed_in != code && is_placeholder(code) {
                            if let Some(l) = label {
                                self.relabel(&signed_in, &l);
                            }
                        }
                        self.progress(emit, code, "done");
                        SwitchResult::done(self.clock.elapsed_ms() - start)
                    }
                    Err(e) => {
                        self.progress(emit, code, "error");
                        SwitchResult::error(self.clock.elapsed_ms() - start, e)
                    }
                }
            }
            Err(_) => SwitchResult::needs_login(
                self.clock.elapsed_ms() - start,
                "Log in once in Battle.net to save this account.",
            ),
        }
    }

    pub fn cancel_add(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Add a new account: the user logs in by hand once, we capture its token.
    pub fn add_account(&self, emit: &dyn Emitter) -> Result<Account, String> {
        let Some(_guard) = self.take_busy() else {
            return Err("A switch is already running.".into());
        };
        self.cancel.store(false, Ordering::SeqCst);
        if let Some(msg) = self.blocked_reason() {
            return Err(msg);
        }
        self.close_running_game(|_| {})?;
        let config_path = self.paths.battlenet_config();
        let previous_names = self.current_saved_names(&config_path);

        emit.emit("add-progress", json!({"stage": "waiting_login"}));
        let captured = self.capture_cycle(Some(""), ADD_TIMEOUT_MS, true);

        match captured {
            Ok(cap) => {
                let account = self.store_captured(cap, None)?;
                emit.emit("add-progress", json!({"stage": "captured"}));
                Ok(account)
            }
            Err(e) => {
                // Restore whatever account was selected before we started.
                if let Some(names) = previous_names {
                    let _ = self.write_saved_names_only(&config_path, &names);
                }
                emit.emit("add-progress", json!({"stage": "error", "message": e}));
                Err(e)
            }
        }
    }

    /// Save the account that is already signed in (no user action): relaunch, capture its rotated token.
    pub fn save_current(&self, emit: &dyn Emitter) -> Result<Account, String> {
        let Some(_guard) = self.take_busy() else {
            return Err("A switch is already running.".into());
        };
        if let Some(msg) = self.blocked_reason() {
            return Err(msg);
        }
        self.close_running_game(|_| {})?;
        let config_path = self.paths.battlenet_config();
        let email = self.current_saved_names(&config_path);
        emit.emit("add-progress", json!({"stage": "waiting_login"}));
        // Keep the current SavedAccountNames so Battle.net auto-logs the current account back in.
        let cap = self.capture_cycle(None, CAPTURE_TIMEOUT_MS, false)?;
        let account = self.store_captured(cap, email)?;
        emit.emit("add-progress", json!({"stage": "captured"}));
        Ok(account)
    }

    /// Import every TcNo Battle.net account (minus `skip` labels), restoring the originally signed-in
    /// account at the end.
    pub fn import_tcno(&self, skip: &[String], emit: &dyn Emitter) -> Result<ImportSummary, String> {
        let Some(_guard) = self.take_busy() else {
            return Err("A switch is already running.".into());
        };
        if let Some(msg) = self.blocked_reason() {
            return Err(msg);
        }
        self.close_running_game(|_| {})?;

        let found = crate::tcno::find_accounts(&self.paths.tcno_battlenet_dir());
        let existing = self.accounts().unwrap_or_default();
        // Duplicates are found by the FULL email held in the vault: two different addresses can share
        // one masked form (`pi***@g***.com`), so comparing masks skipped real accounts.
        let known = self.saved_emails(&existing);
        let config_path = self.paths.battlenet_config();
        let original = self.current_account().ok().flatten();

        let mut summary = ImportSummary { imported: 0, needs_login: 0, skipped: 0 };
        let to_do: Vec<_> = found
            .iter()
            .filter(|a| {
                if skip.iter().any(|s| s == &a.label) {
                    summary.skipped += 1;
                    return false;
                }
                // Already saved (same full email, case-insensitive): skip.
                if known.contains(&a.email.trim().to_lowercase()) {
                    summary.skipped += 1;
                    return false;
                }
                true
            })
            .collect();

        let total = to_do.len() as u32;
        for (i, acc) in to_do.iter().enumerate() {
            emit.emit(
                "import-progress",
                json!({"done": i as u32, "total": total, "label": acc.label, "status": "start"}),
            );
            // Point Battle.net at this email and relaunch; a token appears if "keep me logged in" held.
            let cap = self.capture_cycle_for_email(&acc.email, CAPTURE_TIMEOUT_MS);
            let status = match cap {
                Ok(c) => {
                    if let Ok(a) = self.store_captured(c, Some(acc.email.clone())) {
                        self.relabel(&a.code, &acc.label);
                    }
                    summary.imported += 1;
                    "imported"
                }
                Err(_) => {
                    // Store as needs_login: email known, no token.
                    self.store_needs_login(&acc.email, &acc.label)?;
                    summary.needs_login += 1;
                    "needs_login"
                }
            };
            emit.emit(
                "import-progress",
                json!({"done": (i as u32) + 1, "total": total, "label": acc.label, "status": status}),
            );
        }

        // Switch back to whoever was signed in before.
        if let Some(orig) = original {
            let _ = self.write_login_config(&config_path, "");
            let _ = self.switch_back(&orig);
        }
        Ok(summary)
    }

    // ---- shared capture ------------------------------------------------------------------

    /// Close Battle.net, snapshot UnifiedAuth, optionally set SavedAccountNames, relaunch, and watch
    /// for a sign-in. Returns the account code, its token bytes and (if readable) its email.
    fn capture_cycle(
        &self,
        set_saved_names: Option<&str>,
        timeout_ms: u64,
        honour_cancel: bool,
    ) -> Result<Captured, String> {
        let bn_dir = self.battlenet_dir();
        let logs = self.paths.battlenet_logs_dir();
        let config_path = self.paths.battlenet_config();

        let before = self.registry.read_unified_auth().unwrap_or_default();
        if self.procs.battlenet_running(&bn_dir) {
            self.procs.close_battlenet(&bn_dir)?;
        }
        if let Some(names) = set_saved_names {
            self.write_login_config(&config_path, names)?;
        }
        let launch_ms = self.clock.now_ms();
        self.procs.start_battlenet(&paths::battlenet_launcher_exe(&bn_dir).to_string_lossy())?;

        // An interactive wait (Add) also ends when the user closes Battle.net's login window.
        // A non-interactive cycle (import) gives up as soon as the login window shows; an interactive one
        // (Add) waits for the user to log in there.
        let opts = WatchOpts { detect_login: !honour_cancel, detect_close: honour_cancel, honour_cancel };
        let code = match self.watch(&logs, &bn_dir, launch_ms, timeout_ms, None, opts) {
            WatchEnd::SignedIn(code) => code,
            WatchEnd::Closed => return Err("Battle.net was closed before you logged in. Nothing was added.".into()),
            WatchEnd::Cancelled => return Err("Cancelled.".into()),
            WatchEnd::LoginShown => return Err("Battle.net asked for a login.".into()),
            _ => return Err("Timed out waiting for sign-in.".into()),
        };
        self.capture_after(&before, code, &config_path)
    }

    /// After a sign-in line for `code`: find its token by diffing UnifiedAuth against `before`. Battle.net
    /// can write the token a moment after the log line, so this polls briefly. The value name equals the
    /// account code, which settles the rare case where several values changed.
    fn capture_after(
        &self,
        before: &registry::AuthValues,
        code: String,
        config_path: &std::path::Path,
    ) -> Result<Captured, String> {
        let start = self.clock.elapsed_ms();
        loop {
            let after = self.registry.read_unified_auth().unwrap_or_default();
            let name = match registry::changed_value(before, &after) {
                ChangedValue::One(n) => Some(n),
                ChangedValue::Many(names) if names.contains(&code) => Some(code.clone()),
                ChangedValue::Many(_) => None,
                // No change seen yet. An unchanged existing token under the code still counts.
                ChangedValue::None if after.contains_key(&code) => Some(code.clone()),
                ChangedValue::None => None,
            };
            if let Some(name) = name {
                let bytes = after.get(&name).cloned().ok_or("Could not read the account token.")?;
                let email = self.current_saved_names(config_path);
                return Ok(Captured { code, ua_name: name, bytes, email });
            }
            if self.clock.elapsed_ms().saturating_sub(start) >= TOKEN_WAIT_MS {
                return Err("Could not find the account token.".into());
            }
            self.clock.sleep_ms(POLL_MS);
        }
    }

    /// Import variant: set a specific email, relaunch, expect auto-login.
    fn capture_cycle_for_email(&self, email: &str, timeout_ms: u64) -> Result<Captured, String> {
        self.capture_cycle(Some(email), timeout_ms, false)
    }

    fn store_captured(&self, cap: Captured, email_override: Option<String>) -> Result<Account, String> {
        let email = email_override.or(cap.email).unwrap_or_default();
        let entry = VaultEntry::new(email.clone(), cap.ua_name.clone(), &cap.bytes, self.clock.now_ms());
        vault::write_entry(&self.paths.vault_file(&cap.code), self.protector.as_ref(), &entry)?;

        let mut accounts = self.accounts()?;
        let masked = store::mask_email(&email);
        let now = self.clock.now_ms();
        if let Some(acc) = accounts.iter_mut().find(|a| a.code == cap.code) {
            acc.email_masked = masked;
            acc.status = Status::Ready;
            acc.last_used = Some(now);
        } else {
            let order = store::next_order(&accounts);
            accounts.push(Account {
                code: cap.code.clone(),
                // Never derived from the email (accounts.json holds only the masked form).
                label: format!("Account {}", order + 1),
                battletag: None,
                email_masked: masked,
                order,
                last_used: Some(now),
                status: Status::Ready,
                added_at: now,
            });
        }
        self.save_accounts(&accounts)?;
        accounts.into_iter().find(|a| a.code == cap.code).ok_or("account lost after save".into())
    }

    fn store_needs_login(&self, email: &str, label: &str) -> Result<(), String> {
        // A placeholder code for an account we have not seen sign in yet (label-derived, stable). The
        // first real sign-in moves it to the real code (see switch_account).
        let code = format!("{PLACEHOLDER_PREFIX}{}", sanitize(label));
        let entry = VaultEntry::new(email.to_string(), String::new(), &[], self.clock.now_ms());
        vault::write_entry(&self.paths.vault_file(&code), self.protector.as_ref(), &entry)?;
        let mut accounts = self.accounts()?;
        let now = self.clock.now_ms();
        if !accounts.iter().any(|a| a.code == code) {
            accounts.push(Account {
                code,
                label: label.to_string(),
                battletag: None,
                email_masked: store::mask_email(email),
                order: store::next_order(&accounts),
                last_used: None,
                status: Status::NeedsLogin,
                added_at: now,
            });
            self.save_accounts(&accounts)?;
        }
        Ok(())
    }

    fn switch_back(&self, code: &str) -> SwitchResult {
        // A plain switch, but without the busy guard (we already hold it): reuse the inner steps by
        // temporarily releasing is unsafe, so just do the minimal restore.
        let bn_dir = self.battlenet_dir();
        let logs = self.paths.battlenet_logs_dir();
        let config_path = self.paths.battlenet_config();
        let vault_path = self.paths.vault_file(code);
        let Ok(entry) = vault::read_entry(&vault_path, self.protector.as_ref()) else {
            return SwitchResult::error(0, "no token to switch back to");
        };
        if self.procs.battlenet_running(&bn_dir) {
            let _ = self.procs.close_battlenet(&bn_dir);
        }
        if let Ok(bytes) = entry.value_bytes() {
            if !entry.ua_name.is_empty() {
                let _ = self.registry.write_unified_auth(&entry.ua_name, &bytes);
            }
        }
        let launch_ms = self.clock.now_ms();
        let _ = self.write_login_config(&config_path, &entry.email);
        let _ = self.procs.start_battlenet(&paths::battlenet_launcher_exe(&bn_dir).to_string_lossy());
        match self.watch(&logs, &bn_dir, launch_ms, VERIFY_MS, Some(code), WatchOpts::default()) {
            WatchEnd::SignedIn(_) => SwitchResult::done(0),
            _ => SwitchResult::needs_login(0, "needs login"),
        }
    }

    // ---- small helpers -------------------------------------------------------------------

    /// Read-modify-write Battle.net.config, retried with backoff while the file is locked.
    fn edit_config(
        &self,
        config_path: &std::path::Path,
        edit: impl Fn(&mut serde_json::Value) -> Result<(), String>,
    ) -> Result<(), String> {
        config::with_lock_retry(
            || {
                let mut cfg = config::read_or_default(config_path)?;
                edit(&mut cfg)?;
                config::write(config_path, &cfg)
            },
            &|ms| self.clock.sleep_ms(ms),
        )
    }

    fn write_login_config(&self, config_path: &std::path::Path, email: &str) -> Result<(), String> {
        self.edit_config(config_path, |cfg| {
            config::set_saved_account_names(cfg, email)?;
            config::set_auto_login_true(cfg)
        })
    }

    fn write_saved_names_only(&self, config_path: &std::path::Path, email: &str) -> Result<(), String> {
        self.edit_config(config_path, |cfg| config::set_saved_account_names(cfg, email))
    }

    fn current_saved_names(&self, config_path: &std::path::Path) -> Option<String> {
        let cfg = config::read_or_default(config_path).ok()?;
        cfg.get("Client")?.get("SavedAccountNames")?.as_str().map(|s| s.to_string()).filter(|s| !s.is_empty())
    }

    /// Poll the logs (and Battle.net's windows/processes, per `opts`) until the sign-in line for
    /// `want` (or any code when `None`) appears in a log written after `launch_ms`, or another end
    /// condition hits. While Battle.net updates itself the clock is extended (up to
    /// `UPDATE_GRACE_MS`) and the login/close checks pause, since it restarts after the update.
    fn watch(
        &self,
        logs: &std::path::Path,
        bn_dir: &str,
        launch_ms: u64,
        timeout_ms: u64,
        want: Option<&str>,
        opts: WatchOpts,
    ) -> WatchEnd {
        let start = self.clock.elapsed_ms();
        let mut last = start;
        let mut grace = 0u64;
        let (mut login_streak, mut gone_streak, mut seen_running) = (0u32, 0u32, false);
        loop {
            if opts.honour_cancel && self.cancel.load(Ordering::SeqCst) {
                return WatchEnd::Cancelled;
            }
            if let Ok(files) = logwatch::read_log_files(logs) {
                if let Some(found) = logwatch::signed_in_code(&files, Some(launch_ms)) {
                    if want.map_or(true, |w| w == found) {
                        return WatchEnd::SignedIn(found);
                    }
                }
                if opts.detect_login && logwatch::login_hint(&files, launch_ms) {
                    return WatchEnd::LoginShown;
                }
            }
            let updating = (opts.detect_login || opts.detect_close) && self.procs.battlenet_updating();
            let now = self.clock.elapsed_ms();
            if updating {
                grace = (grace + now.saturating_sub(last)).min(UPDATE_GRACE_MS);
                login_streak = 0;
                gone_streak = 0;
            } else {
                if opts.detect_login {
                    if self.procs.login_window_visible(bn_dir) {
                        login_streak += 1;
                        if login_streak >= LOGIN_STREAK {
                            return WatchEnd::LoginShown;
                        }
                    } else {
                        login_streak = 0;
                    }
                }
                if opts.detect_close {
                    if self.procs.battlenet_running(bn_dir) {
                        seen_running = true;
                        gone_streak = 0;
                    } else if seen_running {
                        gone_streak += 1;
                        if gone_streak >= GONE_STREAK {
                            return WatchEnd::Closed;
                        }
                    }
                }
            }
            last = now;
            if now.saturating_sub(start) >= timeout_ms + grace {
                return WatchEnd::TimedOut;
            }
            self.clock.sleep_ms(POLL_MS);
        }
    }

    /// After a successful sign-in, re-read the rotated token and store it. The token lives under the
    /// UnifiedAuth value name saved in the vault, which is NOT the account code (seen in testing: account
    /// 3c5d7e9f had its token under 7B1C2D3E).
    fn capture_rotated(&self, code: &str, email: Option<&str>) {
        let path = self.paths.vault_file(code);
        let Ok(existing) = vault::read_entry(&path, self.protector.as_ref()) else { return };
        let Ok(values) = self.registry.read_unified_auth() else { return };
        if let Some(bytes) = values.get(&existing.ua_name) {
            let email = email.map(|s| s.to_string()).unwrap_or(existing.email);
            let entry = VaultEntry::new(email, existing.ua_name.clone(), bytes, self.clock.now_ms());
            let _ = vault::write_entry(&path, self.protector.as_ref(), &entry);
        }
    }

    /// Save the account's token again a while after a switch: Blizzard's token security checkpoint can
    /// rotate it seconds after the sign-in line (seen in testing).
    pub fn recapture_token(&self, code: &str) {
        self.capture_rotated(code, None);
    }

    /// Refresh the vault token of an account from the live registry (keeps it current before a switch).
    fn refresh_token(&self, code: &str) {
        self.capture_rotated(code, None);
    }

    fn touch_last_used(&self, code: &str) {
        if let Ok(mut a) = self.accounts() {
            if let Some(acc) = a.iter_mut().find(|x| x.code == code) {
                acc.last_used = Some(self.clock.now_ms());
                let _ = self.save_accounts(&a);
            }
        }
    }

    fn set_status(&self, code: &str, status: Status) {
        if let Ok(mut a) = self.accounts() {
            if let Some(acc) = a.iter_mut().find(|x| x.code == code) {
                acc.status = status;
                let _ = self.save_accounts(&a);
            }
        }
    }

    fn relabel(&self, code: &str, label: &str) {
        let _ = self.rename_account(code, label);
    }
}

struct Captured {
    code: String,
    ua_name: String,
    bytes: Vec<u8>,
    email: Option<String>,
}

/// Is this a placeholder code (imported, real code not known yet)?
fn is_placeholder(code: &str) -> bool {
    code.starts_with(PLACEHOLDER_PREFIX)
}

/// A filesystem-safe token for a placeholder account code.
fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::test_support::FakeClock;
    use crate::procs::test_support::FakeProcs;
    use crate::registry::test_support::FakeRegistry;
    use crate::registry::AuthValues;
    use crate::vault::test_support::XorProtector;
    use std::sync::Arc;

    struct CaptureEmit(std::sync::Mutex<Vec<(String, serde_json::Value)>>);
    impl Emitter for CaptureEmit {
        fn emit(&self, event: &str, payload: serde_json::Value) {
            self.0.lock().unwrap().push((event.to_string(), payload));
        }
    }
    impl CaptureEmit {
        fn new() -> Self {
            CaptureEmit(std::sync::Mutex::new(Vec::new()))
        }
        fn stages(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .filter(|(e, _)| e == "switch-progress")
                .map(|(_, p)| p["stage"].as_str().unwrap_or("").to_string())
                .collect()
        }
    }

    /// A backend over a tempdir, with fake registry/procs/clock. Returns the backend plus the paths.
    fn backend_with(
        reg: Arc<FakeRegistry>,
        procs: Arc<FakeProcs>,
        clock: FakeClock,
        dir: &std::path::Path,
    ) -> Backend {
        let paths = Paths::new(dir.join("app"), dir.join("local"));
        std::fs::create_dir_all(&paths.store_dir).unwrap();
        // Settings with an explicit Battle.net path so no registry lookup is needed.
        let settings = Settings { battlenet_path: Some(r"C:\BN".into()), ..Default::default() };
        store::save_settings(&paths.settings_json(), &settings).unwrap();
        Backend {
            paths,
            registry: Box::new(RegShim(reg)),
            procs: Box::new(ProcShim(procs)),
            clock: Box::new(clock),
            protector: Box::new(XorProtector),
            busy: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
        }
    }

    // Shims to put an Arc<Fake*> behind the Box<dyn> the backend holds, while a test keeps its own Arc.
    struct RegShim(Arc<FakeRegistry>);
    impl Registry for RegShim {
        fn read_unified_auth(&self) -> Result<AuthValues, String> {
            self.0.read_unified_auth()
        }
        fn write_unified_auth(&self, name: &str, value: &[u8]) -> Result<(), String> {
            self.0.write_unified_auth(name, value)
        }
    }
    struct ProcShim(Arc<FakeProcs>);
    impl Procs for ProcShim {
        fn running_game(&self) -> Option<String> {
            self.0.running_game()
        }
        fn close_games(&self) -> Vec<String> {
            self.0.close_games()
        }
        fn battlenet_running(&self, d: &str) -> bool {
            self.0.battlenet_running(d)
        }
        fn close_battlenet(&self, d: &str) -> Result<(), String> {
            self.0.close_battlenet(d)
        }
        fn start_battlenet(&self, e: &str) -> Result<(), String> {
            self.0.start_battlenet(e)
        }
        fn login_window_visible(&self, d: &str) -> bool {
            self.0.login_window_visible(d)
        }
        fn focus_battlenet(&self, d: &str) -> bool {
            self.0.focus_battlenet(d)
        }
        fn battlenet_updating(&self) -> bool {
            self.0.battlenet_updating()
        }
    }

    fn seed_ready_account(be: &Backend, code: &str, email: &str, token: &[u8]) {
        seed_ready_account_ua(be, code, code, email, token);
    }

    fn seed_ready_account_ua(be: &Backend, code: &str, ua_name: &str, email: &str, token: &[u8]) {
        let entry = VaultEntry::new(email.into(), ua_name.into(), token, 1000);
        vault::write_entry(&be.paths.vault_file(code), be.protector.as_ref(), &entry).unwrap();
        let acc = Account {
            code: code.into(),
            label: "Test".into(),
            battletag: None,
            email_masked: store::mask_email(email),
            order: 0,
            last_used: None,
            status: Status::Ready,
            added_at: 1000,
        };
        be.save_accounts(&[acc]).unwrap();
    }

    #[test]
    fn remove_account_refuses_a_code_it_never_saved() {
        let dir = tempfile::tempdir().unwrap();
        let be = backend_with(Arc::new(FakeRegistry::empty()), FakeProcs::new().into_arc(), FakeClock::new(0, 6000), dir.path());
        seed_ready_account(&be, "aaaa0001", "alice@b.com", &[1]);
        assert!(be.remove_account(r"..\..\outside").is_err());
        assert!(be.paths.vault_file("aaaa0001").is_file(), "nothing removed");
        be.remove_account("aaaa0001").unwrap();
        assert!(!be.paths.vault_file("aaaa0001").exists());
    }

    #[test]
    fn switch_happy_path_restores_token_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("local").join("Battle.net").join("Logs");
        std::fs::create_dir_all(&logs).unwrap();
        let reg = Arc::new(FakeRegistry::empty());
        // start_battlenet simulates Battle.net logging in: writes the success line and rotates the token.
        let reg2 = reg.clone();
        let logs2 = logs.clone();
        let procs = FakeProcs::new()
            .with_on_start(move || {
                std::fs::write(
                    logs2.join("battle.net-new.log"),
                    "Querying parental control info for account: 1:0:aaaa0001",
                )
                .unwrap();
                reg2.write_unified_auth("aaaa0001", &[9, 9, 9, 9]).unwrap(); // rotated
            })
            .into_arc();
        let be = backend_with(reg.clone(), procs.clone(), FakeClock::new(0, 6000), dir.path());
        seed_ready_account(&be, "aaaa0001", "alice@b.com", &[1, 2, 3, 4]);

        let emit = CaptureEmit::new();
        let r = be.switch_account("aaaa0001", &emit);
        assert!(r.ok, "{r:?}");
        assert_eq!(r.status, "done");
        // Token was restored before launch and then the rotated one captured after.
        let stored = vault::read_entry(&be.paths.vault_file("aaaa0001"), be.protector.as_ref()).unwrap();
        assert_eq!(stored.value_bytes().unwrap(), vec![9, 9, 9, 9], "rotated token captured");
        // Stages include the pipeline.
        let stages = emit.stages();
        assert!(stages.contains(&"restoring".to_string()));
        assert!(stages.contains(&"launching".to_string()));
        assert!(stages.contains(&"done".to_string()));
        assert_eq!(procs.started.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn rotated_token_is_captured_under_its_own_value_name_not_the_code() {
        // Real shape: an account 3c5d7e9f keeps its token under UnifiedAuth 7B1C2D3E.
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("local").join("Battle.net").join("Logs");
        std::fs::create_dir_all(&logs).unwrap();
        let reg = Arc::new(FakeRegistry::empty());
        let (reg2, logs2) = (reg.clone(), logs.clone());
        let procs = FakeProcs::new()
            .with_on_start(move || {
                std::fs::write(
                    logs2.join("battle.net-new.log"),
                    "Querying parental control info for account: 1:0:3c5d7e9f",
                )
                .unwrap();
                reg2.write_unified_auth("7B1C2D3E", &[7, 7, 7, 7]).unwrap(); // rotated
            })
            .into_arc();
        let be = backend_with(reg.clone(), procs, FakeClock::new(0, 6000), dir.path());
        seed_ready_account_ua(&be, "3c5d7e9f", "7B1C2D3E", "alice@b.com", &[1, 2, 3, 4]);

        let r = be.switch_account("3c5d7e9f", &CaptureEmit::new());
        assert!(r.ok, "{r:?}");
        let stored = vault::read_entry(&be.paths.vault_file("3c5d7e9f"), be.protector.as_ref()).unwrap();
        assert_eq!(stored.ua_name, "7B1C2D3E");
        assert_eq!(stored.value_bytes().unwrap(), vec![7, 7, 7, 7], "rotated token captured");
        assert!(reg.read_unified_auth().unwrap().get("3c5d7e9f").is_none(), "never written under the code");
    }

    #[test]
    fn switch_needs_login_when_no_sign_in_appears() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("local").join("Battle.net").join("Logs")).unwrap();
        let reg = Arc::new(FakeRegistry::empty());
        // start does nothing: the login window stays open, no success line.
        let procs = FakeProcs::new().into_arc();
        let be = backend_with(reg, procs, FakeClock::new(0, 7000), dir.path());
        seed_ready_account(&be, "bbbb0002", "smurf@b.com", &[5, 5]);

        let emit = CaptureEmit::new();
        let r = be.switch_account("bbbb0002", &emit);
        assert!(!r.ok);
        assert_eq!(r.status, "needs_login", "{r:?}");
        assert!(emit.stages().contains(&"needs_login".to_string()));
        // The account was marked needs_login and its old token kept.
        let a = be.list_accounts().unwrap();
        assert_eq!(a[0].status, Status::NeedsLogin);
        let kept = vault::read_entry(&be.paths.vault_file("bbbb0002"), be.protector.as_ref()).unwrap();
        assert_eq!(kept.value_bytes().unwrap(), vec![5, 5]);
    }

    #[test]
    fn needs_login_then_user_logs_in_captures_token_and_marks_ready() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("local").join("Battle.net").join("Logs");
        std::fs::create_dir_all(&logs).unwrap();
        // The user's manual login lands in a fresh log. The verify step only accepts bbbb0002, so it
        // times out; the needs-login wait then accepts the code that really signed in.
        std::fs::write(
            logs.join("battle.net-late.log"),
            "Querying parental control info for account: 1:0:cccc0003",
        )
        .unwrap();
        let reg = Arc::new(FakeRegistry::empty());
        let reg2 = reg.clone();
        // Battle.net writes the logged-in account's new token after launch.
        let procs = FakeProcs::new()
            .with_on_start(move || reg2.write_unified_auth("cccc0003", &[8, 8, 8]).unwrap())
            .into_arc();
        let be = backend_with(reg, procs, FakeClock::new(0, 7000), dir.path());
        seed_ready_account(&be, "bbbb0002", "smurf@b.com", &[5, 5]);

        let emit = CaptureEmit::new();
        let r = be.switch_account("bbbb0002", &emit);
        assert_eq!(r.status, "done", "{r:?}");
        let stages = emit.stages();
        assert!(stages.contains(&"needs_login".to_string()));
        assert_eq!(stages.last(), Some(&"done".to_string()));
        // The token Battle.net wrote was found by the diff and saved.
        let saved = vault::read_entry(&be.paths.vault_file("cccc0003"), be.protector.as_ref()).unwrap();
        assert_eq!(saved.value_bytes().unwrap(), vec![8, 8, 8]);
        assert!(be.list_accounts().unwrap().iter().any(|a| a.code == "cccc0003" && a.status == Status::Ready));
    }

    #[test]
    fn placeholder_import_account_moves_to_its_real_code_on_first_login() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("local").join("Battle.net").join("Logs");
        std::fs::create_dir_all(&logs).unwrap();
        let reg = Arc::new(FakeRegistry::empty());
        let (reg2, logs2) = (reg.clone(), logs.clone());
        let procs = FakeProcs::new()
            .with_on_start(move || {
                std::fs::write(
                    logs2.join("battle.net-1.log"),
                    "Querying parental control info for account: 1:0:abcd1234",
                )
                .unwrap();
                reg2.write_unified_auth("abcd1234", &[3, 1, 4]).unwrap();
            })
            .into_arc();
        let be = backend_with(reg, procs, FakeClock::new(0, 6000), dir.path());
        // An imported needs-login account: email known, no token, placeholder code.
        be.store_needs_login("alice@b.com", "Alice").unwrap();
        let placeholder = be.list_accounts().unwrap()[0].code.clone();
        assert!(is_placeholder(&placeholder));

        let r = be.switch_account(&placeholder, &CaptureEmit::new());
        assert_eq!(r.status, "done", "{r:?}");
        let accounts = be.list_accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].code, "abcd1234");
        assert_eq!(accounts[0].label, "Alice", "label kept across the move");
        assert_eq!(accounts[0].status, Status::Ready);
        assert!(!be.paths.vault_file(&placeholder).exists(), "placeholder vault removed");
        let v = vault::read_entry(&be.paths.vault_file("abcd1234"), be.protector.as_ref()).unwrap();
        assert_eq!(v.email, "alice@b.com");
        assert_eq!(v.value_bytes().unwrap(), vec![3, 1, 4]);
    }

    #[test]
    fn switch_closes_a_running_game_then_proceeds() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Arc::new(FakeRegistry::empty());
        let procs = FakeProcs::new().with_game("Overwatch").into_arc();
        let be = backend_with(reg, procs.clone(), FakeClock::new(0, 6000), dir.path());
        seed_ready_account(&be, "cccc0003", "x@b.com", &[1]);

        let emit = CaptureEmit::new();
        let r = be.switch_account("cccc0003", &emit);
        assert_eq!(procs.games_closed.load(Ordering::SeqCst), 1);
        assert!(procs.running_game().is_none());
        // The switch went on past the game: Battle.net was started.
        assert_eq!(procs.started.load(Ordering::SeqCst), 1, "{}", r.message);
        assert!(!r.message.contains("Close Overwatch"), "{}", r.message);
        let stages = emit.stages();
        assert_eq!(stages.first(), Some(&"closing_game".to_string()), "{stages:?}");
        assert!(stages.contains(&"closing".to_string()));
        assert!(stages.contains(&"launching".to_string()));
        let ev = emit.0.lock().unwrap().iter().find(|(_, p)| p["stage"] == "closing_game").cloned().unwrap();
        assert_eq!(ev.1["game"], "Overwatch");
        assert_eq!(ev.1["code"], "cccc0003");
    }

    #[test]
    fn switch_errors_clearly_when_the_game_will_not_close() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Arc::new(FakeRegistry::empty());
        let fp = FakeProcs::new().with_game("Overwatch");
        fp.game_survives.store(true, Ordering::SeqCst);
        fp.bn_running.store(true, Ordering::SeqCst);
        let procs = fp.into_arc();
        let be = backend_with(reg, procs.clone(), FakeClock::new(0, 6000), dir.path());
        seed_ready_account(&be, "cccc0003", "x@b.com", &[1]);

        let emit = CaptureEmit::new();
        let r = be.switch_account("cccc0003", &emit);
        assert!(!r.ok);
        assert_eq!(r.status, "error");
        assert_eq!(r.message, "Could not close Overwatch. Close it and try again.");
        // Battle.net was left alone: not closed, not restarted.
        assert_eq!(procs.closed.load(Ordering::SeqCst), 0);
        assert_eq!(procs.started.load(Ordering::SeqCst), 0);
        assert_eq!(emit.stages().last(), Some(&"error".to_string()));
    }

    #[test]
    fn switch_to_the_signed_in_account_leaves_the_game_running() {
        let dir = tempfile::tempdir().unwrap();
        let logs = logs_dir(dir.path());
        std::fs::write(logs.join("battle.net-1.log"), "Querying parental control info for account: 1:0:cccc0003").unwrap();
        let reg = Arc::new(FakeRegistry::empty());
        let fp = FakeProcs::new().with_game("Overwatch");
        fp.bn_running.store(true, Ordering::SeqCst);
        let procs = fp.into_arc();
        let be = backend_with(reg, procs.clone(), FakeClock::new(0, 6000), dir.path());
        seed_ready_account(&be, "cccc0003", "x@b.com", &[1]);

        let r = be.switch_account("cccc0003", &CaptureEmit::new());
        assert!(r.ok, "{}", r.message);
        assert_eq!(procs.games_closed.load(Ordering::SeqCst), 0);
        assert!(procs.running_game().is_some());
    }

    #[test]
    fn add_save_and_import_close_the_game_first() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Arc::new(FakeRegistry::empty());
        let fp = FakeProcs::new().with_game("Overwatch");
        fp.game_survives.store(true, Ordering::SeqCst);
        let procs = fp.into_arc();
        let be = backend_with(reg, procs.clone(), FakeClock::new(0, 6000), dir.path());
        let msg = "Could not close Overwatch. Close it and try again.";
        assert_eq!(be.add_account(&NoopEmitter).unwrap_err(), msg);
        assert_eq!(be.save_current(&NoopEmitter).unwrap_err(), msg);
        assert_eq!(be.import_tcno(&[], &NoopEmitter).unwrap_err(), msg);
        assert_eq!(procs.games_closed.load(Ordering::SeqCst), 3);
        assert_eq!(procs.started.load(Ordering::SeqCst), 0, "Battle.net untouched while the game runs");
    }

    #[test]
    fn switch_refuses_a_concurrent_switch() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Arc::new(FakeRegistry::empty());
        let procs = FakeProcs::new().into_arc();
        let be = backend_with(reg, procs, FakeClock::new(0, 6000), dir.path());
        seed_ready_account(&be, "dddd0004", "x@b.com", &[1]);
        // Pretend a switch is already in flight.
        be.busy.store(true, Ordering::SeqCst);
        let r = be.switch_account("dddd0004", &CaptureEmit::new());
        assert!(!r.ok);
        assert!(r.message.to_lowercase().contains("already running"), "{}", r.message);
    }

    #[test]
    fn import_skips_listed_labels_and_counts_them() {
        let dir = tempfile::tempdir().unwrap();
        // A TcNo cache with two accounts.
        let tcno = dir
            .path()
            .join("app")
            .join("TcNo Account Switcher")
            .join("LoginCache")
            .join("BattleNet");
        for (folder, email) in [("Carol", "carol@b.com"), ("Alice", "alice@b.com")] {
            let f = tcno.join(folder);
            std::fs::create_dir_all(&f).unwrap();
            std::fs::write(f.join("email.json"), format!("\"{email}\"")).unwrap();
        }
        std::fs::create_dir_all(dir.path().join("local").join("Battle.net").join("Logs")).unwrap();

        let reg = Arc::new(FakeRegistry::empty());
        // Every relaunch "logs in" as a fixed code so Alice imports as ready; Carol must never be cycled.
        let reg2 = reg.clone();
        let logs = dir.path().join("local").join("Battle.net").join("Logs");
        let procs = FakeProcs::new()
            .with_on_start(move || {
                std::fs::write(
                    logs.join("battle.net-x.log"),
                    "Querying parental control info for account: 1:0:eeee0005",
                )
                .unwrap();
                reg2.write_unified_auth("eeee0005", &[7, 7]).unwrap();
            })
            .into_arc();
        let be = backend_with(reg, procs.clone(), FakeClock::new(0, 40000), dir.path());

        let summary = be.import_tcno(&["Carol".to_string()], &NoopEmitter).unwrap();
        assert_eq!(summary.skipped, 1, "Carol skipped");
        assert_eq!(summary.imported, 1, "Alice imported");
        // Carol's email never entered our store.
        let accounts = be.list_accounts().unwrap();
        assert!(!accounts.iter().any(|a| a.email_masked == store::mask_email("carol@b.com")));
    }

    #[test]
    fn tcno_available_counts_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let tcno = dir.path().join("app").join("TcNo Account Switcher").join("LoginCache").join("BattleNet");
        let f = tcno.join("One");
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(f.join("email.json"), "\"a@b.com\"").unwrap();
        let be = backend_with(
            Arc::new(FakeRegistry::empty()),
            FakeProcs::new().into_arc(),
            FakeClock::new(0, 1000),
            dir.path(),
        );
        assert_eq!(be.tcno_available(), 1);
    }

    fn logs_dir(dir: &std::path::Path) -> std::path::PathBuf {
        let logs = dir.join("local").join("Battle.net").join("Logs");
        std::fs::create_dir_all(&logs).unwrap();
        logs
    }

    fn seed_config(be: &Backend, email: &str) {
        let cfg = format!("{{\n    \"Client\": {{\n        \"SavedAccountNames\": \"{email}\",\n        \"AutoLogin\": true\n    }}\n}}");
        let p = be.paths.battlenet_config();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, cfg).unwrap();
    }

    fn saved_names(be: &Backend) -> Option<String> {
        be.current_saved_names(&be.paths.battlenet_config())
    }

    fn needs_login_event_ms(emit: &CaptureEmit) -> Option<u64> {
        emit.0
            .lock()
            .unwrap()
            .iter()
            .find(|(e, p)| e == "switch-progress" && p["stage"] == "needs_login")
            .and_then(|(_, p)| p["ms"].as_u64())
    }

    #[test]
    fn login_window_reports_needs_login_fast() {
        let dir = tempfile::tempdir().unwrap();
        logs_dir(dir.path());
        let procs = FakeProcs::new().into_arc();
        // Battle.net comes up showing its login window (the token was refused).
        procs.login_window.store(true, Ordering::SeqCst);
        let be = backend_with(Arc::new(FakeRegistry::empty()), procs, FakeClock::new(0, 500), dir.path());
        seed_ready_account(&be, "aaaa0001", "alice@b.com", &[1, 2]);

        let emit = CaptureEmit::new();
        let r = be.switch_account("aaaa0001", &emit);
        assert_eq!(r.status, "needs_login", "{r:?}");
        let ms = needs_login_event_ms(&emit).expect("needs_login event");
        assert!(ms <= 2_000, "needs_login after {ms} ms, not after the 20 s verify window");
    }

    #[test]
    fn a_flashing_login_window_does_not_count() {
        // Battle.net shows a small window for one poll while it auto-logs in: not a login page.
        let dir = tempfile::tempdir().unwrap();
        logs_dir(dir.path());
        let procs = FakeProcs::new().into_arc();
        procs.bn_running.store(true, Ordering::SeqCst);
        procs.login_window_polls.store(1, Ordering::SeqCst);
        let be = backend_with(Arc::new(FakeRegistry::empty()), procs.clone(), FakeClock::new(0, 500), dir.path());
        let opts = WatchOpts { detect_login: true, ..Default::default() };
        let logs = be.paths.battlenet_logs_dir();
        assert_eq!(be.watch(&logs, "C:\\BN", 0, 3_000, Some("aaaa0001"), opts), WatchEnd::TimedOut);
        // Two polls in a row is the login page.
        procs.login_window_polls.store(2, Ordering::SeqCst);
        assert_eq!(be.watch(&logs, "C:\\BN", 0, 3_000, Some("aaaa0001"), opts), WatchEnd::LoginShown);
    }

    #[test]
    fn already_signed_in_brings_battlenet_forward() {
        let dir = tempfile::tempdir().unwrap();
        let logs = logs_dir(dir.path());
        std::fs::write(logs.join("battle.net-1.log"), "Querying parental control info for account: 1:0:aaaa0001").unwrap();
        let procs = FakeProcs::new().into_arc();
        procs.bn_running.store(true, Ordering::SeqCst);
        let be = backend_with(Arc::new(FakeRegistry::empty()), procs.clone(), FakeClock::new(0, 500), dir.path());
        seed_ready_account(&be, "aaaa0001", "alice@b.com", &[1]);

        let emit = CaptureEmit::new();
        let r = be.switch_account("aaaa0001", &emit);
        assert!(r.ok && r.status == "done", "{r:?}");
        assert_eq!(procs.focused.load(Ordering::SeqCst), 1, "window brought forward");
        assert_eq!(procs.closed.load(Ordering::SeqCst), 0, "not closed");
        assert_eq!(procs.started.load(Ordering::SeqCst), 0, "not relaunched");
        assert_eq!(emit.stages().last(), Some(&"done".to_string()));
    }

    #[test]
    fn switch_refuses_while_battlenet_updates() {
        let dir = tempfile::tempdir().unwrap();
        logs_dir(dir.path());
        let procs = FakeProcs::new().into_arc();
        procs.bn_running.store(true, Ordering::SeqCst);
        procs.updating.store(true, Ordering::SeqCst);
        let be = backend_with(Arc::new(FakeRegistry::empty()), procs.clone(), FakeClock::new(0, 500), dir.path());
        seed_ready_account(&be, "aaaa0001", "alice@b.com", &[1]);

        let r = be.switch_account("aaaa0001", &CaptureEmit::new());
        assert_eq!(r.status, "error");
        assert_eq!(r.message, UPDATING_MESSAGE);
        assert_eq!(procs.closed.load(Ordering::SeqCst), 0, "never killed mid-update");
        // Add refuses too.
        assert_eq!(be.add_account(&NoopEmitter).unwrap_err(), UPDATING_MESSAGE);
    }

    #[test]
    fn update_hint_in_a_fresh_log_blocks_a_switch() {
        let dir = tempfile::tempdir().unwrap();
        let logs = logs_dir(dir.path());
        std::fs::write(logs.join("battle.net-u.log"), "INFO Updating Battle.net to 2.41.0").unwrap();
        let procs = FakeProcs::new().into_arc();
        // The fake clock's "now" is the real time, so the log counts as fresh.
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let be = backend_with(Arc::new(FakeRegistry::empty()), procs, FakeClock::new(now, 500), dir.path());
        seed_ready_account(&be, "aaaa0001", "alice@b.com", &[1]);
        assert_eq!(be.switch_account("aaaa0001", &CaptureEmit::new()).message, UPDATING_MESSAGE);
    }

    #[test]
    fn locked_config_retries_then_gives_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        logs_dir(dir.path());
        let procs = FakeProcs::new().into_arc();
        let be = backend_with(Arc::new(FakeRegistry::empty()), procs.clone(), FakeClock::new(0, 500), dir.path());
        seed_ready_account(&be, "aaaa0001", "alice@b.com", &[1]);
        // A directory where the config file should be: every read and write fails, like a lock.
        std::fs::create_dir_all(be.paths.battlenet_config()).unwrap();
        let r = be.switch_account("aaaa0001", &CaptureEmit::new());
        assert_eq!(r.status, "error");
        assert_eq!(r.message, config::LOCKED_MESSAGE);
        assert_eq!(procs.started.load(Ordering::SeqCst), 0, "Battle.net not started on a failed write");
    }

    #[test]
    fn closing_the_login_window_during_switch_restores_previous_account() {
        let dir = tempfile::tempdir().unwrap();
        logs_dir(dir.path());
        let procs = FakeProcs::new().into_arc();
        procs.login_window.store(true, Ordering::SeqCst);
        // After a few checks the user closes Battle.net instead of logging in.
        procs.exit_after_checks.store(3, Ordering::SeqCst);
        let be = backend_with(Arc::new(FakeRegistry::empty()), procs.clone(), FakeClock::new(0, 500), dir.path());
        seed_ready_account(&be, "aaaa0001", "alice@b.com", &[1]);
        seed_config(&be, "before@b.com");

        let r = be.switch_account("aaaa0001", &CaptureEmit::new());
        assert_eq!(r.status, "needs_login", "{r:?}");
        assert!(r.message.contains("closed"), "{}", r.message);
        assert!(r.ms < 10_000, "ended at once, not after the 180 s wait: {} ms", r.ms);
        assert_eq!(saved_names(&be).as_deref(), Some("before@b.com"));
    }

    #[test]
    fn closing_the_login_window_during_add_cancels_and_restores() {
        let dir = tempfile::tempdir().unwrap();
        logs_dir(dir.path());
        let procs = FakeProcs::new().into_arc();
        procs.exit_after_checks.store(2, Ordering::SeqCst);
        let be = backend_with(Arc::new(FakeRegistry::empty()), procs, FakeClock::new(0, 500), dir.path());
        seed_config(&be, "before@b.com");

        let emit = CaptureEmit::new();
        let err = be.add_account(&emit).unwrap_err();
        assert!(err.contains("closed"), "{err}");
        assert_eq!(saved_names(&be).as_deref(), Some("before@b.com"), "previous account restored");
        assert!(be.list_accounts().unwrap().is_empty(), "nothing added");
        let last = emit.0.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.0, "add-progress");
        assert_eq!(last.1["stage"], "error");
    }

    #[test]
    fn battlenet_self_update_mid_wait_is_not_a_close() {
        // Battle.net exits to update itself while the user is on the login window: no Closed.
        let dir = tempfile::tempdir().unwrap();
        logs_dir(dir.path());
        let procs = FakeProcs::new().into_arc();
        let be = backend_with(Arc::new(FakeRegistry::empty()), procs.clone(), FakeClock::new(0, 500), dir.path());
        procs.bn_running.store(true, Ordering::SeqCst);
        procs.updating.store(true, Ordering::SeqCst);
        let opts = WatchOpts { detect_close: true, ..Default::default() };
        procs.bn_running.store(false, Ordering::SeqCst);
        let logs = be.paths.battlenet_logs_dir();
        let end = be.watch(&logs, "C:\\BN", 0, 2_000, None, opts);
        assert_eq!(end, WatchEnd::TimedOut);
        // The update grace extended the wait past the plain 2 s timeout.
        assert!(be.clock.elapsed_ms() > 2_000 + 500);
    }

    #[test]
    fn import_duplicate_check_uses_the_full_email() {
        let dir = tempfile::tempdir().unwrap();
        let tcno = dir.path().join("app").join("TcNo Account Switcher").join("LoginCache").join("BattleNet");
        // Same mask (ni***@b***.com) as the saved account, different address: must be imported.
        // Same address in other case: must be skipped.
        for (folder, email) in [("Alina", "alina@b.com"), ("AliceAgain", "  ALICE.one@b.com")] {
            let f = tcno.join(folder);
            std::fs::create_dir_all(&f).unwrap();
            std::fs::write(f.join("email.json"), format!("\"{}\"", email.trim())).unwrap();
        }
        let logs = logs_dir(dir.path());
        let reg = Arc::new(FakeRegistry::empty());
        let reg2 = reg.clone();
        let procs = FakeProcs::new()
            .with_on_start(move || {
                std::fs::write(logs.join("battle.net-x.log"), "Querying parental control info for account: 1:0:ffff0006")
                    .unwrap();
                reg2.write_unified_auth("ffff0006", &[6]).unwrap();
            })
            .into_arc();
        let be = backend_with(reg, procs, FakeClock::new(0, 40000), dir.path());
        seed_ready_account(&be, "aaaa0001", "alice.one@b.com", &[1]);
        assert_eq!(store::mask_email("alina@b.com"), store::mask_email("alice.one@b.com"));

        let summary = be.import_tcno(&[], &NoopEmitter).unwrap();
        assert_eq!(summary.skipped, 1, "{summary:?}");
        assert_eq!(summary.imported, 1, "{summary:?}");
    }
}
