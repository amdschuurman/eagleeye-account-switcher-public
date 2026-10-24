//! Reading the signed-in account from Battle.net's logs. Battle.net writes, per session,
//! `Querying parental control info for account: 1:0:<code>` into `%LOCALAPPDATA%\Battle.net\Logs\
//! battle.net-<time>.log`. We read that line to learn which account is signed in, and when we watch
//! for a switch to finish we only consider log files modified after our own launch, so a stale line
//! from a previous session is never mistaken for success.

use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

/// The account code is 8 lowercase hex chars in the live data, but we accept any hex run to be safe.
fn account_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"Querying parental control info for account: 1:0:([0-9a-f]+)").unwrap())
}

/// The last account code mentioned in a log's text (the most recent sign-in in that file), if any.
pub fn last_code_in_text(text: &str) -> Option<String> {
    account_re()
        .captures_iter(text)
        .last()
        .map(|c| c[1].to_string())
}

/// Log hints that Battle.net is showing its login page instead of signing in. Best effort and
/// UNVERIFIED against a real login-page log (the 2026-10-23 live tests keyed on the window shape,
/// which is the primary signal; see `procs::is_login_window_size`). Kept narrow on purpose: a false
/// hit only shows "Log in once" early, and the needs-login wait still finishes the switch if the
/// sign-in line arrives after all.
fn login_hint_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(show(ing)? (the )?login (window|page|screen)|login (page|window|screen) (shown|displayed|opened)|re-?authentication required|authentication required)",
        )
        .unwrap()
    })
}

/// Log hints that Battle.net is updating ITSELF (not a game). Best effort and UNVERIFIED against a
/// real self-update log; only the tail of a log written in the last `UPDATE_RECENT_MS` counts, so an
/// old update that finished long ago never blocks a switch.
fn update_hint_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(self[- ]?updat(e|ing)|updating battle\.net|battle\.net (is )?updating)").unwrap())
}

/// How recent a log must be for its update hint to count.
pub const UPDATE_RECENT_MS: u64 = 120_000;
/// Only the end of a log is scanned for hints (the current state, not the session's history).
const TAIL_BYTES: usize = 16 * 1024;

fn tail(text: &str) -> &str {
    if text.len() <= TAIL_BYTES {
        return text;
    }
    let mut start = text.len() - TAIL_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// Does the newest log written after `since_ms` show the login page?
pub fn login_hint(files: &[LogFile], since_ms: u64) -> bool {
    files
        .iter()
        .filter(|f| f.modified_ms > since_ms)
        .max_by_key(|f| f.modified_ms)
        .is_some_and(|f| login_hint_re().is_match(tail(&f.text)))
}

/// Does any log modified within `UPDATE_RECENT_MS` of `now_ms` end with a self-update hint?
pub fn update_hint(files: &[LogFile], now_ms: u64) -> bool {
    files
        .iter()
        .filter(|f| f.modified_ms + UPDATE_RECENT_MS >= now_ms)
        .any(|f| update_hint_re().is_match(tail(&f.text)))
}

/// Agent logs (`%ProgramData%\Battle.net\Agent\**\Agent*.log`, one or two folders deep). A
/// missing folder is an empty list.
pub fn read_agent_logs(agent_dir: &Path) -> Vec<LogFile> {
    let mut out = Vec::new();
    let mut dirs = vec![(agent_dir.to_path_buf(), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth < 2 {
                    dirs.push((path, depth + 1));
                }
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if !(name.starts_with("agent") && name.ends_with(".log")) {
                continue;
            }
            out.push(read_one(&entry));
        }
    }
    out
}

fn read_one(entry: &std::fs::DirEntry) -> LogFile {
    let modified_ms = entry
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let text = std::fs::read(entry.path()).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    LogFile { modified_ms, text }
}

/// One log file's modification time (ms since epoch) and its text.
#[derive(Clone, Debug)]
pub struct LogFile {
    pub modified_ms: u64,
    pub text: String,
}

/// The signed-in account across a set of log files: the code from the most recently modified file
/// that has one. With `since_ms = Some(t)`, files modified at or before `t` are ignored (logs that
/// predate our launch do not count as a fresh sign-in).
pub fn signed_in_code(files: &[LogFile], since_ms: Option<u64>) -> Option<String> {
    let mut candidates: Vec<&LogFile> = files
        .iter()
        .filter(|f| since_ms.map_or(true, |t| f.modified_ms > t))
        .collect();
    // Newest first.
    candidates.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    for f in candidates {
        if let Some(code) = last_code_in_text(&f.text) {
            return Some(code);
        }
    }
    None
}

/// Is this a Battle.net session log by name (`battle.net-*.log`, case-insensitive)?
fn is_battlenet_log(name: &str) -> bool {
    let n = name.to_lowercase();
    n.starts_with("battle.net-") && n.ends_with(".log")
}

/// Read every Battle.net session log in `logs_dir` (name + mtime + text). A missing directory is an
/// empty list, not an error (Battle.net may never have run).
pub fn read_log_files(logs_dir: &Path) -> Result<Vec<LogFile>, String> {
    let entries = match std::fs::read_dir(logs_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("could not read the Battle.net log folder".into()),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_battlenet_log(&name) {
            continue;
        }
        // A log being written may hold invalid UTF-8 mid-line; read lossily.
        out.push(read_one(&entry));
    }
    Ok(out)
}

/// The signed-in code right now (newest log, no cutoff).
/// Who Battle.net's NEWEST session signed in as. Only that session counts: when it shows a login window
/// instead (an expired or rejected token), nobody is signed in, even if an older log names an account
/// (seen in testing: the older log made a switch to that account skip the token restore).
pub fn current_code(logs_dir: &Path) -> Result<Option<String>, String> {
    Ok(newest_session_code(&read_log_files(logs_dir)?))
}

fn newest_session_code(files: &[LogFile]) -> Option<String> {
    files.iter().max_by_key(|f| f.modified_ms).and_then(|f| last_code_in_text(&f.text))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "2026-10-23 10:00:00 Querying parental control info for account: 1:0:12ab34cd\n";

    #[test]
    fn current_code_is_nobody_when_the_newest_session_did_not_sign_in() {
        let old = LogFile { modified_ms: 1, text: LINE.into() };
        let failed = LogFile { modified_ms: 2, text: "Tassadar token rejected by BGS".into() };
        assert_eq!(newest_session_code(&[old.clone(), failed]), None);
        assert_eq!(newest_session_code(&[old]), Some("12ab34cd".to_string()));
    }

    #[test]
    fn parses_the_code_from_the_exact_line() {
        assert_eq!(last_code_in_text(LINE), Some("12ab34cd".to_string()));
        assert_eq!(last_code_in_text("nothing here"), None);
    }

    #[test]
    fn returns_the_last_code_when_a_file_has_several() {
        let text = format!(
            "{}\nnoise\n{}\n",
            "Querying parental control info for account: 1:0:aaaa0001",
            "Querying parental control info for account: 1:0:bbbb0002"
        );
        assert_eq!(last_code_in_text(&text), Some("bbbb0002".to_string()));
    }

    #[test]
    fn old_logs_are_ignored_with_a_cutoff() {
        let files = vec![
            // Old log, predates our launch, has a sign-in line for an old account.
            LogFile { modified_ms: 1000, text: "account: 1:0:oldacct0".into() },
            // A log written just before launch: still too old.
            LogFile {
                modified_ms: 4999,
                text: "Querying parental control info for account: 1:0:5a1e0000".into(),
            },
        ];
        // Launch at 5000: nothing newer, so no success yet.
        assert_eq!(signed_in_code(&files, Some(5000)), None);

        // A fresh log appears after launch with the account we switched to.
        let mut after = files.clone();
        after.push(LogFile {
            modified_ms: 6000,
            text: "Querying parental control info for account: 1:0:aeca0001".into(),
        });
        assert_eq!(signed_in_code(&after, Some(5000)), Some("aeca0001".to_string()));
    }

    #[test]
    fn without_a_cutoff_the_newest_file_wins() {
        let files = vec![
            LogFile { modified_ms: 100, text: "Querying parental control info for account: 1:0:0d0e0000".into() },
            LogFile { modified_ms: 200, text: "Querying parental control info for account: 1:0:aea00e00".into() },
        ];
        assert_eq!(signed_in_code(&files, None), Some("aea00e00".to_string()));
    }

    #[test]
    fn login_hint_only_in_a_log_after_launch() {
        let files = vec![
            LogFile { modified_ms: 100, text: "Showing login window".into() },
            LogFile { modified_ms: 900, text: "starting up\n".into() },
        ];
        // The old log shows the login page but predates the launch; the newest after-launch log does not.
        assert!(!login_hint(&files, 500));
        let mut after = files.clone();
        after.push(LogFile { modified_ms: 1000, text: "INFO Authentication required for account".into() });
        assert!(login_hint(&after, 500));
        assert!(!login_hint(&[], 0));
    }

    #[test]
    fn update_hint_needs_a_recent_log() {
        let now = 1_000_000;
        let old = LogFile { modified_ms: now - UPDATE_RECENT_MS - 1, text: "Starting self-update".into() };
        assert!(!update_hint(&[old], now), "a finished update long ago does not block");
        let fresh = LogFile { modified_ms: now - 1000, text: "noise\nUpdating Battle.net to 2.40\n".into() };
        assert!(update_hint(&[fresh], now));
        let game = LogFile { modified_ms: now, text: "Updating Overwatch: 42%".into() };
        assert!(!update_hint(&[game], now), "a game update is not a Battle.net self-update");
    }

    #[test]
    fn hints_only_read_the_tail() {
        let mut text = "Showing login window\n".to_string();
        text.push_str(&"x".repeat(TAIL_BYTES + 10));
        let files = vec![LogFile { modified_ms: 10, text }];
        assert!(!login_hint(&files, 0), "a hint that scrolled out of the tail is history");
    }

    #[test]
    fn agent_logs_are_found_a_level_deep() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("Agent.9999").join("Logs");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("Agent-20261023.log"), "self-update started").unwrap();
        std::fs::write(sub.join("other.txt"), "self-update started").unwrap();
        let logs = read_agent_logs(dir.path());
        assert_eq!(logs.len(), 1);
        assert!(read_agent_logs(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn log_name_filter() {
        assert!(is_battlenet_log("Battle.net-1234.log"));
        assert!(is_battlenet_log("battle.net-2026-10-23.log"));
        assert!(!is_battlenet_log("Agent-1.log"));
        assert!(!is_battlenet_log("battle.net.config"));
    }

    #[test]
    fn reads_real_files_and_applies_the_cutoff_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("battle.net-1.log");
        std::fs::write(&p, "Querying parental control info for account: 1:0:deadbeef").unwrap();
        // No cutoff: found.
        assert_eq!(current_code(dir.path()).unwrap(), Some("deadbeef".to_string()));
        // Cutoff far in the future: the file is older, so ignored.
        let future = u64::MAX;
        assert_eq!(signed_in_code(&read_log_files(dir.path()).unwrap(), Some(future)), None);
    }
}
