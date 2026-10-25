//! Discord Rich Presence: while the main window is shown, Discord lists the Account Switcher like a game
//! ("Managing Battle.net accounts", "Switching accounts") with our artwork. Other people see this, so the
//! activity NEVER carries an account label, BattleTag, email or account code: only fixed text and the number
//! of saved accounts (DESIGN.md "Discord presence").
//!
//! One background thread owns the connection; the app only sends it messages over a channel and never waits
//! on it, so a switch or the UI can never block on Discord. The thread never starts Discord: when it is not
//! running it retries quietly (15 s, doubling to 2 min); when the pipe breaks it reconnects and sends the
//! last activity again. Identical activities are not sent twice and at most one SET_ACTIVITY goes out per 2 s
//! (Discord allows about 5 per 20 s).
//!
//! The IPC is Discord's local protocol over the named pipe `\\.\pipe\discord-ipc-0` to `-9`: every frame is a
//! little-endian u32 opcode, a little-endian u32 length, then that many bytes of JSON. Op 0 is the handshake
//! `{v: 1, client_id}` (answered with a READY frame), op 1 a command such as SET_ACTIVITY. Written here
//! rather than taken from a crate so the thread can notice a closed Discord while idle (PeekNamedPipe) and
//! drain Discord's replies without ever blocking on a read.

use serde_json::{json, Value};
use std::io;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, Runtime};

/// The Discord application id (Developer Portal, "Application ID") whose Rich Presence assets carry the
/// keys `eagleeye`, `battlenet` and `overwatch`. Empty = the feature is fully inert: no thread is started
/// and the Discord pipe is never touched.
pub const APP_ID: &str = "1563626078138798080";

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
/// Replies larger than this are not a Discord frame we understand.
const MAX_FRAME: u32 = 64 * 1024;
/// How long `shutdown` waits for the thread to clear the activity.
const SHUTDOWN_WAIT: Duration = Duration::from_millis(500);

/// What the user is doing in the app right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Doing {
    Idle,
    Switching,
    Adding,
}

/// The activity Discord shows for `doing`. `n_accounts` is the number of saved accounts, `started_at` the
/// Unix time in milliseconds the window was shown. Pure, unit-tested; carries no account data by design.
pub fn activity_for(doing: Doing, n_accounts: usize, started_at: u64) -> Value {
    let (details, state, small_image, small_text) = match doing {
        Doing::Idle => {
            let saved = match n_accounts {
                0 => "no accounts saved yet".to_string(),
                1 => "1 account saved".to_string(),
                n => format!("{n} accounts saved"),
            };
            ("Managing Battle.net accounts", format!("Overwatch, {saved}"), "battlenet", "Battle.net")
        }
        Doing::Switching => ("Switching accounts", "Signing in to Battle.net".to_string(), "overwatch", "Overwatch"),
        Doing::Adding => ("Adding an account", "Waiting for the Battle.net login".to_string(), "battlenet", "Battle.net"),
    };
    json!({
        "details": details,
        "state": state,
        "timestamps": { "start": started_at },
        "assets": {
            "large_image": "eagleeye",
            "large_text": "EagleEye Account Switcher",
            "small_image": small_image,
            "small_text": small_text,
        },
    })
}

/// What the connection thread does with the activity it wants Discord to show.
#[derive(Debug, PartialEq, Eq)]
enum Gate {
    /// Discord already shows exactly this (or nothing, and nothing is wanted).
    Skip,
    Send,
    /// Too soon after the last SET_ACTIVITY: send after this long.
    Wait(Duration),
}

/// Dedupe and rate limit. `desired`/`shown` are `None` for "no activity"; `since_last` is the time since the
/// last SET_ACTIVITY (`None` = never sent). Pure, unit-tested.
fn gate(desired: Option<&Value>, shown: Option<&Value>, since_last: Option<Duration>, min_gap: Duration) -> Gate {
    if desired == shown {
        return Gate::Skip;
    }
    match since_last {
        Some(elapsed) if elapsed < min_gap => Gate::Wait(min_gap - elapsed),
        _ => Gate::Send,
    }
}

/// Wait before the next connect attempt after `failures` failed attempts in a row: 15 s, doubling, capped at
/// 2 min. Pure, unit-tested.
fn backoff(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(4);
    Duration::from_secs((15u64 << doublings).min(120))
}

/// One IPC frame: u32 opcode and u32 length (both little-endian), then the JSON.
fn encode_frame(op: u32, payload: &Value) -> Vec<u8> {
    let body = payload.to_string();
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body.as_bytes());
    out
}

/// The opcode and body length of a frame header.
fn decode_header(h: &[u8; 8]) -> (u32, u32) {
    (u32::from_le_bytes([h[0], h[1], h[2], h[3]]), u32::from_le_bytes([h[4], h[5], h[6], h[7]]))
}

/// The SET_ACTIVITY command; `activity: None` clears ours.
fn set_activity_cmd(activity: Option<&Value>, pid: u32, nonce: u64) -> Value {
    json!({
        "cmd": "SET_ACTIVITY",
        "args": { "pid": pid, "activity": activity },
        "nonce": nonce.to_string(),
    })
}

// ---- Transport ------------------------------------------------------------------------------------------

/// A connected Discord pipe. `available` never blocks; it fails once Discord has closed the pipe.
trait Pipe: Send {
    fn write_all(&mut self, data: &[u8]) -> io::Result<()>;
    fn available(&mut self) -> io::Result<usize>;
    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()>;
}

type Connect = Box<dyn FnMut() -> io::Result<Box<dyn Pipe>> + Send>;

/// Wait (polling, never a blocking read) until `n` bytes can be read, or fail at `deadline`.
fn wait_for(pipe: &mut dyn Pipe, n: usize, deadline: Instant) -> io::Result<()> {
    loop {
        if pipe.available()? >= n {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "no reply from Discord"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn read_frame(pipe: &mut dyn Pipe, timeout: Duration) -> io::Result<(u32, Value)> {
    let deadline = Instant::now() + timeout;
    wait_for(pipe, 8, deadline)?;
    let mut header = [0u8; 8];
    pipe.read_exact(&mut header)?;
    let (op, len) = decode_header(&header);
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "oversized Discord frame"));
    }
    wait_for(pipe, len as usize, deadline)?;
    let mut body = vec![0u8; len as usize];
    pipe.read_exact(&mut body)?;
    let value = serde_json::from_slice(&body).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad Discord frame"))?;
    Ok((op, value))
}

/// Op 0 handshake; Discord answers READY, or closes (op 2) on an unknown client id.
fn handshake(pipe: &mut dyn Pipe, client_id: &str, timeout: Duration) -> io::Result<()> {
    pipe.write_all(&encode_frame(OP_HANDSHAKE, &json!({ "v": 1, "client_id": client_id })))?;
    match read_frame(pipe, timeout)? {
        (OP_FRAME, reply) if reply["evt"] == "READY" => Ok(()),
        _ => Err(io::Error::new(io::ErrorKind::ConnectionRefused, "Discord refused the handshake")),
    }
}

/// Read and drop whatever Discord sent (replies to SET_ACTIVITY), so its side never fills up. Fails when the
/// pipe is closed.
fn drain(pipe: &mut dyn Pipe) -> io::Result<()> {
    let n = pipe.available()?;
    if n > 0 {
        let mut sink = vec![0u8; n];
        pipe.read_exact(&mut sink)?;
    }
    Ok(())
}

#[cfg(windows)]
mod win {
    use super::Pipe;
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Pipes::PeekNamedPipe;

    struct WinPipe(File);

    impl Pipe for WinPipe {
        fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
            self.0.write_all(data)?;
            self.0.flush()
        }

        fn available(&mut self) -> io::Result<usize> {
            let mut avail = 0u32;
            // SAFETY: the handle belongs to the open File for the duration of the call; no buffer is passed.
            unsafe { PeekNamedPipe(HANDLE(self.0.as_raw_handle()), None, 0, None, Some(&mut avail), None) }
                .map_err(io::Error::other)?;
            Ok(avail as usize)
        }

        fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
            Read::read_exact(&mut self.0, buf)
        }
    }

    /// Open the first Discord pipe that exists. Never starts Discord.
    pub fn connect() -> io::Result<Box<dyn Pipe>> {
        for i in 0..10 {
            if let Ok(f) = OpenOptions::new().read(true).write(true).open(format!(r"\\.\pipe\discord-ipc-{i}")) {
                return Ok(Box::new(WinPipe(f)));
            }
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "Discord is not running"))
    }
}

/// The real connector: the Windows named pipe, or nothing on dev builds elsewhere.
fn system_connect() -> io::Result<Box<dyn Pipe>> {
    #[cfg(windows)]
    {
        win::connect()
    }
    #[cfg(not(windows))]
    {
        Err(io::Error::new(io::ErrorKind::Unsupported, "Discord presence is Windows only"))
    }
}

// ---- Connection thread ----------------------------------------------------------------------------------

enum Msg {
    Show(Value),
    Clear,
    /// Clear the activity, close the pipe, then answer.
    Shutdown(Sender<()>),
}

#[derive(Clone, Copy)]
struct Timing {
    /// At most one SET_ACTIVITY per this long.
    min_gap: Duration,
    /// How often an open pipe is checked (and drained) while nothing else happens.
    tick: Duration,
    /// How long the handshake may take.
    handshake: Duration,
}

const TIMING: Timing =
    Timing { min_gap: Duration::from_secs(2), tick: Duration::from_secs(5), handshake: Duration::from_secs(3) };

/// The state the connection thread owns. Driven by `wait` + `handle` + `step`, so tests run it without a thread
/// or a real clock.
struct Worker {
    client_id: String,
    connect: Connect,
    timing: Timing,
    conn: Option<Box<dyn Pipe>>,
    /// What Discord should show (`None` = nothing).
    desired: Option<Value>,
    /// What Discord shows now as far as we know (`None` after a clear or on a fresh connection).
    shown: Option<Value>,
    last_send: Option<Instant>,
    /// Failed attempts in a row; reset by a SET_ACTIVITY that went through.
    failures: u32,
    next_connect: Instant,
    nonce: u64,
}

impl Worker {
    fn new(client_id: &str, connect: Connect, timing: Timing, now: Instant) -> Self {
        Worker {
            client_id: client_id.to_string(),
            connect,
            timing,
            conn: None,
            desired: None,
            shown: None,
            last_send: None,
            failures: 0,
            next_connect: now,
            nonce: 0,
        }
    }

    /// How long the thread may sleep waiting for a message before `step` has work to do.
    fn wait(&self, now: Instant) -> Duration {
        if self.conn.is_some() {
            match gate(self.desired.as_ref(), self.shown.as_ref(), self.last_send.map(|l| now - l), self.timing.min_gap) {
                Gate::Send => Duration::ZERO,
                Gate::Wait(d) => d.min(self.timing.tick),
                Gate::Skip => self.timing.tick,
            }
        } else if self.desired.is_some() {
            self.next_connect.saturating_duration_since(now)
        } else {
            // Nothing to show and no pipe: sleep until the app sends something.
            Duration::from_secs(3600)
        }
    }

    /// Apply one message; `false` = the thread ends.
    fn handle(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Show(activity) => self.desired = Some(activity),
            Msg::Clear => self.desired = None,
            Msg::Shutdown(ack) => {
                self.close();
                let _ = ack.send(());
                return false;
            }
        }
        true
    }

    /// Clear our activity (best effort, no rate limit) and drop the pipe. Discord also clears an activity
    /// when its pipe closes, so this is belt and braces.
    fn close(&mut self) {
        if let Some(mut pipe) = self.conn.take() {
            if self.shown.is_some() {
                self.nonce += 1;
                let _ = pipe.write_all(&encode_frame(OP_FRAME, &set_activity_cmd(None, std::process::id(), self.nonce)));
            }
        }
        self.shown = None;
    }

    /// The pipe broke: drop it and reconnect at once if it had been healthy, else after the backoff.
    fn broken(&mut self, now: Instant) {
        self.conn = None;
        self.shown = None;
        self.next_connect = if self.failures == 0 { now } else { now + backoff(self.failures) };
    }

    /// Connect when needed, notice a closed pipe, and send the desired activity when the gate allows.
    fn step(&mut self, now: Instant) {
        if let Some(pipe) = self.conn.as_mut() {
            if drain(pipe.as_mut()).is_err() {
                self.broken(now);
            }
        }
        if self.conn.is_none() {
            // Only touch the pipe when there is something to show.
            if self.desired.is_none() || now < self.next_connect {
                return;
            }
            let timeout = self.timing.handshake;
            let id = self.client_id.clone();
            match (self.connect)().and_then(|mut p| handshake(p.as_mut(), &id, timeout).map(|_| p)) {
                Ok(pipe) => {
                    self.conn = Some(pipe);
                    self.shown = None;
                }
                Err(_) => {
                    self.failures += 1;
                    self.next_connect = now + backoff(self.failures);
                    return;
                }
            }
        }
        let since = self.last_send.map(|l| now.saturating_duration_since(l));
        if gate(self.desired.as_ref(), self.shown.as_ref(), since, self.timing.min_gap) != Gate::Send {
            return;
        }
        self.nonce += 1;
        let frame = encode_frame(OP_FRAME, &set_activity_cmd(self.desired.as_ref(), std::process::id(), self.nonce));
        let Some(pipe) = self.conn.as_mut() else { return };
        match pipe.write_all(&frame) {
            Ok(()) => {
                self.shown = self.desired.clone();
                self.last_send = Some(now);
                self.failures = 0;
            }
            Err(_) => {
                // A write that fails right after a reconnect counts as a failure, so it cannot spin.
                self.broken(now);
                self.failures += 1;
            }
        }
    }
}

fn run_worker(mut w: Worker, rx: Receiver<Msg>) {
    loop {
        match rx.recv_timeout(w.wait(Instant::now())) {
            Ok(msg) => {
                if !w.handle(msg) {
                    return;
                }
                // Act on the newest state only.
                while let Ok(more) = rx.try_recv() {
                    if !w.handle(more) {
                        return;
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            // The app is gone: dropping the pipe makes Discord clear the activity.
            Err(RecvTimeoutError::Disconnected) => return,
        }
        w.step(Instant::now());
    }
}

// ---- App side -------------------------------------------------------------------------------------------

/// What the app knows; the activity follows from it.
#[derive(Clone, Debug, PartialEq)]
struct View {
    /// Settings `discordPresence`.
    enabled: bool,
    /// The main window is shown (not hidden to the tray).
    visible: bool,
    doing: Doing,
    accounts: usize,
    /// When the window was shown (Unix ms); kept while it stays shown, reset when it hides.
    started_at: Option<u64>,
}

impl View {
    fn settle(&mut self, now_ms: u64) {
        if !self.visible {
            self.started_at = None;
        } else if self.started_at.is_none() {
            self.started_at = Some(now_ms);
        }
    }

    /// The activity to show, `None` = clear.
    fn desired(&self) -> Option<Value> {
        match (self.enabled && self.visible, self.started_at) {
            (true, Some(start)) => Some(activity_for(self.doing, self.accounts, start)),
            _ => None,
        }
    }
}

/// The app's handle on the presence thread (Tauri state). Inert when `APP_ID` is empty.
pub struct Presence {
    tx: Option<Sender<Msg>>,
    view: Mutex<View>,
    /// The last thing sent to the thread, so unchanged views send nothing.
    pushed: Mutex<Option<Option<Value>>>,
}

impl Presence {
    pub fn start() -> Self {
        Self::spawn(APP_ID, Box::new(system_connect))
    }

    fn spawn(app_id: &str, connect: Connect) -> Self {
        let tx = (!app_id.trim().is_empty()).then(|| {
            let (tx, rx) = mpsc::channel();
            let worker = Worker::new(app_id.trim(), connect, TIMING, Instant::now());
            let spawned = std::thread::Builder::new().name("discord-presence".into()).spawn(move || run_worker(worker, rx));
            spawned.ok().map(|_| tx)
        });
        Presence {
            tx: tx.flatten(),
            view: Mutex::new(View { enabled: false, visible: false, doing: Doing::Idle, accounts: 0, started_at: None }),
            pushed: Mutex::new(None),
        }
    }

    fn active(&self) -> bool {
        self.tx.is_some()
    }

    /// Change the view and tell the thread if the activity changed. Never blocks on Discord.
    fn update(&self, change: impl FnOnce(&mut View)) {
        let Some(tx) = &self.tx else { return };
        let Ok(mut view) = self.view.lock() else { return };
        change(&mut view);
        view.settle(crate::ranks::now_ms());
        let want = view.desired();
        let Ok(mut pushed) = self.pushed.lock() else { return };
        if pushed.as_ref() == Some(&want) {
            return;
        }
        let msg = match &want {
            Some(activity) => Msg::Show(activity.clone()),
            None => Msg::Clear,
        };
        let _ = tx.send(msg);
        *pushed = Some(want);
    }

    /// Clear the activity and stop the thread, waiting at most `wait`.
    fn stop(&self, wait: Duration) {
        let Some(tx) = &self.tx else { return };
        let (ack_tx, ack_rx) = mpsc::channel();
        if tx.send(Msg::Shutdown(ack_tx)).is_ok() {
            let _ = ack_rx.recv_timeout(wait);
        }
    }
}

fn state<R: Runtime>(app: &AppHandle<R>) -> Option<Arc<Presence>> {
    app.try_state::<Arc<Presence>>().map(|s| s.inner().clone()).filter(|p| p.active())
}

/// Re-read the setting and the saved account count, plus any extra change, and update the activity.
fn refresh_with<R: Runtime>(app: &AppHandle<R>, extra: impl FnOnce(&mut View)) {
    let Some(p) = state(app) else { return };
    let be = app.state::<Arc<crate::engine::Backend>>();
    let enabled = be.get_settings().discord_presence;
    let accounts = be.list_accounts().ok().map(|a| a.len());
    p.update(|v| {
        v.enabled = enabled;
        if let Some(n) = accounts {
            v.accounts = n;
        }
        extra(v);
    });
}

/// Settings or the account list changed.
pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    refresh_with(app, |_| {});
}

/// The main window was shown (`true`) or hidden to the tray (`false`).
pub fn set_visible<R: Runtime>(app: &AppHandle<R>, visible: bool) {
    refresh_with(app, |v| v.visible = visible);
}

/// Shows `doing` until the guard drops, then goes back to idle (with a fresh account count).
pub struct Busy<R: Runtime> {
    app: AppHandle<R>,
}

pub fn busy<R: Runtime>(app: &AppHandle<R>, doing: Doing) -> Busy<R> {
    if let Some(p) = state(app) {
        p.update(|v| v.doing = doing);
    }
    Busy { app: app.clone() }
}

impl<R: Runtime> Drop for Busy<R> {
    fn drop(&mut self) {
        refresh_with(&self.app, |v| v.doing = Doing::Idle);
    }
}

/// Before the process exits: clear the activity, best effort, at most half a second.
pub fn shutdown<R: Runtime>(app: &AppHandle<R>) {
    if let Some(p) = state(app) {
        p.stop(SHUTDOWN_WAIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn idle_activity_is_exact_and_carries_no_account_data() {
        let a = activity_for(Doing::Idle, 3, 1_761_300_000_000);
        assert_eq!(
            a,
            json!({
                "details": "Managing Battle.net accounts",
                "state": "Overwatch, 3 accounts saved",
                "timestamps": { "start": 1_761_300_000_000u64 },
                "assets": {
                    "large_image": "eagleeye",
                    "large_text": "EagleEye Account Switcher",
                    "small_image": "battlenet",
                    "small_text": "Battle.net",
                },
            })
        );
        assert_eq!(activity_for(Doing::Idle, 1, 0)["state"], "Overwatch, 1 account saved");
        assert_eq!(activity_for(Doing::Idle, 0, 0)["state"], "Overwatch, no accounts saved yet");
    }

    #[test]
    fn switching_and_adding_activities() {
        let s = activity_for(Doing::Switching, 3, 42);
        assert_eq!(s["details"], "Switching accounts");
        assert_eq!(s["state"], "Signing in to Battle.net");
        assert_eq!(s["assets"]["small_image"], "overwatch");
        assert_eq!(s["assets"]["small_text"], "Overwatch");
        assert_eq!(s["assets"]["large_image"], "eagleeye");
        assert_eq!(s["timestamps"]["start"], 42);
        let a = activity_for(Doing::Adding, 3, 42);
        assert_eq!(a["details"], "Adding an account");
        assert_eq!(a["state"], "Waiting for the Battle.net login");
        assert_eq!(a["timestamps"]["start"], 42, "the start time stays across states");
    }

    #[test]
    fn gate_dedupes_and_rate_limits() {
        let gap = Duration::from_secs(2);
        let a = json!({"details": "a"});
        let b = json!({"details": "b"});
        assert_eq!(gate(Some(&a), Some(&a), None, gap), Gate::Skip, "same activity");
        assert_eq!(gate(None, None, Some(Duration::ZERO), gap), Gate::Skip, "nothing shown, nothing wanted");
        assert_eq!(gate(Some(&a), None, None, gap), Gate::Send, "first send");
        assert_eq!(gate(Some(&b), Some(&a), Some(Duration::from_millis(500)), gap), Gate::Wait(Duration::from_millis(1500)));
        assert_eq!(gate(Some(&b), Some(&a), Some(gap), gap), Gate::Send, "gap passed");
        assert_eq!(gate(None, Some(&a), Some(Duration::from_secs(10)), gap), Gate::Send, "clear");
    }

    #[test]
    fn backoff_doubles_from_15_s_to_2_min() {
        let secs: Vec<u64> = (1..=6).map(|f| backoff(f).as_secs()).collect();
        assert_eq!(secs, vec![15, 30, 60, 120, 120, 120]);
        assert_eq!(backoff(0).as_secs(), 15);
        assert_eq!(backoff(u32::MAX).as_secs(), 120);
    }

    #[test]
    fn frames_encode_little_endian_and_decode_back() {
        let f = encode_frame(OP_HANDSHAKE, &json!({"v": 1, "client_id": "123"}));
        let body = r#"{"v":1,"client_id":"123"}"#;
        assert_eq!(&f[..4], &[0, 0, 0, 0]);
        assert_eq!(&f[4..8], &(body.len() as u32).to_le_bytes());
        assert_eq!(&f[8..], body.as_bytes());
        let header: [u8; 8] = encode_frame(OP_FRAME, &json!({}))[..8].try_into().unwrap();
        assert_eq!(decode_header(&header), (1, 2));
        assert_eq!(decode_header(&[1, 0, 0, 0, 0x10, 0x01, 0, 0]), (1, 0x110));
    }

    #[test]
    fn set_activity_command_shape() {
        let a = json!({"details": "x"});
        let cmd = set_activity_cmd(Some(&a), 77, 5);
        assert_eq!(cmd, json!({"cmd": "SET_ACTIVITY", "args": {"pid": 77, "activity": {"details": "x"}}, "nonce": "5"}));
        assert!(set_activity_cmd(None, 77, 6)["args"]["activity"].is_null(), "clear sends a null activity");
    }

    #[test]
    fn view_start_time_is_stable_while_shown_and_resets_on_hide() {
        let mut v = View { enabled: true, visible: true, doing: Doing::Idle, accounts: 2, started_at: None };
        v.settle(1_000);
        assert_eq!(v.started_at, Some(1_000));
        v.doing = Doing::Switching;
        v.settle(5_000);
        assert_eq!(v.started_at, Some(1_000), "stable across updates");
        assert_eq!(v.desired().unwrap()["timestamps"]["start"], 1_000);
        v.visible = false;
        v.settle(6_000);
        assert_eq!(v.desired(), None, "hidden: cleared");
        v.visible = true;
        v.settle(9_000);
        assert_eq!(v.started_at, Some(9_000), "shown again: new start");
        v.enabled = false;
        assert_eq!(v.desired(), None, "setting off: cleared");
    }

    #[test]
    fn empty_app_id_is_inert() {
        let p = Presence::spawn("", Box::new(|| panic!("the pipe must not be touched")));
        assert!(!p.active());
        p.update(|v| {
            v.enabled = true;
            v.visible = true;
        });
        p.stop(Duration::from_millis(10));
    }

    // ---- Worker against a fake Discord ----

    #[derive(Default)]
    struct Fake {
        /// Bytes Discord "sent" to us.
        inbox: VecDeque<u8>,
        /// Frames we wrote, decoded.
        written: Vec<(u32, Value)>,
        closed: bool,
        fail_writes: bool,
    }

    struct FakePipe(Arc<Mutex<Fake>>);

    impl Pipe for FakePipe {
        fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
            let mut f = self.0.lock().unwrap();
            if f.closed || f.fail_writes {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"));
            }
            let (op, len) = decode_header(data[..8].try_into().unwrap());
            assert_eq!(len as usize, data.len() - 8);
            let body: Value = serde_json::from_slice(&data[8..]).unwrap();
            if op == OP_HANDSHAKE {
                // Discord answers READY.
                let ready = encode_frame(OP_FRAME, &json!({"cmd": "DISPATCH", "evt": "READY"}));
                f.inbox.extend(ready);
            }
            f.written.push((op, body));
            Ok(())
        }
        fn available(&mut self) -> io::Result<usize> {
            let f = self.0.lock().unwrap();
            if f.closed {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"));
            }
            Ok(f.inbox.len())
        }
        fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
            let mut f = self.0.lock().unwrap();
            for b in buf.iter_mut() {
                *b = f.inbox.pop_front().ok_or(io::ErrorKind::UnexpectedEof)?;
            }
            Ok(())
        }
    }

    const FAST: Timing =
        Timing { min_gap: Duration::from_secs(2), tick: Duration::from_secs(5), handshake: Duration::from_millis(200) };

    type Fakes = Arc<Mutex<Vec<Arc<Mutex<Fake>>>>>;

    /// A worker whose connector hands out fake pipes while `up` is true; returns the shared fakes.
    fn worker(up: Arc<Mutex<bool>>, t0: Instant) -> (Worker, Fakes) {
        let pipes: Fakes = Arc::default();
        let p2 = pipes.clone();
        let connect: Connect = Box::new(move || {
            if !*up.lock().unwrap() {
                return Err(io::Error::new(io::ErrorKind::NotFound, "Discord is not running"));
            }
            let fake = Arc::new(Mutex::new(Fake::default()));
            p2.lock().unwrap().push(fake.clone());
            Ok(Box::new(FakePipe(fake)) as Box<dyn Pipe>)
        });
        (Worker::new("123", connect, FAST, t0), pipes)
    }

    fn activities(fake: &Arc<Mutex<Fake>>) -> Vec<Value> {
        let f = fake.lock().unwrap();
        f.written.iter().filter(|(op, _)| *op == OP_FRAME).map(|(_, b)| b["args"]["activity"].clone()).collect()
    }

    #[test]
    fn worker_handshakes_sends_dedupes_and_rate_limits() {
        let t0 = Instant::now();
        let (mut w, pipes) = worker(Arc::new(Mutex::new(true)), t0);
        assert_eq!(w.wait(t0), Duration::from_secs(3600), "nothing to show: no pipe access, long sleep");
        w.step(t0);
        assert!(pipes.lock().unwrap().is_empty(), "nothing to show: never connects");

        let a = json!({"details": "a"});
        let b = json!({"details": "b"});
        w.handle(Msg::Show(a.clone()));
        w.step(t0);
        let fake = pipes.lock().unwrap()[0].clone();
        {
            let f = fake.lock().unwrap();
            assert_eq!(f.written[0], (OP_HANDSHAKE, json!({"v": 1, "client_id": "123"})));
        }
        assert_eq!(activities(&fake), vec![a.clone()]);

        // Same activity again: nothing sent.
        w.handle(Msg::Show(a.clone()));
        w.step(t0 + Duration::from_secs(3));
        assert_eq!(activities(&fake).len(), 1);

        // A change within 2 s waits; it goes out once the gap has passed.
        w.handle(Msg::Show(b.clone()));
        let t1 = t0 + Duration::from_millis(500);
        w.step(t1);
        assert_eq!(activities(&fake).len(), 1);
        assert_eq!(w.wait(t1), Duration::from_millis(1500));
        w.step(t0 + Duration::from_secs(2));
        assert_eq!(activities(&fake), vec![a, b]);

        // Clear goes out as a null activity.
        w.handle(Msg::Clear);
        w.step(t0 + Duration::from_secs(5));
        assert!(activities(&fake).last().unwrap().is_null());
    }

    #[test]
    fn worker_backs_off_while_discord_is_closed_and_reconnects_with_the_last_activity() {
        let t0 = Instant::now();
        let up = Arc::new(Mutex::new(false));
        let (mut w, pipes) = worker(up.clone(), t0);
        let a = json!({"details": "a"});
        w.handle(Msg::Show(a.clone()));
        w.step(t0);
        assert_eq!(w.wait(t0), Duration::from_secs(15), "first retry after 15 s");
        w.step(t0 + Duration::from_secs(15));
        assert_eq!(w.wait(t0 + Duration::from_secs(15)), Duration::from_secs(30), "then 30 s");
        w.step(t0 + Duration::from_secs(20));
        assert!(pipes.lock().unwrap().is_empty(), "no attempt before the backoff ends");

        // Discord starts: the next attempt connects and shows the activity.
        *up.lock().unwrap() = true;
        let t1 = t0 + Duration::from_secs(45);
        w.step(t1);
        assert_eq!(activities(&pipes.lock().unwrap()[0]), vec![a.clone()]);
        assert_eq!(w.failures, 0);

        // Discord closes while idle: the drain notices, the worker reconnects at once and resends.
        pipes.lock().unwrap()[0].lock().unwrap().closed = true;
        let t2 = t1 + Duration::from_secs(10);
        w.step(t2);
        let all = pipes.lock().unwrap();
        assert_eq!(all.len(), 2, "reconnected");
        assert_eq!(activities(&all[1]), vec![a]);
    }

    #[test]
    fn worker_write_failures_cannot_spin() {
        let t0 = Instant::now();
        let (mut w, pipes) = worker(Arc::new(Mutex::new(true)), t0);
        w.handle(Msg::Show(json!({"details": "a"})));
        // Every pipe fails its SET_ACTIVITY write (the handshake goes through first).
        w.step(t0);
        pipes.lock().unwrap()[0].lock().unwrap().fail_writes = true;
        w.handle(Msg::Show(json!({"details": "b"})));
        w.step(t0 + Duration::from_secs(3));
        assert!(w.conn.is_none());
        assert_eq!(w.failures, 1);
        assert_eq!(w.wait(t0 + Duration::from_secs(3)), Duration::ZERO, "a healthy pipe that broke: reconnect at once");
    }

    #[test]
    fn worker_shutdown_clears_and_acks() {
        let t0 = Instant::now();
        let (mut w, pipes) = worker(Arc::new(Mutex::new(true)), t0);
        w.handle(Msg::Show(json!({"details": "a"})));
        w.step(t0);
        let (ack_tx, ack_rx) = mpsc::channel();
        assert!(!w.handle(Msg::Shutdown(ack_tx)));
        assert!(ack_rx.try_recv().is_ok());
        assert!(activities(&pipes.lock().unwrap()[0]).last().unwrap().is_null(), "cleared before closing");
        assert!(w.conn.is_none());
    }

    #[test]
    fn thread_stops_within_the_timeout_and_never_blocks_the_caller() {
        let p = Presence::spawn("123", Box::new(|| Err(io::Error::new(io::ErrorKind::NotFound, "not running"))));
        assert!(p.active());
        let t = Instant::now();
        p.update(|v| {
            v.enabled = true;
            v.visible = true;
        });
        assert!(t.elapsed() < Duration::from_millis(100), "update never waits on Discord");
        p.stop(Duration::from_secs(2));
        assert!(t.elapsed() < Duration::from_secs(2), "the thread answered the shutdown");
    }
}
