//! OverFast API client (`TeKrop/overfast-api`, MIT): the public Overwatch career page as JSON.
//!
//! `GET {base}/players/{Name-1234}/summary` gives `competitive.pc.{tank,damage,support,open}` as
//! `{division: "silver", tier: 4, ...}` or `null`, plus `competitive.pc.season`. OverFast names the
//! Champion tier `ultimate` (Blizzard's icon is `Rank_UltimateTier`); we call it `champion`.
//!
//! Private vs unranked: both give `competitive: null` on the summary. A private career also has no
//! stats at all, while a public one that played anything has `general` stats, so only when
//! `competitive` is null we ask `{base}/players/{id}/stats/summary` once: `general` null or missing
//! means private.
//!
//! 404 on the summary does not prove the player is missing: OverFast caches "unknown player" with a
//! backoff (about 10 minutes, then about an hour), so a profile made public minutes ago keeps
//! answering 404 by BattleTag. The search (`{base}/players?name=Name-1234`) updates sooner and gives
//! the Blizzard ID; `{base}/players/{blizzard_id}/summary` bypasses the cached 404. Only an exact
//! `player_id` match counts: the search also lists other players with the same name.
//!
//! The summary's `avatar` (the player icon URL, may be null) rides along on `Found`. The search hit carries an
//! `avatar` too; the 404 fallback uses it when the summary by Blizzard ID has none, and a private hit keeps it.
//!
//! At most three requests per call (summary, search, summary by Blizzard ID), 5 s timeout, no
//! retries: the rank cache decides when to ask again.

use crate::ranks::{Tier, TierDiv};
use serde_json::Value;
use std::time::Duration;

/// Minimal blocking GET, behind a trait so tests run on fixtures.
pub trait Http: Send + Sync {
    /// Status code and body. `Err` only for transport failures (DNS, timeout, TLS).
    fn get(&self, url: &str) -> Result<(u16, Vec<u8>), String>;

    /// A GET for untrusted image URLs: no redirects, and the body is cut off past `max` bytes (`Err`).
    /// The default reads everything and checks after; the real client stops reading early.
    fn get_capped(&self, url: &str, max: usize) -> Result<(u16, Vec<u8>), String> {
        let (status, body) = self.get(url)?;
        if body.len() > max {
            return Err("the response is too large".into());
        }
        Ok((status, body))
    }
}

/// The real client: reqwest over rustls, 5 s timeout.
pub struct ReqwestHttp {
    client: reqwest::blocking::Client,
    /// For `get_capped`: never follows a redirect, so a URL checked against the allowlist is the only host asked.
    no_redirect: reqwest::blocking::Client,
}

impl ReqwestHttp {
    pub fn new() -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(5))
            .user_agent(concat!("EagleEye-Account-Switcher/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| "could not create the HTTP client".to_string())?;
        let no_redirect = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("EagleEye-Account-Switcher/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| "could not create the HTTP client".to_string())?;
        Ok(ReqwestHttp { client, no_redirect })
    }
}

impl Http for ReqwestHttp {
    fn get(&self, url: &str) -> Result<(u16, Vec<u8>), String> {
        let resp = self.client.get(url).send().map_err(|e| {
            if e.is_timeout() {
                "OverFast did not answer within 5 s".to_string()
            } else {
                "could not reach the rank service".to_string()
            }
        })?;
        let status = resp.status().as_u16();
        let body = resp.bytes().map_err(|_| "rank service response was cut off".to_string())?;
        Ok((status, body.to_vec()))
    }

    fn get_capped(&self, url: &str, max: usize) -> Result<(u16, Vec<u8>), String> {
        use std::io::Read;
        let resp = self.no_redirect.get(url).send().map_err(|_| "could not reach the image host".to_string())?;
        let status = resp.status().as_u16();
        if resp.content_length().is_some_and(|n| n > max as u64) {
            return Err("the response is too large".into());
        }
        let mut body = Vec::new();
        resp.take(max as u64 + 1).read_to_end(&mut body).map_err(|_| "image response was cut off".to_string())?;
        if body.len() > max {
            return Err("the response is too large".into());
        }
        Ok((status, body))
    }
}

/// The four competitive roles from the PC profile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfileRoles {
    pub tank: Option<TierDiv>,
    pub damage: Option<TierDiv>,
    pub support: Option<TierDiv>,
    pub open: Option<TierDiv>,
}

/// What one lookup found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// The profile was read. `private_profile` true means no data could be shown at all.
    Found { season: Option<u32>, roles: ProfileRoles, private_profile: bool, avatar_url: Option<String> },
    /// OverFast does not know this BattleTag (yet). `recheck_at` (ms) is when OverFast says it will
    /// look again, from the 404 body.
    NotFound { recheck_at: Option<u64> },
}

/// Build `{base}/players/{id}/{tail...}` with the player id percent-encoded as one path segment.
pub fn player_url(base: &str, player_id: &str, tail: &[&str]) -> Result<String, String> {
    let mut url = reqwest::Url::parse(base.trim()).map_err(|_| "the OverFast URL setting is not a valid URL".to_string())?;
    {
        let mut segs = url
            .path_segments_mut()
            .map_err(|_| "the OverFast URL setting is not a valid base URL".to_string())?;
        segs.pop_if_empty();
        segs.push("players").push(player_id);
        for t in tail {
            segs.push(t);
        }
    }
    Ok(url.into())
}

/// `{base}/players?name={player_id}`, the search.
pub fn search_url(base: &str, player_id: &str) -> Result<String, String> {
    let mut url = reqwest::Url::parse(base.trim()).map_err(|_| "the OverFast URL setting is not a valid URL".to_string())?;
    url.path_segments_mut()
        .map_err(|_| "the OverFast URL setting is not a valid base URL".to_string())?
        .pop_if_empty()
        .push("players");
    url.query_pairs_mut().append_pair("name", player_id);
    Ok(url.into())
}

/// When OverFast checks an unknown player again (ms), from a 404 body: `next_check_at` (unix
/// seconds) at the top level (cached answer) or inside `error` (fresh answer).
pub fn recheck_at(body: &[u8]) -> Option<u64> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let at = v.get("next_check_at").or_else(|| v.get("error").and_then(|e| e.get("next_check_at")))?;
    at.as_u64().or_else(|| at.as_f64().filter(|f| *f > 0.0).map(|f| f as u64)).map(|s| s * 1000)
}

/// The exact search hit for one player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    /// `|` decoded; may be empty for a private hit.
    pub blizzard_id: String,
    pub is_public: bool,
    pub avatar_url: Option<String>,
}

/// A non-empty string field.
fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Search for `player_id` (`Name-1234`) and return its Blizzard ID, whether the profile is public and
/// its avatar. Only an exact `player_id` match counts; anything else (no match, transport error, odd
/// data) is `None`.
pub fn search_exact(http: &dyn Http, base: &str, player_id: &str) -> Option<SearchHit> {
    let (status, body) = http.get(&search_url(base, player_id).ok()?).ok()?;
    if status != 200 {
        return None;
    }
    let v: Value = serde_json::from_slice(&body).ok()?;
    let hit = v.get("results")?.as_array()?.iter().find(|r| r.get("player_id").and_then(Value::as_str) == Some(player_id))?;
    let is_public = hit.get("is_public").and_then(Value::as_bool).unwrap_or(true);
    let blizzard_id = hit.get("blizzard_id").and_then(Value::as_str).unwrap_or("").replace("%7C", "|").replace("%7c", "|");
    if is_public && blizzard_id.is_empty() {
        return None;
    }
    Some(SearchHit { blizzard_id, is_public, avatar_url: text(hit, "avatar") })
}

/// OverFast's division name to our tier (`ultimate` and `champion` both mean Champion).
pub fn tier_from_overfast(name: &str) -> Option<Tier> {
    match name.trim().to_ascii_lowercase().as_str() {
        "ultimate" | "champion" => Some(Tier::Champion),
        other => Tier::parse(other),
    }
}

fn role_from(v: &Value) -> Option<TierDiv> {
    let tier = tier_from_overfast(v.get("division")?.as_str()?)?;
    let division = v.get("tier")?.as_u64()?;
    (1..=5).contains(&division).then_some(TierDiv { tier, division: division as u8 })
}

/// Parsed summary: `None` for the competitive block means `competitive` (or its `pc` part) is null.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub competitive: Option<(Option<u32>, ProfileRoles)>,
    /// The player icon URL (`avatar`), when the profile has one.
    pub avatar: Option<String>,
}

/// Parse a `/players/{id}/summary` body.
pub fn parse_summary(body: &[u8]) -> Result<Summary, String> {
    let v: Value = serde_json::from_slice(body).map_err(|_| "rank service sent unreadable data".to_string())?;
    if !v.is_object() {
        return Err("rank service sent unexpected data".into());
    }
    let avatar = text(&v, "avatar");
    let pc = v.get("competitive").and_then(|c| c.get("pc")).filter(|p| p.is_object());
    let Some(pc) = pc else { return Ok(Summary { competitive: None, avatar }) };
    let season = pc.get("season").and_then(Value::as_u64).map(|s| s as u32);
    let r = |k: &str| pc.get(k).and_then(role_from);
    let roles = ProfileRoles { tank: r("tank"), damage: r("damage"), support: r("support"), open: r("open") };
    Ok(Summary { competitive: Some((season, roles)), avatar })
}

/// Does a `/players/{id}/stats/summary` body carry any stats? (No = private career.)
pub fn stats_have_data(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("general").cloned())
        .map_or(false, |g| g.as_object().map_or(false, |o| !o.is_empty()))
}

fn status_error(status: u16) -> String {
    match status {
        429 => "the rank service is rate limiting, it will be retried later".into(),
        500..=599 => format!("the rank service is having trouble ({status})"),
        s => format!("the rank service answered {s}"),
    }
}

/// Look up one BattleTag (`Name#1234`).
pub fn lookup(http: &dyn Http, base: &str, battletag: &str) -> Result<Lookup, String> {
    let id = crate::battletag::to_player_id(battletag).ok_or("not a BattleTag")?;
    let (status, body) = http.get(&player_url(base, &id, &["summary"])?)?;
    // `key` is the id the profile answered to: the BattleTag, or the Blizzard ID after a cached 404.
    // `search_avatar` is the search hit's avatar, used when the summary itself has none.
    let (key, body, search_avatar) = match status {
        200 => (id, body, None),
        404 => {
            let recheck_at = recheck_at(&body);
            match search_exact(http, base, &id) {
                Some(SearchHit { is_public: false, avatar_url, .. }) => {
                    return Ok(Lookup::Found { season: None, roles: ProfileRoles::default(), private_profile: true, avatar_url })
                }
                Some(SearchHit { blizzard_id, avatar_url, .. }) => match http.get(&player_url(base, &blizzard_id, &["summary"])?) {
                    Ok((200, b)) => (blizzard_id, b, avatar_url),
                    _ => return Ok(Lookup::NotFound { recheck_at }),
                },
                None => return Ok(Lookup::NotFound { recheck_at }),
            }
        }
        s => return Err(status_error(s)),
    };
    let summary = parse_summary(&body)?;
    let avatar_url = summary.avatar.or(search_avatar);
    if let Some((season, roles)) = summary.competitive {
        return Ok(Lookup::Found { season, roles, private_profile: false, avatar_url });
    }
    // No competitive block: private, or public without competitive play. One extra request decides.
    let private_profile = match http.get(&player_url(base, &key, &["stats", "summary"])?) {
        Ok((200, b)) => !stats_have_data(&b),
        // Unknown: show "Unranked" rather than claim the profile is private.
        _ => false,
    };
    Ok(Lookup::Found { season: None, roles: ProfileRoles::default(), private_profile, avatar_url })
}

#[cfg(test)]
pub mod test_support {
    use super::Http;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Canned responses by URL; records every request.
    #[derive(Default)]
    pub struct FakeHttp {
        pub routes: HashMap<String, (u16, Vec<u8>)>,
        pub seen: Mutex<Vec<String>>,
    }

    impl FakeHttp {
        pub fn with(mut self, url: &str, status: u16, body: &[u8]) -> Self {
            self.routes.insert(url.to_string(), (status, body.to_vec()));
            self
        }
        pub fn calls(&self) -> usize {
            self.seen.lock().unwrap().len()
        }
    }

    impl Http for FakeHttp {
        fn get(&self, url: &str) -> Result<(u16, Vec<u8>), String> {
            self.seen.lock().unwrap().push(url.to_string());
            self.routes.get(url).cloned().ok_or_else(|| "could not reach the rank service".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::FakeHttp;
    use super::*;

    const TEKROP: &[u8] = include_bytes!("fixtures/overfast_summary_tekrop.json");
    const TEKROP_STATS: &[u8] = include_bytes!("fixtures/overfast_stats_summary_tekrop.json");
    const BASE: &str = "https://overfast-api.tekrop.fr";

    #[test]
    fn urls_encode_the_player_id() {
        assert_eq!(
            player_url(BASE, "TeKrop-2217", &["summary"]).unwrap(),
            "https://overfast-api.tekrop.fr/players/TeKrop-2217/summary"
        );
        assert_eq!(
            player_url("http://overfast.invalid:8080/api/", "Örsted-1", &["stats", "summary"]).unwrap(),
            "http://overfast.invalid:8080/api/players/%C3%96rsted-1/stats/summary"
        );
        assert!(player_url("not a url", "x-1", &[]).is_err());
        // A Blizzard ID keeps its `|` (OverFast answers to it raw or as %7C).
        assert_eq!(player_url(BASE, "abc|def", &["summary"]).unwrap(), format!("{BASE}/players/abc|def/summary"));
        assert_eq!(search_url(BASE, "Kestrel-2187").unwrap(), format!("{BASE}/players?name=Kestrel-2187"));
        assert_eq!(search_url("http://overfast.invalid:8080/api/", "Kestrel-2187").unwrap(), "http://overfast.invalid:8080/api/players?name=Kestrel-2187");
    }

    #[test]
    fn recheck_at_flat_and_nested() {
        let flat = br#"{"error":"Player not found","retry_after":4040,"next_check_at":1700000000,"check_count":3}"#;
        let nested = br#"{"error":{"error":"Player not found","retry_after":600,"next_check_at":1700000600}}"#;
        assert_eq!(recheck_at(flat), Some(1_700_000_000_000));
        assert_eq!(recheck_at(nested), Some(1_700_000_600_000));
        assert_eq!(recheck_at(br#"{"error":"Player not found"}"#), None);
        assert_eq!(recheck_at(b"<html>"), None);
    }

    const NOT_FOUND: &[u8] = br#"{"error":"Player not found","retry_after":4040,"next_check_at":1700000000,"check_count":3}"#;

    #[test]
    fn a_cached_404_falls_back_to_the_blizzard_id() {
        let search = br#"{"total":2,"results":[
            {"player_id":"abc%7Cxyz","name":"Kestrel","is_public":true,"blizzard_id":"abc%7Cxyz"},
            {"player_id":"Kestrel-2187","name":"Kestrel","is_public":true,"blizzard_id":"abc%7Cdef"}]}"#;
        let http = FakeHttp::default()
            .with(&format!("{BASE}/players/Kestrel-2187/summary"), 404, NOT_FOUND)
            .with(&format!("{BASE}/players?name=Kestrel-2187"), 200, search)
            .with(&format!("{BASE}/players/abc|def/summary"), 200, TEKROP);
        match lookup(&http, BASE, "Kestrel#2187").unwrap() {
            Lookup::Found { season, roles, private_profile, .. } => {
                assert_eq!(season, Some(22));
                assert!(roles.support.is_some());
                assert!(!private_profile);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(http.calls(), 3);
    }

    #[test]
    fn the_stats_check_uses_the_blizzard_id_too() {
        let search = br#"{"results":[{"player_id":"Kestrel-2187","is_public":true,"blizzard_id":"abc%7Cdef"}]}"#;
        let http = FakeHttp::default()
            .with(&format!("{BASE}/players/Kestrel-2187/summary"), 404, NOT_FOUND)
            .with(&format!("{BASE}/players?name=Kestrel-2187"), 200, search)
            .with(&format!("{BASE}/players/abc|def/summary"), 200, br#"{"competitive":null}"#)
            .with(&format!("{BASE}/players/abc|def/stats/summary"), 200, TEKROP_STATS);
        assert!(matches!(lookup(&http, BASE, "Kestrel#2187").unwrap(), Lookup::Found { private_profile: false, .. }));
        assert_eq!(http.seen.lock().unwrap().last().unwrap(), &format!("{BASE}/players/abc|def/stats/summary"));
    }

    #[test]
    fn a_name_only_search_match_is_not_used() {
        // Same name, other players: no exact player_id, so the BattleTag stays not found.
        let search = br#"{"results":[{"player_id":"abc%7Cxyz","name":"Kestrel","is_public":true,"blizzard_id":"abc%7Cxyz"},
            {"player_id":"Kestrel-9999","name":"Kestrel","is_public":true,"blizzard_id":"abc%7Cuvw"}]}"#;
        let http = FakeHttp::default()
            .with(&format!("{BASE}/players/Kestrel-2187/summary"), 404, NOT_FOUND)
            .with(&format!("{BASE}/players?name=Kestrel-2187"), 200, search);
        assert_eq!(lookup(&http, BASE, "Kestrel#2187").unwrap(), Lookup::NotFound { recheck_at: Some(1_700_000_000_000) });
        assert_eq!(http.calls(), 2);
        assert_eq!(search_exact(&http, BASE, "Kestrel-2187"), None);
    }

    #[test]
    fn a_private_search_hit_is_a_private_profile() {
        let search = br#"{"results":[{"player_id":"Kestrel-2187","is_public":false,"blizzard_id":"abc%7Cdef"}]}"#;
        let http = FakeHttp::default()
            .with(&format!("{BASE}/players/Kestrel-2187/summary"), 404, NOT_FOUND)
            .with(&format!("{BASE}/players?name=Kestrel-2187"), 200, search);
        assert_eq!(
            search_exact(&http, BASE, "Kestrel-2187"),
            Some(SearchHit { blizzard_id: "abc|def".to_string(), is_public: false, avatar_url: None })
        );
        match lookup(&http, BASE, "Kestrel#2187").unwrap() {
            Lookup::Found { season, roles, private_profile, .. } => {
                assert!(private_profile);
                assert_eq!(season, None);
                assert_eq!(roles, ProfileRoles::default());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn a_failed_summary_by_blizzard_id_is_still_not_found() {
        let search = br#"{"results":[{"player_id":"Kestrel-2187","is_public":true,"blizzard_id":"abc%7Cdef"}]}"#;
        let http = FakeHttp::default()
            .with(&format!("{BASE}/players/Kestrel-2187/summary"), 404, NOT_FOUND)
            .with(&format!("{BASE}/players?name=Kestrel-2187"), 200, search)
            .with(&format!("{BASE}/players/abc|def/summary"), 404, b"{}");
        assert_eq!(lookup(&http, BASE, "Kestrel#2187").unwrap(), Lookup::NotFound { recheck_at: Some(1_700_000_000_000) });
    }

    #[test]
    fn parses_the_real_tekrop_summary() {
        let s = parse_summary(TEKROP).unwrap();
        let (season, roles) = s.competitive.unwrap();
        assert_eq!(season, Some(22));
        assert_eq!(roles.support, Some(TierDiv { tier: Tier::Silver, division: 4 }));
        assert_eq!(roles.tank, None);
        assert_eq!(roles.damage, None);
        assert_eq!(roles.open, None);
    }

    const SEARCH_AVATAR: &str = "https://d15f34w2p8l1cc.cloudfront.net/overwatch/aaaa000000000000000000000000000000000000000000000000000000000001.png";

    #[test]
    fn summary_avatar_is_read_and_a_null_one_is_none() {
        let s = parse_summary(TEKROP).unwrap();
        let avatar = s.avatar.unwrap();
        assert!(avatar.starts_with("https://") && avatar.ends_with(".png"), "{avatar}");
        assert!(crate::avatars::file_name(&avatar).is_some(), "the real avatar URL shape is accepted");
        assert_eq!(parse_summary(br#"{"avatar":null,"competitive":null}"#).unwrap().avatar, None);
        assert_eq!(parse_summary(br#"{"avatar":"","competitive":null}"#).unwrap().avatar, None);
        assert_eq!(parse_summary(br#"{"competitive":null}"#).unwrap().avatar, None);
        let http = FakeHttp::default()
            .with(&format!("{BASE}/players/Plain-1111/summary"), 200, br#"{"avatar":null,"competitive":null}"#)
            .with(&format!("{BASE}/players/Plain-1111/stats/summary"), 200, TEKROP_STATS);
        assert!(matches!(lookup(&http, BASE, "Plain#1111").unwrap(), Lookup::Found { avatar_url: None, private_profile: false, .. }));
        let http = FakeHttp::default().with(&format!("{BASE}/players/TeKrop-2217/summary"), 200, TEKROP);
        assert!(matches!(lookup(&http, BASE, "TeKrop#2217").unwrap(), Lookup::Found { avatar_url: Some(_), .. }));
    }

    #[test]
    fn the_404_fallback_carries_the_search_avatar() {
        let search = format!(r#"{{"results":[{{"player_id":"Kestrel-2187","is_public":true,"blizzard_id":"abc%7Cdef","avatar":"{SEARCH_AVATAR}"}}]}}"#);
        // The summary by Blizzard ID has no avatar: the search one is used.
        let http = FakeHttp::default()
            .with(&format!("{BASE}/players/Kestrel-2187/summary"), 404, NOT_FOUND)
            .with(&format!("{BASE}/players?name=Kestrel-2187"), 200, search.as_bytes())
            .with(&format!("{BASE}/players/abc|def/summary"), 200, br#"{"avatar":null,"competitive":{"pc":{"tank":{"division":"gold","tier":2},"season":22}}}"#);
        match lookup(&http, BASE, "Kestrel#2187").unwrap() {
            Lookup::Found { avatar_url, season, .. } => {
                assert_eq!(avatar_url.as_deref(), Some(SEARCH_AVATAR));
                assert_eq!(season, Some(22));
            }
            other => panic!("unexpected {other:?}"),
        }
        // The summary's own avatar wins over the search one.
        let http = FakeHttp::default()
            .with(&format!("{BASE}/players/Kestrel-2187/summary"), 404, NOT_FOUND)
            .with(&format!("{BASE}/players?name=Kestrel-2187"), 200, search.as_bytes())
            .with(&format!("{BASE}/players/abc|def/summary"), 200, TEKROP);
        let Lookup::Found { avatar_url, .. } = lookup(&http, BASE, "Kestrel#2187").unwrap() else { panic!("not found") };
        assert_ne!(avatar_url.as_deref(), Some(SEARCH_AVATAR));
        assert!(avatar_url.is_some());
    }

    #[test]
    fn a_private_search_hit_keeps_its_avatar() {
        let search = format!(r#"{{"results":[{{"player_id":"Kestrel-2187","is_public":false,"blizzard_id":"","avatar":"{SEARCH_AVATAR}"}}]}}"#);
        let http = FakeHttp::default()
            .with(&format!("{BASE}/players/Kestrel-2187/summary"), 404, NOT_FOUND)
            .with(&format!("{BASE}/players?name=Kestrel-2187"), 200, search.as_bytes());
        match lookup(&http, BASE, "Kestrel#2187").unwrap() {
            Lookup::Found { private_profile, avatar_url, .. } => {
                assert!(private_profile);
                assert_eq!(avatar_url.as_deref(), Some(SEARCH_AVATAR));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(http.calls(), 2);
    }

    #[test]
    fn ultimate_is_champion_and_unknown_tiers_are_dropped() {
        let body = br#"{"competitive":{"pc":{"tank":{"division":"ultimate","tier":1},
            "damage":{"division":"emerald","tier":3},"support":{"division":"mythic","tier":2},
            "open":{"division":"gold","tier":9},"season":23},"console":null}}"#;
        let (season, roles) = parse_summary(body).unwrap().competitive.unwrap();
        assert_eq!(season, Some(23));
        assert_eq!(roles.tank, Some(TierDiv { tier: Tier::Champion, division: 1 }));
        assert_eq!(roles.damage, Some(TierDiv { tier: Tier::Emerald, division: 3 }));
        assert_eq!(roles.support, None, "unknown tier name");
        assert_eq!(roles.open, None, "division out of 1..5");
    }

    #[test]
    fn null_competitive_and_console_only_have_no_pc_block() {
        assert_eq!(parse_summary(br#"{"competitive":null}"#).unwrap().competitive, None);
        assert_eq!(parse_summary(br#"{"competitive":{"pc":null,"console":{}}}"#).unwrap().competitive, None);
        assert!(parse_summary(b"<html>").is_err());
    }

    #[test]
    fn lookup_found() {
        let http = FakeHttp::default().with(&format!("{BASE}/players/TeKrop-2217/summary"), 200, TEKROP);
        match lookup(&http, BASE, "TeKrop#2217").unwrap() {
            Lookup::Found { season, roles, private_profile, .. } => {
                assert_eq!(season, Some(22));
                assert!(roles.support.is_some());
                assert!(!private_profile);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(http.calls(), 1, "no stats request when competitive data exists");
    }

    #[test]
    fn lookup_not_found_and_errors() {
        let http = FakeHttp::default()
            .with(&format!("{BASE}/players/Nobody-1234/summary"), 404, br#"{"error":"Player not found"}"#)
            .with(&format!("{BASE}/players/Busy-1234/summary"), 429, b"{}");
        assert_eq!(lookup(&http, BASE, "Nobody#1234").unwrap(), Lookup::NotFound { recheck_at: None });
        assert!(lookup(&http, BASE, "Busy#1234").unwrap_err().contains("rate limiting"));
        assert!(lookup(&http, BASE, "Offline#1234").is_err(), "transport error");
        assert!(lookup(&http, BASE, "no-hash").is_err());
    }

    #[test]
    fn private_vs_unranked() {
        let http = FakeHttp::default()
            .with(&format!("{BASE}/players/Priv-1111/summary"), 200, br#"{"competitive":null}"#)
            .with(&format!("{BASE}/players/Priv-1111/stats/summary"), 200, br#"{"general":null,"roles":null,"heroes":null}"#)
            .with(&format!("{BASE}/players/Pub-2222/summary"), 200, br#"{"competitive":null}"#)
            .with(&format!("{BASE}/players/Pub-2222/stats/summary"), 200, TEKROP_STATS);
        assert!(matches!(lookup(&http, BASE, "Priv#1111").unwrap(), Lookup::Found { private_profile: true, .. }));
        assert!(matches!(lookup(&http, BASE, "Pub#2222").unwrap(), Lookup::Found { private_profile: false, .. }));
        assert!(stats_have_data(TEKROP_STATS));
        assert!(!stats_have_data(b"{}"));
    }
}
