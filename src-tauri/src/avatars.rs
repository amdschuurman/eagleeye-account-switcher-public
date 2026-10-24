//! Overwatch player icons (the career profile's `avatar`), fetched once into the store's `icons/avatars`
//! folder as `<sha256>.png`, so the account cards show them offline and the existing icon asset scope covers them.
//!
//! OverFast returns the avatar as a Blizzard CDN URL whose file name is the image's SHA-256 (`.../<64 hex>.png`).
//! The content behind such a name never changes, so a file already on disk is never fetched again. Only https URLs
//! on Blizzard's CDN hosts with that file name shape are accepted, and only a PNG body under `MAX_BYTES` is written.

use crate::overfast::Http;
use std::path::Path;

/// Larger than any player icon (128 x 128, about 14 KB) by a wide margin.
pub const MAX_BYTES: usize = 512 * 1024;

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The hosts OverFast hands out avatars from: Blizzard's own CloudFront distribution (measured 2026-10-24) and
/// Blizzard's static host. Exact names only, not all of `*.cloudfront.net`, which anyone can rent. If Blizzard moves
/// the icons, cards fall back to the letter until this list is updated.
const ALLOWED_HOSTS: &[&str] = &["d15f34w2p8l1cc.cloudfront.net", "static.playoverwatch.com"];

fn allowed_host(host: &str) -> bool {
    ALLOWED_HOSTS.iter().any(|h| host.eq_ignore_ascii_case(h))
}

/// The checked URL to fetch and its local file name (`<sha256>.png`), or `None` when the URL is not one we accept.
/// The fetch uses the URL as parsed here, not the raw string, so the checked host is the host that is asked.
fn checked(url: &str) -> Option<(reqwest::Url, String)> {
    let u = reqwest::Url::parse(url.trim()).ok()?;
    if u.scheme() != "https" || !allowed_host(u.host_str()?) || u.port().is_some() || !u.username().is_empty() || u.password().is_some() {
        return None;
    }
    let sha = u.path_segments()?.last()?.strip_suffix(".png")?.to_ascii_lowercase();
    let ok = sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit());
    ok.then(|| (u, format!("{sha}.png")))
}

/// The local file name for an avatar URL (`<sha256>.png`), or `None` when the URL is not one we accept.
pub fn file_name(url: &str) -> Option<String> {
    checked(url).map(|(_, f)| f)
}

/// Is this body a PNG of an acceptable size?
fn acceptable(body: &[u8]) -> bool {
    body.len() <= MAX_BYTES && body.len() > PNG_MAGIC.len() && body.starts_with(PNG_MAGIC)
}

/// Make sure the avatar behind `url` is on disk in `dir` (atomic write). Returns the local file name. A present
/// file is never fetched again; a rejected URL or body writes nothing.
pub fn ensure(dir: &Path, http: &dyn Http, url: &str) -> Result<String, String> {
    let (url, file) = checked(url).ok_or("not an accepted avatar URL")?;
    let target = dir.join(&file);
    if target.is_file() {
        return Ok(file);
    }
    match http.get_capped(url.as_str(), MAX_BYTES)? {
        (200, body) if acceptable(&body) => crate::atomic::write(&target, &body).map(|()| file),
        (200, _) => Err("the avatar is not a PNG of a sensible size".into()),
        (s, _) => Err(format!("the avatar host answered {s}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overfast::test_support::FakeHttp;

    const SHA: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0";

    fn url() -> String {
        format!("https://d15f34w2p8l1cc.cloudfront.net/overwatch/{SHA}.png")
    }

    fn png(len: usize) -> Vec<u8> {
        let mut v = PNG_MAGIC.to_vec();
        v.resize(len, 7);
        v
    }

    #[test]
    fn file_names_come_only_from_accepted_urls() {
        assert_eq!(file_name(&url()), Some(format!("{SHA}.png")));
        assert_eq!(file_name(&format!("https://static.playoverwatch.com/a/{}.png", SHA.to_uppercase())), Some(format!("{SHA}.png")));
        assert_eq!(file_name(&format!("http://d15f34w2p8l1cc.cloudfront.net/overwatch/{SHA}.png")), None, "https only");
        assert_eq!(file_name(&format!("https://avatars.example.com/overwatch/{SHA}.png")), None, "other host");
        assert_eq!(file_name(&format!("https://cloudfront.net.example.com/{SHA}.png")), None, "suffix trick");
        assert_eq!(file_name(&format!("https://d15f34w2p8l1cc.cloudfront.net:8443/{SHA}.png")), None, "odd port");
        assert_eq!(file_name("https://d15f34w2p8l1cc.cloudfront.net/overwatch/portrait.png"), None, "no hash name");
        assert_eq!(file_name(&format!("https://d15f34w2p8l1cc.cloudfront.net/overwatch/{SHA}.jpg")), None);
        assert_eq!(file_name("not a url"), None);
        assert_eq!(file_name(&format!("https://dsomeoneelse.cloudfront.net/overwatch/{SHA}.png")), None, "other CloudFront tenant");
    }

    #[test]
    fn ensure_downloads_once() {
        let dir = tempfile::tempdir().unwrap();
        let http = FakeHttp::default().with(&url(), 200, &png(14_000));
        assert_eq!(ensure(dir.path(), &http, &url()).unwrap(), format!("{SHA}.png"));
        assert_eq!(std::fs::read(dir.path().join(format!("{SHA}.png"))).unwrap().len(), 14_000);
        assert_eq!(ensure(dir.path(), &http, &url()).unwrap(), format!("{SHA}.png"));
        assert_eq!(http.calls(), 1, "a present avatar is never fetched again");
    }

    #[test]
    fn ensure_rejects_non_png_oversize_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(format!("{SHA}.png"));
        let http = FakeHttp::default().with(&url(), 200, b"<html>error</html>");
        assert!(ensure(dir.path(), &http, &url()).is_err());
        assert!(!file.exists(), "an HTML body is not written");

        let http = FakeHttp::default().with(&url(), 200, &png(MAX_BYTES + 1));
        assert!(ensure(dir.path(), &http, &url()).is_err());
        assert!(!file.exists(), "an oversize body is not written");

        let http = FakeHttp::default().with(&url(), 200, PNG_MAGIC);
        assert!(ensure(dir.path(), &http, &url()).is_err(), "a bare signature is empty");

        let http = FakeHttp::default().with(&url(), 404, b"");
        assert!(ensure(dir.path(), &http, &url()).is_err());
        let http = FakeHttp::default();
        assert!(ensure(dir.path(), &http, "https://avatars.example.com/x.png").is_err());
        assert_eq!(http.calls(), 0, "a rejected URL is never requested");
        assert!(!file.exists());
    }
}
