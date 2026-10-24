# Design

How the EagleEye Account Switcher is put together, for anyone who wants to change or extend it. The Battle.net
facts it relies on are in [RESEARCH.md](RESEARCH.md).

Stack: **Tauri 2** with a **Rust engine** (`src-tauri/`) and a **React + TypeScript UI** (`src/`). Windows only.
Battle.net only.

## Layout

| Path | What lives there |
|---|---|
| `src-tauri/src/engine.rs` | The switch, add, save-current and import flows (one at a time, behind a busy flag) |
| `src-tauri/src/commands.rs` | The Tauri commands the UI calls (the contract below) |
| `src-tauri/src/registry.rs`, `vault.rs` | UnifiedAuth tokens in the registry, and our DPAPI-encrypted copies |
| `src-tauri/src/procs.rs` | Finding, closing and starting Battle.net, and closing a running Blizzard game |
| `src-tauri/src/logwatch.rs` | Reading the signed-in account from Battle.net's newest log |
| `src-tauri/src/battletag.rs` | BattleTags from Battle.net's `CachedData.db` (read only) |
| `src-tauri/src/overfast.rs`, `ranks.rs`, `icons.rs`, `avatars.rs` | Overwatch ranks, rank icons and player icons |
| `src-tauri/src/tcno.rs` | Import from TcNo Account Switcher |
| `src-tauri/src/tray.rs`, `updater.rs` | Tray icon and close-to-tray; optional updates |
| `src/api/` | The command contract in TypeScript, the real Tauri bridge, and a mock engine for the browser |
| `src/ui/`, `src/styles/` | Components and styles (colours only from `palette.css` through `theme.css`); `src/ui/Pages.tsx` holds the Platforms and Battle.net pages |
| `src/emblems.ts` | The optional Battle.net and Overwatch emblems, bundled only when `npm run icons` fetched them |
| `tools/fetch-rank-icons.mjs` | `npm run icons`: Blizzard rank, role and brand art into `temp/` for local builds and the preview |
| `tests/shots.mjs`, `tests/readme-shots.mjs` | Drive the mock UI headless: checks of every screen, and the README screenshots |

Platform specific code sits behind traits (`Procs`, the registry, the clock), so the engine's state machine and its
tests run on Linux with fakes. `cargo test` covers the flows; the real Win32 calls only compile on Windows.

## Data on disk

Everything lives in `%APPDATA%\EagleEye Account Switcher\`, on the user's own PC:

| File | Contents |
|---|---|
| `accounts.json` | The non-secret list: per account its `code` (8 hex Battle.net account code), `label`, `battletag`, masked email (`ke***@g***.com`), `order`, `lastUsed`, `status` (`ready` or `needs_login`) |
| `vault\<code>.bin` | Windows DPAPI (`CryptProtectData`, current user, app-specific entropy) over `{email, uaName, uaValue, capturedAt}`. Written atomically. Never logged, never printed |
| `settings.json` | Battle.net folder, close after switch, minimise to tray, OverFast URL |
| `meta.json` | Per account category (Main or Smurf) |
| `ranks.json` | Cached rank lookups, player icon URLs and manual rank overrides |
| `icons\` | Rank icons and player icons fetched from Blizzard at runtime |

All writes go through a temp file in the same folder and a rename, so a crash never leaves a half-written file.

## Flows

### Switch
1. If a Blizzard game is running, close it first (WM_CLOSE to its windows, then terminate if it does not exit).
   Only one switch runs at a time; extra clicks are ignored.
2. If Battle.net is already signed in to the target account, bring it forward and stop. Otherwise save the current
   account's token from the registry first, since tokens rotate on every login.
3. Close Battle.net: terminate only `Battle.net.exe` processes whose image path is inside the configured Battle.net
   folder, and wait for them to exit. `Agent.exe` is left running.
4. Write the target's token to `UnifiedAuth\<uaName>`, and set `Client.SavedAccountNames` (the email) and
   `Client.AutoLogin` in `Battle.net.config`, keeping every other key as it was.
5. Start `Battle.net Launcher.exe`.
6. Watch the new log for the target's account code (up to 20 s). The small login window instead means
   `needs_login`.
7. On success, save the rotated token again (and again 5 s and 30 s later).

Each stage is reported to the UI as a `switch-progress` event with the elapsed time.

### Add account
1. Close Battle.net, take a hash of every UnifiedAuth value, clear `SavedAccountNames`.
2. Start Battle.net. It shows its login window and the user logs in once with "Keep me logged in".
3. When the log names an account, the UnifiedAuth value that is new or changed is that account's token. The email is
   `SavedAccountNames` after the login; the BattleTag comes from `CachedData.db`.
4. Save the vault file and the account entry.

**Save current** is the same without the login: it keeps the current email, relaunches, and captures the token that
changed. It takes about 5 s.

### Import from TcNo
If `%APPDATA%\TcNo Account Switcher\LoginCache\BattleNet\*\email.json` exists, each email goes through the save
cycle. A sign-in imports it as `ready` (labelled with TcNo's folder name); a login window imports it as
`needs_login`. At the end the app switches back to the account that was signed in before.

### Needs login
Switching to a `needs_login` account opens Battle.net with the email filled in. When the user logs in, the token is
captured the same way as in Add and the account becomes `ready`.

## Command contract

The UI depends on exactly these commands (`src/api/types.ts` is the TypeScript side):

```
list_accounts() -> Account[]              // with category
current_account() -> string | null        // signed-in code from the newest log
switch_account(code) -> SwitchResult      // {ok, status: "done" | "needs_login" | "error", ms, message}
add_account() -> Account                  // resolves when captured; rejects on cancel or timeout
cancel_add()
save_current() -> Account
import_tcno() -> ImportSummary            // {imported, needsLogin, skipped}
tcno_available() -> number
rename_account(code, label), remove_account(code), reorder_accounts(codes)
get_settings(), set_settings(settings)
get_ranks(code), get_all_ranks(), refresh_ranks(code | null, force?)
set_rank_override(code, role, rank | null), set_category(code, category), sort_by_rank(role | null)
icon_dir() -> string                      // tier_<tier>.png, div_<n>.png, role_<role>.svg, avatars/
update_state() -> UpdateState             // {status, current, version?, notes?, progress?, error?, checkedAt?, retryAt?}
check_update() -> UpdateState             // check now; records the answer only; inside the cooldown no request
install_update()                          // "Update now": returns at once, follow update-state
```

Events: `switch-progress` `{code, stage, ms}` (stages `closing_game`, `closing`, `restoring`, `launching`,
`verifying`, `done`, `needs_login`, `error`), `add-progress` `{stage, message?}`, `import-progress`
`{done, total, label, status}`, `ranks-updated` `{code}`, `ranks-error` `{code, error}`, `update-state` (status
`idle`, `checking`, `available`, `downloading`, `installing`, `error`, `upToDate`, `rateLimited` or `disabled`;
`checkedAt` and `retryAt` are ms since the epoch).

## Ranks

- The BattleTag of each account comes from `CachedData.db`, so nothing needs typing.
- Hovering reads the local cache only, so the card appears instantly. The engine refreshes each account at most
  once an hour in the background, and right after a switch for the account just signed in.
- Rank, division and role icons are Blizzard artwork. Their URLs are listed in `src-tauri/resources/ow-icons.json`;
  the app downloads them once into `icons\` on first use. They are never committed or bundled. When Blizzard changes
  the file names, refresh that list.
- Player icons: the URL from the summary is downloaded once into `icons\avatars\` (https on Blizzard's CDN only, PNG
  only, at most 512 KB). A failure never fails a refresh; the card falls back to the letter circle.
- Manual overrides per role: a rank, an "in placements" flag and an optional predicted rank. Predicted ranks are
  drawn faded with a soft pulse and the word "Predicted". A manual value wins until a newer season arrives.
- Sorting uses tier, then division (Champion 1 highest, Bronze 5 lowest); unranked accounts go last.

## UI

- Frameless window (about 960 x 620) with its own title bar. The EagleEye eagle mark sits top left; it is the original
  black-body mark on the dark ground and is never recoloured. Next to the name: back and forward buttons. Top right,
  one row: the update button (only while an update is offered), settings, minimise, close.
- Two pages. Back goes to Platforms, a data-driven list (`PLATFORMS` in `src/ui/Pages.tsx`, Battle.net only today, so
  another platform is one more entry). Its card opens the Battle.net page with the account grid. Alt+Left/Right and
  the mouse's back and forward buttons navigate too.
- The Battle.net heading and the platform card carry the Battle.net and Overwatch emblems. They are Blizzard
  trademarks, so the repository does not carry them: `npm run icons` downloads them into `temp/brand/` and
  `src/emblems.ts` bundles them at build time when they are there. Without them the heading has no emblem and the
  platform card shows the letter B.
- Colours come only from `src/styles/palette.css` (the `--ee-*` tokens), named for use in `theme.css`. `app.css` and
  `ranks.css` hold no colour literals; anything else is derived from the tokens with `color-mix`.
- The background is one diagonal gradient: the window ground (`--ee-ground`) from the top left to the middle, fading
  into the darker `--ee-ground-end` in the bottom right corner. No artwork.
- Motion: fades and short slides, 150 to 250 ms, closing faster than opening, all of it off under reduced motion.
  Sort, filter and tab changes animate the cards to their new places (FLIP, transform only, skipped above 60 cards);
  a page change is a short crossfade with a slight slide in the direction of travel.

## Updates

`updater.rs` is wired in but switched off: `plugins.updater.endpoints` in `tauri.conf.json` is empty, so the app
never checks, shows no update button, and Settings says "Updates are not set up in this build". With an endpoint and
a public key set, updates are offered, never forced:

- The engine checks 5 to 30 s after start, then about every 6 hours with up to 15 minutes of random jitter, and only
  records the answer. An orange "Update to X" button joins the title bar stack (a small cross hides it for the
  session). Nothing downloads until the user presses it or "Update now" in Settings; the app then waits for a running
  switch, add or import, installs the signed build the check verified and restarts. A failed download keeps the offer,
  so Retry installs again.
- Settings has "Check for updates". It shows "Checking for updates" for at least 1.1 s, then crossfades to "You are up
  to date", the offer, or "Try again in N s".
- Load on the update server is bounded: at most one manual check a minute (a press inside that minute makes no
  request), a failed check backs off 15 min, 30, 60 and so on up to 6 h, and a server that answers with an error
  status (429, 503) is left alone for 15 minutes.

See "Set up your own auto-updates" in the [README](../README.md).
