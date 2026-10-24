# Third-party notices

The EagleEye Account Switcher is MIT licensed (see [LICENSE](LICENSE)). It is built on open-source software and uses
one third-party web service. The full dependency lists are in `package-lock.json` and `src-tauri/Cargo.lock`; most
dependencies are MIT or Apache-2.0 licensed. The items below have other terms or deserve a mention.

## Rust crates under the Mozilla Public License 2.0

These come in through Tauri (its HTML and CSS handling) and are used unmodified. Their source code is available on
[crates.io](https://crates.io) and from their own repositories, under the
[MPL-2.0](https://www.mozilla.org/en-US/MPL/2.0/).

- `cssparser`
- `cssparser-macros`
- `dtoa-short`
- `option-ext`
- `selectors`

## Fonts under the SIL Open Font License 1.1

Bundled through the `@fontsource` packages, under the [OFL-1.1](https://openfontlicense.org/):

- Montserrat (`@fontsource/montserrat`)
- Noto Sans (`@fontsource/noto-sans`)

## OverFast API

Overwatch ranks and player icon addresses come from the [OverFast API](https://github.com/TeKrop/overfast-api)
(MIT licensed), a third-party service that reads Blizzard's public career pages. The app uses its public instance
at `https://overfast-api.tekrop.fr` by default. This project does not run or control that service. The test fixtures
in `src-tauri/src/fixtures/` are responses from that API for the public profile used in OverFast's own documentation.

## Blizzard Entertainment

Battle.net and Overwatch are trademarks of Blizzard Entertainment, Inc. This project is not affiliated with or
endorsed by Blizzard Entertainment.

Rank icons, role icons and player icons are Blizzard artwork. They are not part of this repository and are not
shipped with the app: the app downloads them from Blizzard's own servers at runtime (the addresses are listed in
`src-tauri/resources/ow-icons.json`) and keeps them in the user's own data folder.

The Battle.net and Overwatch emblems shown next to the Battle.net heading and on the Platforms page are Blizzard
trademarks. They are not covered by this project's MIT license and are not part of this repository. `npm run icons`
(`tools/fetch-rank-icons.mjs`) downloads them from Wikimedia Commons (`Battle.net_logo.svg`, of which only the blue
emblem is kept, and `Overwatch_circle_logo.svg`, with its grey ring turned white) into `temp/brand/`, and a build made
after that includes them. A build without them works the same and simply shows no emblem.

The screenshots in `docs/screenshots/` show the rank icons, the emblems and real Overwatch player icons as they appear
in the app; the accounts and BattleTags in them are made up.
