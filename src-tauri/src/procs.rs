//! Process control: is a Blizzard game running (a switch closes it first), is Battle.net running,
//! closing Battle.net, and starting it again. Games are found by exe name and confirmed by full
//! image path. The close is deliberately narrow: only `Battle.net.exe`
//! whose image path lies inside the configured Battle.net folder is terminated, and we wait on the
//! process handles for exit. `Agent.exe` and everything else is left running (hard invariant).

/// Game processes that must not be running during a switch. Lowercase exe name (or name prefix for
/// the versioned ones) -> the friendly game name shown to the user.
#[cfg_attr(not(windows), allow(dead_code))]
const GAME_PROCESSES: &[(&str, bool, &str)] = &[
    // (match, is_prefix, label)
    ("overwatch.exe", false, "Overwatch"),
    ("wow.exe", false, "World of Warcraft"),
    ("wowclassic.exe", false, "World of Warcraft"),
    ("diablo iv.exe", false, "Diablo IV"),
    ("diabloiv.exe", false, "Diablo IV"),
    ("diablo iii", true, "Diablo III"),
    ("diablo ii", true, "Diablo II"),
    ("hearthstone.exe", false, "Hearthstone"),
    ("heroesofthestorm", true, "Heroes of the Storm"),
    ("sc2", true, "StarCraft II"),
    ("starcraft", true, "StarCraft"),
    ("warcraft", true, "Warcraft"),
    ("w3champions.exe", false, "Warcraft III"),
];

/// The friendly name of a Blizzard game if `exe_name` is one, else `None`. Pure, so the table is
/// testable without a process list.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn game_label(exe_name: &str) -> Option<&'static str> {
    let name = exe_name.trim().to_lowercase();
    for (m, is_prefix, label) in GAME_PROCESSES {
        let hit = if *is_prefix { name.starts_with(m) } else { name == *m };
        if hit {
            return Some(label);
        }
    }
    None
}

/// The friendly game name for a full image path: `None` for anything that is not a listed Blizzard
/// game. Pure, so it is unit-tested.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn game_label_for_path(path: &str) -> Option<&'static str> {
    let n = norm(path);
    game_label(n.rsplit('\\').next().unwrap_or(&n))
}

#[cfg_attr(not(windows), allow(dead_code))]
pub const BATTLENET_EXE: &str = "battle.net.exe";

/// Lowercase, backslashes, no `\\?\` prefix, no trailing separator.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn norm(p: &str) -> String {
    let s = p.trim().replace('/', "\\").to_lowercase();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
    s.trim_end_matches('\\').to_string()
}

/// Whether `path` is inside `root` (whole path components).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn is_under(path: &str, root: &str) -> bool {
    let (p, r) = (norm(path), norm(root));
    !r.is_empty() && (p == r || p.starts_with(&format!("{r}\\")))
}

/// Battle.net's login window is a small portrait window (362x663 at 100% scaling). Its main window
/// is landscape and much wider. `w`/`h` are physical pixels from `GetWindowRect`, `dpi` the window's
/// DPI (96 = 100%), so the check holds at any display scaling. Pure, so it is unit-tested.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn is_login_window_size(w: i32, h: i32, dpi: u32) -> bool {
    let dpi = if dpi == 0 { 96 } else { dpi } as i64;
    let (lw, lh) = (w as i64 * 96 / dpi, h as i64 * 96 / dpi);
    // Narrower than 500 logical px, but not a tiny helper/tooltip window, and clearly portrait.
    (250..500).contains(&lw) && lh >= 450 && lh * 10 >= lw * 13
}

/// Is `exe_name` one of Battle.net's own self-update or setup processes? Best effort: Blizzard ships
/// the self-update as `Battle.net-Setup.exe` / `Battle.net Setup.exe` and an update helper whose
/// name starts with `Battle.net` and contains `update`. We key on that shape (a `battle.net` name
/// prefix plus `setup` or `update`) rather than on one exact file name, since it changed over the
/// years. Game updates (run inside Agent.exe) do not match and do not block a switch.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn is_update_process(exe_name: &str) -> bool {
    let n = exe_name.trim().to_lowercase();
    n.starts_with("battle.net") && n.ends_with(".exe") && (n.contains("setup") || n.contains("update"))
}

/// Process control behind a trait so the engine's state machine runs with fakes on Linux.
pub trait Procs: Send + Sync {
    /// The friendly name of a running Blizzard game, if any (the first one found).
    fn running_game(&self) -> Option<String>;
    /// Close every running Blizzard game: WM_CLOSE to its top-level
    /// windows, up to 3 s to exit, then TerminateProcess and up to 10 s more; one more round if a
    /// fresh snapshot still shows one. Returns the labels of games that exited. The caller re-checks
    /// `running_game` to learn whether anything survived.
    fn close_games(&self) -> Vec<String>;
    /// Is Battle.net.exe running from inside `battlenet_dir`?
    fn battlenet_running(&self, battlenet_dir: &str) -> bool;
    /// Terminate every Battle.net.exe whose image path is inside `battlenet_dir`, wait for exit.
    /// Leaves Agent.exe and everything else alone. Ok even when nothing was running.
    fn close_battlenet(&self, battlenet_dir: &str) -> Result<(), String>;
    /// Start `Battle.net Launcher.exe` at the given path.
    fn start_battlenet(&self, launcher_exe: &str) -> Result<(), String>;
    /// Is Battle.net's login window on screen (a visible top-level window of a Battle.net.exe from
    /// `battlenet_dir` with the login window's shape, see `is_login_window_size`)?
    fn login_window_visible(&self, battlenet_dir: &str) -> bool;
    /// Bring Battle.net's main window forward (restore if minimised, then foreground; flash the
    /// taskbar button if Windows refuses focus). `false` when no visible window was found (for
    /// example Battle.net sits hidden in its own tray).
    fn focus_battlenet(&self, battlenet_dir: &str) -> bool;
    /// Is Battle.net updating itself right now (a setup/update process is running)?
    fn battlenet_updating(&self) -> bool;
}

/// The real process control.
#[cfg(windows)]
pub struct WinProcs;

#[cfg(windows)]
impl Procs for WinProcs {
    fn running_game(&self) -> Option<String> {
        win::game_procs(false).first().map(|g| g.label.to_string())
    }
    fn close_games(&self) -> Vec<String> {
        win::close_games()
    }
    fn battlenet_running(&self, battlenet_dir: &str) -> bool {
        !win::battlenet_pids(battlenet_dir).is_empty()
    }
    fn close_battlenet(&self, battlenet_dir: &str) -> Result<(), String> {
        win::close_battlenet(battlenet_dir)
    }
    fn start_battlenet(&self, launcher_exe: &str) -> Result<(), String> {
        win::start_battlenet(launcher_exe)
    }
    fn login_window_visible(&self, battlenet_dir: &str) -> bool {
        let pids = win::battlenet_pids(battlenet_dir);
        !pids.is_empty() && win::windows_of(&pids).iter().any(|w| is_login_window_size(w.w, w.h, w.dpi))
    }
    fn focus_battlenet(&self, battlenet_dir: &str) -> bool {
        win::focus(&win::battlenet_pids(battlenet_dir))
    }
    fn battlenet_updating(&self) -> bool {
        win::snapshot_names().iter().any(|n| is_update_process(n))
    }
}

#[cfg(windows)]
mod win {
    use super::{game_label, game_label_for_path, is_under, BATTLENET_EXE};
    use std::time::{Duration, Instant};
    use windows::core::{BOOL, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, RECT, WAIT_OBJECT_0, WPARAM};
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, EnumWindows, FlashWindowEx, GetWindow, GetWindowRect, GetWindowThreadProcessId,
        IsIconic, IsWindowVisible, PostMessageW, SetForegroundWindow, ShowWindow, FLASHWINFO, FLASHW_ALL,
        FLASHW_TIMERNOFG, GW_OWNER, SW_RESTORE, WM_CLOSE,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    };

    /// Every running process (pid, lowercase exe name).
    fn processes() -> Vec<(u32, String)> {
        let mut out = Vec::new();
        // SAFETY: snapshot owned by this frame and closed below; the entry is sized as required.
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
            let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut ok = Process32FirstW(snap, &mut e).is_ok();
            while ok {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                out.push((e.th32ProcessID, String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase()));
                ok = Process32NextW(snap, &mut e).is_ok();
            }
            let _ = CloseHandle(snap);
        }
        out
    }

    pub fn snapshot_names() -> Vec<String> {
        processes().into_iter().map(|(_, n)| n).collect()
    }

    fn image_path(h: HANDLE) -> Option<String> {
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        // SAFETY: buf holds `len` u16s; the API writes at most that many and updates `len`.
        unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) }.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }

    /// Pids of Battle.net.exe running from inside `battlenet_dir`.
    pub fn battlenet_pids(battlenet_dir: &str) -> Vec<u32> {
        let mut pids = Vec::new();
        for (pid, name) in processes() {
            if pid == 0 || name != BATTLENET_EXE {
                continue;
            }
            // SAFETY: a plain OpenProcess; the handle is closed before the loop continues.
            let Ok(h) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else { continue };
            let path = image_path(h);
            // SAFETY: an owned handle.
            unsafe { let _ = CloseHandle(h); }
            if let Some(p) = path {
                if is_under(&p, battlenet_dir) {
                    pids.push(pid);
                }
            }
        }
        pids
    }

    struct Owned(HANDLE);
    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: an owned handle.
            unsafe { let _ = CloseHandle(self.0); }
        }
    }

    pub fn close_battlenet(battlenet_dir: &str) -> Result<(), String> {
        let pids = battlenet_pids(battlenet_dir);
        if pids.is_empty() {
            return Ok(());
        }
        let mut handles: Vec<Owned> = Vec::new();
        for pid in pids {
            // SAFETY: OpenProcess with TERMINATE + SYNCHRONIZE; the handle is owned by `Owned`.
            if let Ok(h) = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid) } {
                // SAFETY: an owned handle with PROCESS_TERMINATE.
                let _ = unsafe { TerminateProcess(h, 1) };
                handles.push(Owned(h));
            }
        }
        // Wait up to 5 s for each to exit.
        let deadline = Instant::now() + Duration::from_secs(5);
        for owned in &handles {
            let remaining = deadline.saturating_duration_since(Instant::now());
            // SAFETY: an owned handle with SYNCHRONIZE.
            let _ = unsafe { WaitForSingleObject(owned.0, remaining.as_millis() as u32) };
        }
        // Confirm none survive.
        if battlenet_pids(battlenet_dir).is_empty() {
            Ok(())
        } else {
            Err("Battle.net did not close".into())
        }
    }

    /// A running listed game. `handle` is `None` when its image path could not be read: it still
    /// counts as running (cautious), but it is never closed, since its path cannot be checked.
    pub struct GameProc {
        pub pid: u32,
        pub label: &'static str,
        handle: Option<Owned>,
    }

    /// Every running listed game. With `for_close` the handle also carries
    /// TERMINATE + SYNCHRONIZE, and the path check runs on that same handle, so a reused pid cannot
    /// slip a different process in between the check and the kill.
    pub fn game_procs(for_close: bool) -> Vec<GameProc> {
        let mut out = Vec::new();
        for (pid, name) in processes() {
            if pid == 0 || game_label(&name).is_none() {
                continue;
            }
            let mut access = PROCESS_QUERY_LIMITED_INFORMATION;
            if for_close {
                access |= PROCESS_TERMINATE | PROCESS_SYNCHRONIZE;
            }
            // SAFETY: a plain OpenProcess; the handle is owned by `Owned` and closed on drop.
            let owned = unsafe { OpenProcess(access, false, pid) }.ok().map(Owned);
            let path = owned.as_ref().and_then(|o| image_path(o.0));
            match path {
                Some(p) => {
                    let Some(label) = game_label_for_path(&p) else { continue };
                    out.push(GameProc { pid, label, handle: owned });
                }
                None => {
                    let label = game_label(&name).unwrap_or("the game");
                    out.push(GameProc { pid, label, handle: None });
                }
            }
        }
        out
    }

    /// Has the process behind `h` exited, waiting at most until `deadline`?
    fn wait_exit(h: HANDLE, deadline: Instant) -> bool {
        let ms = deadline.saturating_duration_since(Instant::now()).as_millis().min(u32::MAX as u128) as u32;
        // SAFETY: an owned handle with SYNCHRONIZE.
        unsafe { WaitForSingleObject(h, ms) == WAIT_OBJECT_0 }
    }

    pub fn close_games() -> Vec<String> {
        let mut closed: Vec<String> = Vec::new();
        for _round in 0..2 {
            let games: Vec<GameProc> = game_procs(true).into_iter().filter(|g| g.handle.is_some()).collect();
            if games.is_empty() {
                break;
            }
            // Ask politely: WM_CLOSE to every top-level window of each game.
            let pids: Vec<u32> = games.iter().map(|g| g.pid).collect();
            for hwnd in top_level_windows_of(&pids) {
                // SAFETY: posting a message to a window handle we just enumerated; failure is harmless.
                let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
            }
            let soft = Instant::now() + Duration::from_secs(3);
            let mut alive: Vec<&GameProc> = Vec::new();
            for g in &games {
                let h = g.handle.as_ref().map(|o| o.0).unwrap_or_default();
                if wait_exit(h, soft) {
                    push_label(&mut closed, g.label);
                } else {
                    alive.push(g);
                }
            }
            // Still there: terminate, then wait for the exit to land.
            for g in &alive {
                let h = g.handle.as_ref().map(|o| o.0).unwrap_or_default();
                // SAFETY: an owned handle with PROCESS_TERMINATE.
                let _ = unsafe { TerminateProcess(h, 1) };
            }
            let hard = Instant::now() + Duration::from_secs(10);
            for g in &alive {
                let h = g.handle.as_ref().map(|o| o.0).unwrap_or_default();
                if wait_exit(h, hard) {
                    push_label(&mut closed, g.label);
                }
            }
        }
        closed
    }

    fn push_label(v: &mut Vec<String>, label: &str) {
        if !v.iter().any(|l| l == label) {
            v.push(label.to_string());
        }
    }

    unsafe extern "system" fn enum_all_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam is the &mut Collect passed by `top_level_windows_of`, alive for the whole call.
        let c = unsafe { &mut *(lparam.0 as *mut Collect) };
        let mut pid = 0u32;
        // SAFETY: plain Win32 queries on a window handle EnumWindows just handed us.
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if c.pids.contains(&pid) && !GetWindow(hwnd, GW_OWNER).map(|o| !o.0.is_null()).unwrap_or(false) {
                c.out.push(Win { hwnd, w: 0, h: 0, dpi: 0 });
            }
        }
        true.into()
    }

    /// Every unowned top-level window of `pids`, hidden and minimised ones included.
    fn top_level_windows_of(pids: &[u32]) -> Vec<HWND> {
        if pids.is_empty() {
            return Vec::new();
        }
        let mut c = Collect { pids: pids.to_vec(), out: Vec::new() };
        // SAFETY: the callback only dereferences the Collect pointer during this call.
        unsafe {
            let _ = EnumWindows(Some(enum_all_cb), LPARAM(&mut c as *mut Collect as isize));
        }
        c.out.into_iter().map(|w| w.hwnd).collect()
    }

    /// A visible, top-level (unowned), non-minimised window and its physical size and DPI.
    pub struct Win {
        pub hwnd: HWND,
        pub w: i32,
        pub h: i32,
        pub dpi: u32,
    }

    struct Collect {
        pids: Vec<u32>,
        out: Vec<Win>,
    }

    unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam is the &mut Collect passed by `windows_of`, alive for the whole EnumWindows call.
        let c = unsafe { &mut *(lparam.0 as *mut Collect) };
        let mut pid = 0u32;
        // SAFETY: plain Win32 queries on a window handle EnumWindows just handed us.
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if !c.pids.contains(&pid) || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                return true.into();
            }
            if GetWindow(hwnd, GW_OWNER).map(|o| !o.0.is_null()).unwrap_or(false) {
                return true.into();
            }
            let mut r = RECT::default();
            if GetWindowRect(hwnd, &mut r).is_ok() {
                c.out.push(Win { hwnd, w: r.right - r.left, h: r.bottom - r.top, dpi: GetDpiForWindow(hwnd) });
            }
        }
        true.into()
    }

    /// Visible top-level windows owned by any of `pids`.
    pub fn windows_of(pids: &[u32]) -> Vec<Win> {
        if pids.is_empty() {
            return Vec::new();
        }
        let mut c = Collect { pids: pids.to_vec(), out: Vec::new() };
        // SAFETY: the callback only dereferences the Collect pointer during this call.
        unsafe {
            let _ = EnumWindows(Some(enum_cb), LPARAM(&mut c as *mut Collect as isize));
        }
        c.out
    }

    /// Restore and foreground the largest visible window of `pids`; flash it if focus is refused.
    pub fn focus(pids: &[u32]) -> bool {
        let wins = windows_of(pids);
        let Some(best) = wins.iter().max_by_key(|w| w.w as i64 * w.h as i64) else { return false };
        // SAFETY: plain Win32 window calls on a handle we just enumerated.
        unsafe {
            let _ = ShowWindow(best.hwnd, SW_RESTORE);
            for pid in pids {
                let _ = AllowSetForegroundWindow(*pid);
            }
            if !SetForegroundWindow(best.hwnd).as_bool() {
                let info = FLASHWINFO {
                    cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
                    hwnd: best.hwnd,
                    dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
                    uCount: 3,
                    dwTimeout: 0,
                };
                let _ = FlashWindowEx(&info);
            }
        }
        true
    }

    pub fn start_battlenet(launcher_exe: &str) -> Result<(), String> {
        std::process::Command::new(launcher_exe)
            .spawn()
            .map(|_| ())
            .map_err(|_| "could not start Battle.net".to_string())
    }
}

/// Dev builds off Windows: no real Battle.net to control.
#[cfg(not(windows))]
#[derive(Default)]
pub struct DevProcs;

#[cfg(not(windows))]
impl Procs for DevProcs {
    fn running_game(&self) -> Option<String> {
        None
    }
    fn close_games(&self) -> Vec<String> {
        Vec::new()
    }
    fn battlenet_running(&self, _dir: &str) -> bool {
        false
    }
    fn close_battlenet(&self, _dir: &str) -> Result<(), String> {
        Ok(())
    }
    fn start_battlenet(&self, _exe: &str) -> Result<(), String> {
        Ok(())
    }
    fn login_window_visible(&self, _dir: &str) -> bool {
        false
    }
    fn focus_battlenet(&self, _dir: &str) -> bool {
        false
    }
    fn battlenet_updating(&self) -> bool {
        false
    }
}

#[cfg(test)]
pub mod test_support {
    use super::Procs;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// A scripted fake. `on_start` runs whenever `start_battlenet` is called, letting a test simulate
    /// Battle.net logging in (writing a log line, rotating a token in the fake registry).
    pub struct FakeProcs {
        /// The running game, if any. `close_games` clears it unless `game_survives` is set.
        pub game: Mutex<Option<String>>,
        /// The game refuses to die (anti-cheat, a hung process): `close_games` leaves it running.
        pub game_survives: AtomicBool,
        /// How often `close_games` was called.
        pub games_closed: AtomicUsize,
        pub bn_running: AtomicBool,
        pub closed: AtomicUsize,
        pub started: AtomicUsize,
        pub on_start: Box<dyn Fn() + Send + Sync>,
        /// Is the login window showing (set by a test, or by `on_start`).
        pub login_window: AtomicBool,
        pub updating: AtomicBool,
        /// When > 0: the login window is visible for exactly this many more checks.
        pub login_window_polls: AtomicUsize,
        /// How often `focus_battlenet` was called.
        pub focused: AtomicUsize,
        /// When > 0: Battle.net "exits by itself" (the user closed the login window) after this many
        /// more `battlenet_running` checks while it is running.
        pub exit_after_checks: AtomicUsize,
    }

    impl FakeProcs {
        pub fn new() -> Self {
            FakeProcs {
                game: Mutex::new(None),
                game_survives: AtomicBool::new(false),
                games_closed: AtomicUsize::new(0),
                bn_running: AtomicBool::new(false),
                closed: AtomicUsize::new(0),
                started: AtomicUsize::new(0),
                on_start: Box::new(|| {}),
                login_window: AtomicBool::new(false),
                updating: AtomicBool::new(false),
                login_window_polls: AtomicUsize::new(0),
                focused: AtomicUsize::new(0),
                exit_after_checks: AtomicUsize::new(0),
            }
        }
        pub fn with_on_start(mut self, f: impl Fn() + Send + Sync + 'static) -> Self {
            self.on_start = Box::new(f);
            self
        }
        pub fn with_game(self, label: &str) -> Self {
            *self.game.lock().unwrap() = Some(label.to_string());
            self
        }
        pub fn into_arc(self) -> Arc<Self> {
            Arc::new(self)
        }
    }

    impl Procs for FakeProcs {
        fn running_game(&self) -> Option<String> {
            self.game.lock().unwrap().clone()
        }
        fn close_games(&self) -> Vec<String> {
            self.games_closed.fetch_add(1, Ordering::SeqCst);
            let mut g = self.game.lock().unwrap();
            if self.game_survives.load(Ordering::SeqCst) {
                return Vec::new();
            }
            g.take().into_iter().collect()
        }
        fn battlenet_running(&self, _dir: &str) -> bool {
            let running = self.bn_running.load(Ordering::SeqCst);
            if running {
                let left = self.exit_after_checks.load(Ordering::SeqCst);
                if left == 1 {
                    self.bn_running.store(false, Ordering::SeqCst);
                    self.login_window.store(false, Ordering::SeqCst);
                }
                if left > 0 {
                    self.exit_after_checks.store(left - 1, Ordering::SeqCst);
                }
            }
            running
        }
        fn close_battlenet(&self, _dir: &str) -> Result<(), String> {
            self.closed.fetch_add(1, Ordering::SeqCst);
            self.bn_running.store(false, Ordering::SeqCst);
            Ok(())
        }
        fn start_battlenet(&self, _exe: &str) -> Result<(), String> {
            self.started.fetch_add(1, Ordering::SeqCst);
            self.bn_running.store(true, Ordering::SeqCst);
            (self.on_start)();
            Ok(())
        }
        fn login_window_visible(&self, _dir: &str) -> bool {
            let polls = self.login_window_polls.load(Ordering::SeqCst);
            if polls > 0 {
                self.login_window_polls.store(polls - 1, Ordering::SeqCst);
                return true;
            }
            self.bn_running.load(Ordering::SeqCst) && self.login_window.load(Ordering::SeqCst)
        }
        fn focus_battlenet(&self, _dir: &str) -> bool {
            self.focused.fetch_add(1, Ordering::SeqCst);
            true
        }
        fn battlenet_updating(&self) -> bool {
            self.updating.load(Ordering::SeqCst)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_table_matches_exact_and_prefix() {
        assert_eq!(game_label("Overwatch.exe"), Some("Overwatch"));
        assert_eq!(game_label("Diablo IV.exe"), Some("Diablo IV"));
        assert_eq!(game_label("SC2_x64.exe"), Some("StarCraft II"));
        assert_eq!(game_label("HeroesOfTheStorm_x64.exe"), Some("Heroes of the Storm"));
        assert_eq!(game_label("Battle.net.exe"), None);
        assert_eq!(game_label("Agent.exe"), None);
        assert_eq!(game_label("explorer.exe"), None);
    }

    #[test]
    fn game_paths_are_labelled_by_their_exe() {
        assert_eq!(
            game_label_for_path(r"C:\Program Files (x86)\Overwatch\_retail_\Overwatch.exe"),
            Some("Overwatch")
        );
        assert_eq!(game_label_for_path(r"C:\Program Files (x86)\Battle.net\Battle.net.exe"), None);
    }

    #[test]
    fn login_window_shape_at_any_scaling() {
        // The measured login window, at 100%, 125%, 150% and 200% scaling (physical pixels).
        assert!(is_login_window_size(362, 663, 96));
        assert!(is_login_window_size(453, 829, 120));
        assert!(is_login_window_size(543, 995, 144));
        assert!(is_login_window_size(724, 1326, 192));
        // The main window (landscape, wide) and tiny helper windows are not the login window.
        assert!(!is_login_window_size(1280, 800, 96));
        assert!(!is_login_window_size(1920, 1200, 144));
        assert!(!is_login_window_size(300, 120, 96));
        assert!(!is_login_window_size(0, 0, 96));
        // A square-ish small window is not it either.
        assert!(!is_login_window_size(450, 470, 96));
        // DPI 0 (API failure) falls back to 100%.
        assert!(is_login_window_size(362, 663, 0));
    }

    #[test]
    fn update_process_names() {
        assert!(is_update_process("Battle.net-Setup.exe"));
        assert!(is_update_process("Battle.net Setup.exe"));
        assert!(is_update_process("Battle.net Update Helper.exe"));
        assert!(!is_update_process("Battle.net.exe"));
        assert!(!is_update_process("Battle.net Launcher.exe"));
        assert!(!is_update_process("Agent.exe"));
        assert!(!is_update_process("Overwatch Setup.exe"));
    }

    #[test]
    fn is_under_matches_whole_components() {
        assert!(is_under(r"C:\Program Files (x86)\Battle.net\Battle.net.exe", r"C:\Program Files (x86)\Battle.net"));
        assert!(!is_under(r"C:\Program Files (x86)\Battle.net2\Battle.net.exe", r"C:\Program Files (x86)\Battle.net"));
        assert!(is_under(r"\\?\C:/PROGRA~1/Battle.net/x.exe".to_lowercase().as_str(), r"c:\progra~1\battle.net"));
    }
}
