//! Optional self-update. Some seconds after start, and then about every 6 hours, ask the update endpoint
//! (`plugins.updater.endpoints` in tauri.conf.json, a static `latest.json`) for a newer version and only record the
//! answer. Nothing is downloaded or installed until the user presses "Update now" (`install_update`); there is no
//! automatic or forced install path. The signed `Update` the plugin verified at check time is kept and is the one
//! that gets installed. The install waits while a switch, add or import runs, then the installer closes this app
//! and starts the new one.
//!
//! State goes to the UI as the `update-state` event on every change; `update_state` returns the current one.
//!
//! Updates are switched off in a build whose tauri.conf.json has no update endpoint (the default in this repository).
//! Such a build never checks, never shows the title bar pill, and Settings reads "Updates are not set up in this
//! build". README.md ("Set up your own auto-updates") says how to point it at your own server.
//!
//! Load on the endpoint is bounded by design, so a large install base cannot overwhelm it:
//! - every client spreads its checks with random jitter (first check 5 to 30 s after start, then 6 h plus or minus
//!   15 min), so restarts and release days do not line up into bursts;
//! - a manual "Check for updates" makes at most one request per minute (`retry_at`); a press inside the cooldown
//!   answers from memory with no network call;
//! - a failed background check backs off 15 min, 30, 60, ... up to 6 h, and resets on the next success;
//! - when the server pushes back (any non-success HTTP status, which covers 429 and 503), the client stays away
//!   for 15 min (the plugin does not expose `Retry-After`), clamped to 1 min .. 24 h, manual presses included.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_updater::UpdaterExt;

use crate::engine::Backend;

const SEC: u64 = 1000;
const MIN: u64 = 60 * SEC;
const HOUR: u64 = 60 * MIN;
/// The first background check comes this long after start, plus a random 0 .. `FIRST_SPREAD_MS`.
const FIRST_CHECK_MS: u64 = 5 * SEC;
const FIRST_SPREAD_MS: u64 = 25 * SEC;
/// Between successful background checks, plus or minus a random `JITTER_MS`.
const EVERY_MS: u64 = 6 * HOUR;
const JITTER_MS: u64 = 15 * MIN;
/// After a failed background check: 15 min, doubled per further failure, capped at `EVERY_MS`.
const BACKOFF_BASE_MS: u64 = 15 * MIN;
/// The least time between two completed checks a manual press can trigger.
const MANUAL_COOLDOWN_MS: u64 = MIN;
/// How long to stay away when the server pushed back, and the bounds a server-given wait is clamped to.
const PUSHBACK_DEFAULT_MS: u64 = 15 * MIN;
const PUSHBACK_MIN_MS: u64 = MIN;
const PUSHBACK_MAX_MS: u64 = 24 * HOUR;
pub const EVENT: &str = "update-state";
/// Without a known size, a progress event every this many bytes.
const STEP_UNKNOWN: u64 = 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Idle,
    Checking,
    Available,
    Downloading,
    Installing,
    Error,
    UpToDate,
    /// A manual check inside the cooldown, or the server asked us to wait: `retry_at` says until when.
    RateLimited,
    /// No update endpoint configured in this build: nothing is ever checked.
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub downloaded: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

/// What the UI sees. `version` and `notes` are set while an update is known (also on an error after a failed
/// install, so Retry installs again); without `version` an error came from the check itself. `checked_at` is when
/// the last check completed and `retry_at` the earliest next manual check, both in ms since the Unix epoch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateState {
    pub status: Status,
    pub current: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<u64>,
}

/// Why a check failed: an ordinary error to show, or the server pushing back (stay away for a while).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    Error(String),
    /// The server answered with a non-success status. `retry_after_ms` is its requested wait, when known.
    Pushback { retry_after_ms: Option<u64> },
}

impl From<String> for Failure {
    fn from(e: String) -> Self {
        Failure::Error(e)
    }
}

impl From<&str> for Failure {
    fn from(e: &str) -> Self {
        Failure::Error(e.to_string())
    }
}

/// One found update: its version, its notes and the verified payload `P` (the plugin's `Update`; `()` in tests).
pub struct Found<P> {
    pub version: String,
    pub notes: Option<String>,
    pub update: P,
}

/// The update state machine, free of Tauri and network so it can be tested.
pub struct Tracker<P> {
    state: UpdateState,
    pending: Option<P>,
    last_emit: u64,
    /// Failed checks in a row, for the background backoff. Reset by a successful check.
    failures: u32,
}

impl<P: Clone> Tracker<P> {
    pub fn new(current: &str) -> Self {
        Tracker {
            state: UpdateState {
                status: Status::Idle,
                current: current.to_string(),
                version: None,
                notes: None,
                progress: None,
                error: None,
                checked_at: None,
                retry_at: None,
            },
            pending: None,
            last_emit: 0,
            failures: 0,
        }
    }

    /// A tracker for a build without update endpoints: it stays `Disabled` and never checks or installs.
    pub fn disabled(current: &str) -> Self {
        let mut t = Tracker::new(current);
        t.state.status = Status::Disabled;
        t
    }

    pub fn state(&self) -> UpdateState {
        self.state.clone()
    }

    fn busy(&self) -> bool {
        matches!(self.state.status, Status::Checking | Status::Downloading | Status::Installing)
    }

    /// Failed checks in a row (0 after a success), for `next_delay_ms`.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// How long the background schedule must still wait before it may check: 0 once `retry_at` has passed.
    pub fn wait_ms(&self, now_ms: u64) -> u64 {
        self.state.retry_at.map_or(0, |r| r.saturating_sub(now_ms))
    }

    /// Start a check the user asked for. `false` (and no network call) while one runs, or inside `retry_at`: then
    /// the status reads `Available` if an update is known, else `RateLimited`, and `retry_at` stays as it is.
    pub fn begin_manual_check(&mut self, now_ms: u64) -> bool {
        if self.busy() || self.state.status == Status::Disabled {
            return false;
        }
        if self.wait_ms(now_ms) > 0 {
            self.state.status = if self.pending.is_some() { Status::Available } else { Status::RateLimited };
            self.state.error = None;
            return false;
        }
        self.begin_check()
    }

    /// Start a check. `false` while a check, download or install already runs (the caller then does nothing).
    /// It does not look at `retry_at`: the background schedule waits that out first, the user goes through
    /// `begin_manual_check`.
    pub fn begin_check(&mut self) -> bool {
        if self.busy() || self.state.status == Status::Disabled {
            return false;
        }
        self.state.status = Status::Checking;
        self.state.error = None;
        self.state.progress = None;
        true
    }

    /// Record the answer of a check that completed at `now_ms`, and set when the next one may run.
    pub fn check_done(&mut self, result: Result<Option<Found<P>>, Failure>, now_ms: u64) {
        self.state.progress = None;
        self.state.checked_at = Some(now_ms);
        self.state.retry_at = Some(now_ms + MANUAL_COOLDOWN_MS);
        if result.is_ok() {
            self.failures = 0;
        } else {
            self.failures = self.failures.saturating_add(1);
        }
        match result {
            Ok(Some(f)) => {
                self.state.status = Status::Available;
                self.state.version = Some(f.version);
                self.state.notes = f.notes.filter(|n| !n.trim().is_empty());
                self.state.error = None;
                self.pending = Some(f.update);
            }
            Ok(None) => {
                self.state.status = Status::UpToDate;
                self.state.version = None;
                self.state.notes = None;
                self.state.error = None;
                self.pending = None;
            }
            Err(Failure::Pushback { retry_after_ms }) => {
                let wait = retry_after_ms.unwrap_or(PUSHBACK_DEFAULT_MS).clamp(PUSHBACK_MIN_MS, PUSHBACK_MAX_MS);
                self.state.retry_at = Some(now_ms + wait);
                self.state.error = None;
                self.state.status = if self.pending.is_some() { Status::Available } else { Status::RateLimited };
            }
            Err(Failure::Error(e)) => {
                // A failed check forgets nothing it knew: an update found earlier stays installable.
                if self.pending.is_some() {
                    self.state.status = Status::Available;
                } else {
                    self.state.status = Status::Error;
                    self.state.error = Some(e);
                }
            }
        }
    }

    /// Start installing the update found by the last check. `None` when there is none, or one is already running.
    pub fn begin_install(&mut self) -> Option<P> {
        if self.busy() || self.state.status == Status::Disabled {
            return None;
        }
        let p = self.pending.clone()?;
        self.state.status = Status::Downloading;
        self.state.error = None;
        self.state.progress = Some(Progress { downloaded: 0, total: None });
        self.last_emit = 0;
        Some(p)
    }

    /// Count a downloaded chunk. `true` when the UI should hear about it (about every percent, or every 512 KiB
    /// when the size is unknown), so a fast download does not flood the window with events.
    pub fn progress(&mut self, chunk: u64, total: Option<u64>) -> bool {
        let p = self.state.progress.get_or_insert(Progress { downloaded: 0, total: None });
        p.downloaded += chunk;
        if total.is_some() {
            p.total = total;
        }
        let step = match p.total {
            Some(t) if t > 0 => (t / 100).max(1),
            _ => STEP_UNKNOWN,
        };
        let done = p.total.is_some_and(|t| p.downloaded >= t);
        if done || p.downloaded >= self.last_emit + step {
            self.last_emit = p.downloaded;
            return true;
        }
        false
    }

    /// The download finished; installing next (after any running switch).
    pub fn installing(&mut self) {
        self.state.status = Status::Installing;
    }

    /// A download or install failed: show the reason, keep the update so Retry can install it again.
    pub fn fail(&mut self, error: String) {
        self.state.status = Status::Error;
        self.state.error = Some(error);
        self.state.progress = None;
    }
}

// ---- The schedule: pure functions of a seed and a failure count, so they can be tested ----------------------

/// SplitMix64: one well-mixed 64-bit value from any input. Enough to spread clients apart, not for secrets.
fn mix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A random value in `0..span` (0 when `span` is 0), decided by `seed`.
fn spread(seed: u64, span: u64) -> u64 {
    if span == 0 {
        0
    } else {
        mix(seed) % span
    }
}

/// A fresh seed per call: clock nanoseconds, process id and a counter, so two clients (or two calls) differ.
fn random_seed() -> u64 {
    static CALLS: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let n = CALLS.fetch_add(1, Ordering::Relaxed);
    nanos ^ (u64::from(std::process::id()) << 32) ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The first background check: 5 s after start plus a random 0 .. 25 s.
pub fn first_delay_ms(seed: u64) -> u64 {
    FIRST_CHECK_MS + spread(seed, FIRST_SPREAD_MS + 1)
}

/// The wait before the next background check without jitter: 6 h after a success, else 15 min doubled per
/// further failure in a row, capped at 6 h.
pub fn backoff_ms(failures: u32) -> u64 {
    if failures == 0 {
        return EVERY_MS;
    }
    let doublings = (failures - 1).min(16);
    (BACKOFF_BASE_MS << doublings).min(EVERY_MS)
}

/// `base` moved by a random amount of at most a quarter of itself and at most 15 min, either way.
fn jittered(base: u64, seed: u64) -> u64 {
    let j = (base / 4).min(JITTER_MS);
    base - j + spread(seed, 2 * j + 1)
}

/// The wait before the next background check: `backoff_ms` plus jitter (6 h plus or minus 15 min after a success).
pub fn next_delay_ms(failures: u32, seed: u64) -> u64 {
    jittered(backoff_ms(failures), seed)
}

/// Waiting out a `retry_at`: the remaining wait plus a random extra of up to a quarter of it (at most 15 min), so
/// clients the server turned away together do not all come back in the same second.
pub fn retry_wait_ms(remaining: u64, seed: u64) -> u64 {
    if remaining == 0 {
        0
    } else {
        remaining + spread(seed, (remaining / 4).min(JITTER_MS) + 1)
    }
}

/// Does the updater plugin config (`plugins.updater` in tauri.conf.json) name at least one non-empty endpoint?
pub fn configured(plugin_config: Option<&serde_json::Value>) -> bool {
    plugin_config
        .and_then(|c| c.get("endpoints"))
        .and_then(|e| e.as_array())
        .is_some_and(|list| list.iter().any(|u| u.as_str().is_some_and(|s| !s.trim().is_empty())))
}

/// The app's one tracker, shared by the background check and the commands.
pub struct Updater {
    tracker: Mutex<Tracker<tauri_plugin_updater::Update>>,
}

impl Updater {
    pub fn new(current: &str, enabled: bool) -> Self {
        let t = if enabled { Tracker::new(current) } else { Tracker::disabled(current) };
        Updater { tracker: Mutex::new(t) }
    }

    pub fn enabled(&self) -> bool {
        self.lock().state.status != Status::Disabled
    }

    pub fn state(&self) -> UpdateState {
        self.lock().state()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Tracker<tauri_plugin_updater::Update>> {
        self.tracker.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn publish<R: Runtime>(app: &AppHandle<R>, state: UpdateState) {
    use tauri::Emitter as _;
    let _ = app.emit(EVENT, state);
}

fn updater_of<R: Runtime>(app: &AppHandle<R>) -> Arc<Updater> {
    app.state::<Arc<Updater>>().inner().clone()
}

/// The background schedule (checks only): a first check 5 to 30 s after start, then every 6 h plus or minus 15 min,
/// backing off from 15 min up to 6 h while checks fail. It also waits out any `retry_at` (a manual check's
/// cooldown, or the server pushing back) before it asks again. Nothing at all without endpoints.
pub fn start<R: Runtime>(app: &AppHandle<R>) {
    if !updater_of(app).enabled() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let up = updater_of(&app);
        let mut delay = first_delay_ms(random_seed());
        loop {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            loop {
                let remaining = up.lock().wait_ms(now_ms());
                if remaining == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(retry_wait_ms(remaining, random_seed()))).await;
            }
            run_check(&app, false).await;
            delay = next_delay_ms(up.lock().failures(), random_seed());
        }
    });
}

/// "Check for updates": ask the endpoint now and record the answer, unless a check runs or the cooldown
/// (`retry_at`) has not passed; then no request is made and the current state comes back.
pub async fn check<R: Runtime>(app: &AppHandle<R>) -> UpdateState {
    run_check(app, true).await
}

/// Sort a failed plugin check into an error to show or server pushback. The plugin swallows a non-success HTTP
/// status (429, 503, ...) and answers `ReleaseNotFound`, without the status or `Retry-After`; with our one
/// static endpoint that error means the server answered but not with a release, so stay away for the default time.
fn classify(e: tauri_plugin_updater::Error) -> Failure {
    use tauri_plugin_updater::Error as E;
    match &e {
        E::ReleaseNotFound => Failure::Pushback { retry_after_ms: None },
        E::Reqwest(r) if matches!(r.status().map(|s| s.as_u16()), Some(429 | 503)) => {
            Failure::Pushback { retry_after_ms: None }
        }
        _ => Failure::Error(format!("Could not check for updates: {e}")),
    }
}

async fn run_check<R: Runtime>(app: &AppHandle<R>, manual: bool) -> UpdateState {
    let up = updater_of(app);
    let started = {
        let mut t = up.lock();
        if manual {
            t.begin_manual_check(now_ms())
        } else {
            t.begin_check()
        }
    };
    if !started {
        let state = up.state();
        if manual {
            // The status may have turned to RateLimited (or back to Available): let every view know.
            publish(app, state.clone());
        }
        return state;
    }
    publish(app, up.state());
    let result = match app.updater() {
        Ok(u) => u
            .check()
            .await
            .map(|o| o.map(|update| Found { version: update.version.clone(), notes: update.body.clone(), update }))
            .map_err(classify),
        Err(e) => Err(Failure::Error(format!("Could not check for updates: {e}"))),
    };
    let state = {
        let mut t = up.lock();
        t.check_done(result, now_ms());
        t.state()
    };
    publish(app, state.clone());
    state
}

/// Download the update found by the last check (progress events), wait for a running switch, install and restart.
/// Runs in the background; the UI follows `update-state`.
pub fn spawn_install<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let up = updater_of(app);
    let update = up.lock().begin_install().ok_or("There is no update to install right now.")?;
    publish(app, up.state());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = install(&app, &up, update).await {
            let state = {
                let mut t = up.lock();
                t.fail(e);
                t.state()
            };
            publish(&app, state);
        }
    });
    Ok(())
}

async fn install<R: Runtime>(app: &AppHandle<R>, up: &Updater, update: tauri_plugin_updater::Update) -> Result<(), String> {
    let bytes = update
        .download(
            |chunk, total| {
                let mut t = up.lock();
                if t.progress(chunk as u64, total) {
                    let s = t.state();
                    drop(t);
                    publish(app, s);
                }
            },
            || {},
        )
        .await
        .map_err(|e| format!("The download failed: {e}"))?;
    up.lock().installing();
    publish(app, up.state());
    // Wait for a running switch, add or import to finish rather than cut it off.
    let be = app.state::<Arc<Backend>>().inner().clone();
    while be.busy.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    // `install` checks the signature again before it runs the installer, which closes this app.
    update.install(bytes).map_err(|e| format!("The install failed: {e}"))?;
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed "now" for the tests, in ms since the epoch.
    const T0: u64 = 1_700_000_000_000;

    fn found(v: &str) -> Result<Option<Found<()>>, Failure> {
        Ok(Some(Found { version: v.into(), notes: Some("Fixes".into()), update: () }))
    }

    #[test]
    fn starts_idle_with_the_current_version() {
        let t: Tracker<()> = Tracker::new("0.1.4");
        let s = t.state();
        assert_eq!(s.status, Status::Idle);
        assert_eq!(s.current, "0.1.4");
        assert!(s.version.is_none() && s.progress.is_none() && s.error.is_none());
    }

    #[test]
    fn a_check_only_records_an_update_and_never_starts_a_download() {
        let mut t = Tracker::new("0.1.4");
        assert!(t.begin_check());
        assert_eq!(t.state().status, Status::Checking);
        t.check_done(found("0.1.5"), T0);
        let s = t.state();
        assert_eq!(s.status, Status::Available);
        assert_eq!(s.version.as_deref(), Some("0.1.5"));
        assert_eq!(s.notes.as_deref(), Some("Fixes"));
        assert!(s.progress.is_none());
    }

    #[test]
    fn no_update_reads_up_to_date() {
        let mut t: Tracker<()> = Tracker::new("0.1.4");
        t.begin_check();
        t.check_done(Ok(None), T0);
        assert_eq!(t.state().status, Status::UpToDate);
        assert!(t.begin_install().is_none(), "nothing to install when up to date");
    }

    #[test]
    fn a_second_check_while_one_runs_does_nothing() {
        let mut t: Tracker<()> = Tracker::new("0.1.4");
        assert!(t.begin_check());
        assert!(!t.begin_check());
    }

    #[test]
    fn a_failed_check_without_an_update_is_an_error_without_version() {
        let mut t: Tracker<()> = Tracker::new("0.1.4");
        t.begin_check();
        t.check_done(Err("offline".into()), T0);
        let s = t.state();
        assert_eq!(s.status, Status::Error);
        assert_eq!(s.error.as_deref(), Some("offline"));
        assert!(s.version.is_none());
    }

    #[test]
    fn a_failed_check_keeps_an_update_found_earlier() {
        let mut t = Tracker::new("0.1.4");
        t.begin_check();
        t.check_done(found("0.1.5"), T0);
        t.begin_check();
        t.check_done(Err("offline".into()), T0);
        let s = t.state();
        assert_eq!(s.status, Status::Available);
        assert_eq!(s.version.as_deref(), Some("0.1.5"));
        assert!(t.begin_install().is_some());
    }

    #[test]
    fn install_only_starts_from_a_found_update_and_only_once() {
        let mut t = Tracker::new("0.1.4");
        assert!(t.begin_install().is_none(), "idle has nothing to install");
        t.begin_check();
        t.check_done(found("0.1.5"), T0);
        assert!(t.begin_install().is_some());
        let s = t.state();
        assert_eq!(s.status, Status::Downloading);
        assert_eq!(s.progress, Some(Progress { downloaded: 0, total: None }));
        assert!(t.begin_install().is_none(), "a second press while downloading does nothing");
        assert!(!t.begin_check(), "no check while downloading");
    }

    #[test]
    fn progress_adds_up_and_is_throttled_to_about_one_event_per_percent() {
        let mut t = Tracker::new("0.1.4");
        t.begin_check();
        t.check_done(found("0.1.5"), T0);
        t.begin_install();
        let total = Some(10_000u64);
        let mut events = 0;
        for _ in 0..1000 {
            if t.progress(10, total) {
                events += 1;
            }
        }
        assert_eq!(t.state().progress, Some(Progress { downloaded: 10_000, total: Some(10_000) }));
        assert!((90..=101).contains(&events), "events: {events}");
    }

    #[test]
    fn progress_without_a_size_reports_every_half_mebibyte() {
        let mut t = Tracker::new("0.1.4");
        t.begin_check();
        t.check_done(found("0.1.5"), T0);
        t.begin_install();
        assert!(!t.progress(100 * 1024, None));
        assert!(t.progress(500 * 1024, None));
        assert!(!t.progress(100 * 1024, None));
        assert_eq!(t.state().progress.unwrap().total, None);
    }

    #[test]
    fn a_failed_download_keeps_the_update_for_retry() {
        let mut t = Tracker::new("0.1.4");
        t.begin_check();
        t.check_done(found("0.1.5"), T0);
        t.begin_install();
        t.progress(100, Some(1000));
        t.fail("The download failed: timeout".into());
        let s = t.state();
        assert_eq!(s.status, Status::Error);
        assert_eq!(s.version.as_deref(), Some("0.1.5"), "the version stays so Retry installs again");
        assert!(s.progress.is_none());
        assert!(t.begin_install().is_some(), "retry works");
        assert_eq!(t.state().status, Status::Downloading);
        assert!(t.state().error.is_none());
    }

    #[test]
    fn installing_blocks_checks_and_installs() {
        let mut t = Tracker::new("0.1.4");
        t.begin_check();
        t.check_done(found("0.1.5"), T0);
        t.begin_install();
        t.installing();
        assert_eq!(t.state().status, Status::Installing);
        assert!(!t.begin_check());
        assert!(t.begin_install().is_none());
    }

    #[test]
    fn a_later_check_finding_nothing_clears_the_update() {
        let mut t = Tracker::new("0.1.4");
        t.begin_check();
        t.check_done(found("0.1.5"), T0);
        t.begin_check();
        t.check_done(Ok(None), T0);
        assert!(t.state().version.is_none());
        assert!(t.begin_install().is_none());
    }

    #[test]
    fn blank_notes_are_dropped() {
        let mut t = Tracker::new("0.1.4");
        t.begin_check();
        t.check_done(Ok(Some(Found { version: "0.1.5".into(), notes: Some("  ".into()), update: () })), T0);
        assert!(t.state().notes.is_none());
    }

    #[test]
    fn the_state_serialises_in_the_ui_contract_shape() {
        let mut t = Tracker::new("0.1.4");
        let idle = serde_json::to_value(t.state()).unwrap();
        assert_eq!(idle, serde_json::json!({ "status": "idle", "current": "0.1.4" }));
        t.begin_check();
        t.check_done(Ok(None), T0);
        assert_eq!(serde_json::to_value(t.state()).unwrap()["status"], "upToDate");
        t.begin_check();
        t.check_done(found("0.1.5"), T0);
        t.begin_install();
        t.progress(5, Some(10));
        let v = serde_json::to_value(t.state()).unwrap();
        assert_eq!(v["status"], "downloading");
        assert_eq!(v["version"], "0.1.5");
        assert_eq!(v["progress"], serde_json::json!({ "downloaded": 5, "total": 10 }));
    }

    #[test]
    fn every_completed_check_stamps_checked_at_and_a_one_minute_cooldown() {
        let mut t: Tracker<()> = Tracker::new("0.1.4");
        t.begin_check();
        t.check_done(Ok(None), T0);
        assert_eq!(t.state().checked_at, Some(T0));
        assert_eq!(t.state().retry_at, Some(T0 + MANUAL_COOLDOWN_MS));
        t.begin_check();
        t.check_done(Err("offline".into()), T0 + 5 * MIN);
        assert_eq!(t.state().checked_at, Some(T0 + 5 * MIN), "a failed check counts too");
        assert_eq!(t.state().retry_at, Some(T0 + 6 * MIN));
    }

    #[test]
    fn a_manual_check_inside_the_cooldown_makes_no_request_and_reads_rate_limited() {
        let mut t: Tracker<()> = Tracker::new("0.1.4");
        assert!(t.begin_manual_check(T0), "the first press checks");
        t.check_done(Ok(None), T0);
        assert!(!t.begin_manual_check(T0 + 30 * SEC), "a press 30 s later must not reach the network");
        let s = t.state();
        assert_eq!(s.status, Status::RateLimited);
        assert_eq!(s.retry_at, Some(T0 + MANUAL_COOLDOWN_MS), "the refusal does not move retry_at");
        assert_eq!(s.checked_at, Some(T0));
        assert!(t.begin_manual_check(T0 + MANUAL_COOLDOWN_MS), "once the minute is over it checks again");
        assert_eq!(t.state().status, Status::Checking);
    }

    #[test]
    fn a_manual_check_inside_the_cooldown_keeps_a_found_update_available() {
        let mut t = Tracker::new("0.1.4");
        t.begin_manual_check(T0);
        t.check_done(found("0.1.5"), T0);
        assert!(!t.begin_manual_check(T0 + SEC));
        let s = t.state();
        assert_eq!(s.status, Status::Available);
        assert_eq!(s.version.as_deref(), Some("0.1.5"));
        assert!(t.begin_install().is_some(), "still installable");
    }

    #[test]
    fn a_manual_check_while_one_runs_changes_nothing() {
        let mut t: Tracker<()> = Tracker::new("0.1.4");
        assert!(t.begin_manual_check(T0));
        assert!(!t.begin_manual_check(T0));
        assert_eq!(t.state().status, Status::Checking);
    }

    #[test]
    fn server_pushback_sets_retry_at_with_the_default_and_clamps_a_given_wait() {
        let mut t: Tracker<()> = Tracker::new("0.1.4");
        t.begin_check();
        t.check_done(Err(Failure::Pushback { retry_after_ms: None }), T0);
        let s = t.state();
        assert_eq!(s.status, Status::RateLimited);
        assert!(s.error.is_none());
        assert_eq!(s.retry_at, Some(T0 + PUSHBACK_DEFAULT_MS));
        assert!(!t.begin_manual_check(T0 + 10 * MIN), "manual presses respect the pushback too");
        assert_eq!(t.wait_ms(T0 + 10 * MIN), 5 * MIN, "the background waits it out");

        t.begin_check();
        t.check_done(Err(Failure::Pushback { retry_after_ms: Some(5 * SEC) }), T0);
        assert_eq!(t.state().retry_at, Some(T0 + PUSHBACK_MIN_MS), "too short is raised to a minute");
        t.begin_check();
        t.check_done(Err(Failure::Pushback { retry_after_ms: Some(7 * 24 * HOUR) }), T0);
        assert_eq!(t.state().retry_at, Some(T0 + PUSHBACK_MAX_MS), "too long is cut to a day");
        t.begin_check();
        t.check_done(Err(Failure::Pushback { retry_after_ms: Some(2 * HOUR) }), T0);
        assert_eq!(t.state().retry_at, Some(T0 + 2 * HOUR), "in range is kept");
    }

    #[test]
    fn server_pushback_keeps_a_found_update_available() {
        let mut t = Tracker::new("0.1.4");
        t.begin_check();
        t.check_done(found("0.1.5"), T0);
        t.begin_check();
        t.check_done(Err(Failure::Pushback { retry_after_ms: None }), T0 + HOUR);
        let s = t.state();
        assert_eq!(s.status, Status::Available);
        assert_eq!(s.version.as_deref(), Some("0.1.5"));
        assert_eq!(s.retry_at, Some(T0 + HOUR + PUSHBACK_DEFAULT_MS));
    }

    #[test]
    fn backoff_doubles_from_fifteen_minutes_and_caps_at_six_hours() {
        assert_eq!(backoff_ms(0), 6 * HOUR);
        assert_eq!(backoff_ms(1), 15 * MIN);
        assert_eq!(backoff_ms(2), 30 * MIN);
        assert_eq!(backoff_ms(3), HOUR);
        assert_eq!(backoff_ms(4), 2 * HOUR);
        assert_eq!(backoff_ms(5), 4 * HOUR);
        assert_eq!(backoff_ms(6), 6 * HOUR);
        assert_eq!(backoff_ms(40), 6 * HOUR);
        assert_eq!(backoff_ms(u32::MAX), 6 * HOUR, "no overflow");
    }

    #[test]
    fn failures_count_up_and_a_success_resets_the_backoff() {
        let mut t: Tracker<()> = Tracker::new("0.1.4");
        for n in 1..=3 {
            t.begin_check();
            t.check_done(Err("offline".into()), T0);
            assert_eq!(t.failures(), n);
        }
        t.begin_check();
        t.check_done(Err(Failure::Pushback { retry_after_ms: None }), T0);
        assert_eq!(t.failures(), 4, "pushback counts as a failure");
        assert_eq!(backoff_ms(t.failures()), 2 * HOUR);
        t.begin_check();
        t.check_done(Ok(None), T0);
        assert_eq!(t.failures(), 0);
        assert_eq!(backoff_ms(t.failures()), 6 * HOUR);
    }

    #[test]
    fn jitter_stays_in_range_and_actually_spreads() {
        let mut firsts = std::collections::BTreeSet::new();
        for seed in 0..5000u64 {
            let f = first_delay_ms(seed);
            assert!((5 * SEC..=30 * SEC).contains(&f), "first {f}");
            firsts.insert(f / SEC);
            let ok = next_delay_ms(0, seed);
            assert!((6 * HOUR - 15 * MIN..=6 * HOUR + 15 * MIN).contains(&ok), "after success {ok}");
            let b1 = next_delay_ms(1, seed);
            assert!((15 * MIN - 15 * MIN / 4..=15 * MIN + 15 * MIN / 4).contains(&b1), "first backoff {b1}");
            let cap = next_delay_ms(30, seed);
            assert!((6 * HOUR - 15 * MIN..=6 * HOUR + 15 * MIN).contains(&cap), "capped backoff {cap}");
            let w = retry_wait_ms(10 * MIN, seed);
            assert!((10 * MIN..=10 * MIN + 10 * MIN / 4).contains(&w), "retry wait {w}");
        }
        assert!(firsts.len() > 20, "first checks spread over the window, got {} distinct seconds", firsts.len());
        assert_eq!(retry_wait_ms(0, 7), 0);
        assert_ne!(random_seed(), random_seed(), "two calls give two seeds");
    }

    #[test]
    fn a_disabled_tracker_never_checks_or_installs() {
        let mut t: Tracker<()> = Tracker::disabled("1.0.0");
        assert_eq!(t.state().status, Status::Disabled);
        assert!(!t.begin_check());
        assert!(!t.begin_manual_check(T0), "a manual press makes no request either");
        assert_eq!(t.state().status, Status::Disabled, "and does not turn into rateLimited");
        assert!(t.begin_install().is_none());
        assert_eq!(serde_json::to_value(t.state()).unwrap(), serde_json::json!({ "status": "disabled", "current": "1.0.0" }));
    }

    #[test]
    fn configured_needs_a_non_empty_endpoint() {
        assert!(!configured(None));
        assert!(!configured(Some(&serde_json::json!({ "pubkey": "", "endpoints": [] }))));
        assert!(!configured(Some(&serde_json::json!({ "endpoints": ["  "] }))));
        assert!(!configured(Some(&serde_json::json!({ "pubkey": "x" }))));
        assert!(configured(Some(&serde_json::json!({ "endpoints": ["https://updates.example.invalid/latest.json"] }))));
    }

    #[test]
    fn the_shipped_config_has_updates_switched_off() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert!(!configured(conf.pointer("/plugins/updater")));
        assert_eq!(conf.pointer("/bundle/createUpdaterArtifacts"), Some(&serde_json::json!(false)));
    }

    #[test]
    fn a_rate_limited_state_serialises_with_camel_case_times() {
        let mut t: Tracker<()> = Tracker::new("0.1.4");
        t.begin_check();
        t.check_done(Err(Failure::Pushback { retry_after_ms: None }), T0);
        let v = serde_json::to_value(t.state()).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "status": "rateLimited",
                "current": "0.1.4",
                "checkedAt": T0,
                "retryAt": T0 + PUSHBACK_DEFAULT_MS
            })
        );
    }
}
