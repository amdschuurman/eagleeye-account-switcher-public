//! The real Blizzard rank and role icons, fetched once into the store's `icons` folder:
//! `tier_<tier>.png` (bronze ... champion, emerald included), `div_<1-5>.png` and
//! `role_<tank|damage|support|open>.svg`. Manual and predicted ranks use the same art offline.
//!
//! The URLs live in `resources/ow-icons.json` (URLs only, compiled in; the images are Blizzard
//! artwork and are never committed). They were harvested from public career pages; when Blizzard
//! rotates the content hashes, refresh that file. A file already on disk is never fetched again.

use crate::overfast::Http;
use crate::ranks::{Role, Tier};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MANIFEST: &str = include_str!("../resources/ow-icons.json");

#[derive(Deserialize)]
struct Manifest {
    tiers: BTreeMap<String, String>,
    divisions: BTreeMap<String, String>,
    roles: BTreeMap<String, String>,
}

/// One icon to have on disk: local file name and source URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconSpec {
    pub file: String,
    pub url: String,
}

fn role_name(r: Role) -> &'static str {
    match r {
        Role::Tank => "tank",
        Role::Damage => "damage",
        Role::Support => "support",
        Role::Open => "open",
    }
}

/// Every icon the UI uses, from the compiled-in manifest. Errors name any entry that is missing.
pub fn specs() -> Result<Vec<IconSpec>, String> {
    let m: Manifest = serde_json::from_str(MANIFEST).map_err(|e| format!("ow-icons.json is invalid: {e}"))?;
    let mut out = Vec::new();
    let mut missing = Vec::new();
    for t in Tier::ALL {
        match m.tiers.get(t.as_str()) {
            Some(u) => out.push(IconSpec { file: format!("tier_{}.png", t.as_str()), url: u.clone() }),
            None => missing.push(format!("tier {}", t.as_str())),
        }
    }
    for d in 1..=5 {
        match m.divisions.get(&d.to_string()) {
            Some(u) => out.push(IconSpec { file: format!("div_{d}.png"), url: u.clone() }),
            None => missing.push(format!("division {d}")),
        }
    }
    for r in Role::ALL {
        match m.roles.get(role_name(r)) {
            Some(u) => out.push(IconSpec { file: format!("role_{}.svg", role_name(r)), url: u.clone() }),
            None => missing.push(format!("role {}", role_name(r))),
        }
    }
    if missing.is_empty() {
        Ok(out)
    } else {
        Err(format!("ow-icons.json lacks {}", missing.join(", ")))
    }
}

/// Is this body the image type the file name promises? (Guards against an HTML error page.)
fn looks_right(file: &str, body: &[u8]) -> bool {
    if file.ends_with(".png") {
        body.starts_with(b"\x89PNG\r\n\x1a\n")
    } else if file.ends_with(".svg") {
        let head = String::from_utf8_lossy(&body[..body.len().min(512)]).to_ascii_lowercase();
        head.contains("<svg")
    } else {
        false
    }
}

/// Result of `ensure`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IconReport {
    pub present: usize,
    pub fetched: usize,
    pub failed: Vec<String>,
}

/// Download every icon not yet on disk into `dir` (atomic writes). Never re-fetches a present file.
pub fn ensure(dir: &Path, http: &dyn Http, specs: &[IconSpec]) -> IconReport {
    let mut rep = IconReport::default();
    for s in specs {
        let target = dir.join(&s.file);
        if target.is_file() {
            rep.present += 1;
            continue;
        }
        // `#icon` is a fragment for inline use; the file itself is the whole SVG.
        let url = s.url.split('#').next().unwrap_or(&s.url);
        match http.get(url) {
            Ok((200, body)) if looks_right(&s.file, &body) => match crate::atomic::write(&target, &body) {
                Ok(()) => {
                    rep.fetched += 1;
                    rep.present += 1;
                }
                Err(_) => rep.failed.push(s.file.clone()),
            },
            _ => rep.failed.push(s.file.clone()),
        }
    }
    rep
}

/// Are all icons on disk?
pub fn complete(dir: &Path, specs: &[IconSpec]) -> bool {
    specs.iter().all(|s| dir.join(&s.file).is_file())
}

/// The icon folder as a string for the UI.
pub fn dir_string(dir: &Path) -> String {
    PathBuf::from(dir).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overfast::test_support::FakeHttp;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest";
    const SVG: &[u8] = b"<?xml version=\"1.0\"?>\n<svg viewBox=\"0 0 40 40\"><g id='icon'/></svg>";

    #[test]
    fn manifest_has_every_tier_division_and_role() {
        let s = specs().unwrap();
        assert_eq!(s.len(), 9 + 5 + 4);
        assert!(s.iter().any(|x| x.file == "tier_champion.png" && x.url.contains("Rank_UltimateTier")));
        assert!(s.iter().any(|x| x.file == "tier_emerald.png"));
        assert!(s.iter().any(|x| x.file == "role_damage.svg" && x.url.contains("/role/offense.")));
        assert!(s.iter().any(|x| x.file == "role_open.svg"));
        for x in &s {
            assert!(x.url.starts_with("https://static.playoverwatch.com/images/pages/career/icons/"), "{}", x.url);
        }
    }

    #[test]
    fn ensure_fetches_once_and_rejects_wrong_bodies() {
        let dir = tempfile::tempdir().unwrap();
        let specs = vec![
            IconSpec { file: "tier_gold.png".into(), url: "https://x/gold.png".into() },
            IconSpec { file: "role_tank.svg".into(), url: "https://x/tank.svg#icon".into() },
            IconSpec { file: "div_1.png".into(), url: "https://x/html.png".into() },
            IconSpec { file: "div_2.png".into(), url: "https://x/missing.png".into() },
        ];
        let http = FakeHttp::default()
            .with("https://x/gold.png", 200, PNG)
            .with("https://x/tank.svg", 200, SVG)
            .with("https://x/html.png", 200, b"<html>error</html>")
            .with("https://x/missing.png", 404, b"");
        let rep = ensure(dir.path(), &http, &specs);
        assert_eq!(rep.fetched, 2);
        assert_eq!(rep.failed, vec!["div_1.png", "div_2.png"]);
        assert_eq!(std::fs::read(dir.path().join("tier_gold.png")).unwrap(), PNG);
        assert!(!dir.path().join("div_1.png").exists());
        assert!(!complete(dir.path(), &specs));
        assert!(complete(dir.path(), &specs[..2]));

        let calls = http.calls();
        let rep = ensure(dir.path(), &http, &specs[..2]);
        assert_eq!((rep.present, rep.fetched), (2, 0));
        assert_eq!(http.calls(), calls, "present files are never fetched again");
    }
}
