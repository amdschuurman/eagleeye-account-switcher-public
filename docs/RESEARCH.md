# How Battle.net keeps its login

This is the background the switcher is built on. Nothing here is documented by Blizzard; it was worked out by
watching what Battle.net reads and writes on a Windows PC. If Battle.net changes its behaviour, this is the page to
update first.

## The pieces

| What | Where | What it does |
|---|---|---|
| Login tokens | `HKCU\Software\Blizzard Entertainment\Battle.net\UnifiedAuth` | One value per account that ticked "Keep me logged in". Value name: 8 hex characters. Value: a binary blob of about 262 bytes. This is the session. |
| Which account to use | `Client.SavedAccountNames` in `%APPDATA%\Battle.net\Battle.net.config` | The email Battle.net signs in with on start. On its own it only pre-fills the email field. |
| Who is signed in | `%LOCALAPPDATA%\Battle.net\Logs\battle.net-<time>.log` | Prints `Querying parental control info for account: 1:0:<code>` once an account is signed in. `<code>` is the 8 hex account code. |
| BattleTag | `%LOCALAPPDATA%\Battle.net\CachedData.db` (SQLite) | Table `login_cache`: `battle_tag` per account; the account code is the hex of `account_id_lo`. Read only, never written. |

Note that the UnifiedAuth value name is not the same as the account code; the switcher stores both.

## What the tests showed

| Test | Result |
|---|---|
| Point `SavedAccountNames` at an account whose token exists, start Battle.net | Signed in without a password. Closing took about 0.5 s, signing in about 7.5 s |
| Which token belongs to that account | The one value that changed: the token rotates on every login |
| Delete the token, restore it from a saved copy, start Battle.net | Signed in, about 4.4 s |
| Restore the copy from before the last rotation | Still signed in (an older copy stays valid for at least minutes) |
| No token at all | Battle.net shows its small login window (about 362 x 663) |

So an account is three things: the email, the token value name and the token value. Keeping our own encrypted copy
of each token makes switching password free, and removes Battle.net's own limit on how many accounts it remembers.
`Agent.exe` (Blizzard's update agent) can keep running during a switch.

## Things to know

- **Tokens rotate.** Every login writes a new token. The switcher saves the token again right after a switch, and
  again 5 s and 30 s later, because Battle.net can rotate it once more during its security checkpoint.
- **Security checkpoint.** Battle.net runs an external challenge on token logins that completes by itself
  (`UAuth: external challenge complete` in the log).
- **Tokens can be revoked.** Blizzard can reject a token server side (for example after a password change or a
  security reset). The log then shows the token being deleted and Battle.net asks for a login. The app cannot
  prevent this; it marks the account "Log in once", and the next manual login captures a fresh token.
- **How long an unused token lasts** is not known exactly. Blizzard says "Keep me logged in" lasts up to 30 days on
  one computer. The fallback is always a one-time login.
- **Only the newest log counts.** With Battle.net sitting on its login screen, an older log can still name an
  account. Reading the signed-in account from the newest session only avoids skipping a token restore.

## Why other switchers ask for the password

TcNo Account Switcher (the best known Windows switcher) only swaps `Client.SavedAccountNames` for Battle.net. That
pre-fills the email but does not restore the session, so Battle.net asks for the password as soon as it has dropped
that account's token. Restoring the token itself is the difference.

## Ranks

Blizzard has no official API for Overwatch player data. The public career page is server rendered, and the
[OverFast API](https://github.com/TeKrop/overfast-api) (MIT, a third-party project with a free public instance at
`https://overfast-api.tekrop.fr`) turns it into JSON:

- `GET /players/<Name-1234>/summary` returns `competitive.pc.{tank,damage,support,open}` (each a tier and division,
  or `null`), the season number and the player icon URL. OverFast caches each profile for an hour.
- A private profile or an unranked role has no data. Placements and predicted ranks are not on the profile, which is
  why the app lets you set them by hand.
- A 404 does not always mean the player does not exist: OverFast caches "unknown player" for a while, so a profile
  made public minutes ago can still 404. The app then searches `players?name=Name-1234`, accepts only an exact
  `player_id` match, and reads that player's summary directly. A search hit with `is_public: false` is a private
  profile.
