# Contributing

Thanks for helping. Bug reports, ideas and pull requests are all welcome. Please keep changes focused: one fix or
feature per pull request.

## Set up

You need Node.js 20 or newer and Rust (stable). The interface can be developed on any system; the app itself only
builds and runs on Windows.

```
npm ci
npm run dev          # the interface in a browser with made-up accounts, http://localhost:1420
npm run icons        # optional: Blizzard's rank icons and the Battle.net and Overwatch emblems into temp/ (never committed)
```

The mock in `src/api/mock.ts` stands in for the engine. Its query flags (`?empty`, `?slow`, `?noavatars`,
`?update=...`) are listed at the top of that file. A build made after `npm run icons` includes the emblems
(`src/emblems.ts`); without them the app shows none.

On Windows, `npx tauri dev` runs the real app. Remember that it works on your real Battle.net install.

## Tests

Run these before you open a pull request:

```
npm run build
npm run preview                  # serves the build on http://127.0.0.1:1421
npm run test:shots               # in a second terminal: checks every screen, writes screenshots to temp/
cd src-tauri
cargo test
cargo check --target x86_64-pc-windows-gnu --all-targets   # from Linux or WSL; on Windows a plain cargo check
```

`test:shots` needs Playwright's headless Chromium (`npx playwright-core install chromium --only-shell` on Linux or
WSL, or set `CHROMIUM_PATH` to any Chromium). `npm run shots:readme` retakes the README screenshots.

The engine's flows are written against traits (process control, registry, clock), so `cargo test` runs them on any
system with fakes. Code that talks to Win32 directly sits behind `#[cfg(windows)]`.

## No personal data, ever

This app handles login tokens. The repository must never contain any real account data: no emails, BattleTags,
tokens, user folder paths, log excerpts or screenshots of real accounts. Use invented values in tests and mocks
(addresses ending in `.invalid` or `example.com`, made-up BattleTags like `Kestrel#2187`).

`tools/check-private.sh` blocks email addresses and user folder paths, plus any extra words you list in your own
`private/denylist.txt` (one per line, gitignored, never committed). Install it as a pre-commit hook:

```
printf '#!/bin/sh\nexec tools/check-private.sh\n' > .git/hooks/pre-commit && chmod +x .git/hooks/pre-commit
```

`tools/check-private.sh --all` checks every tracked file.

## Style

- Colours only from `src/styles/palette.css` (through `theme.css`); no colour literals in other style files.
- The EagleEye logo is the original black-body mark and is never recoloured.
- Interface text is short and plain.

## License

By contributing you agree that your contribution is licensed under the [MIT License](LICENSE).
