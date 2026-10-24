// Downloads Blizzard artwork the repository does not carry, for local builds, the mock UI and the screenshot tests:
// - rank, division and role icons into temp/rank-icons. The URLs come from src-tauri/resources/ow-icons.json, the same
//   list the app uses at runtime (the app downloads them itself on first use; this copy is for the preview only).
// - the Battle.net and Overwatch emblems into temp/brand (battlenet.svg, overwatch.svg), from Wikimedia Commons. A build
//   made after this bundles them (src/emblems.ts); without them the app shows no emblem.
// All of it is Blizzard artwork and trademarks: it stays in temp/ (gitignored) and is never committed.
//   npm run icons
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const root = new URL("..", import.meta.url).pathname;
const manifest = JSON.parse(readFileSync(join(root, "src-tauri/resources/ow-icons.json"), "utf8"));
const iconsOut = join(root, "temp/rank-icons");
const brandOut = join(root, "temp/brand");
mkdirSync(iconsOut, { recursive: true });
mkdirSync(brandOut, { recursive: true });
// Wikimedia asks every client to name itself.
const headers = { "User-Agent": "eagleeye-account-switcher-icon-fetch/1.0 (build-time asset download)" };

async function get(url) {
  const res = await fetch(url, { headers });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return Buffer.from(await res.arrayBuffer());
}

// Battle.net: the Commons file is the full logo (emblem plus word mark). Keep only the blue emblem path and frame it
// in a square view box.
function battlenetEmblem(svg) {
  const path = svg.toString("utf8").match(/<path fill="#138ffe" d="[^"]+"\/>/);
  if (!path) throw new Error("the blue emblem path was not found; the Commons file changed");
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="609 -14 830 830">${path[0]}</svg>\n`;
}

// Overwatch: the circle logo with its grey ring turned white, so it reads on the dark ground.
function overwatchEmblem(svg) {
  const text = svg.toString("utf8");
  if (!text.includes("fill:#4a4c4e")) throw new Error("the grey fill was not found; the Commons file changed");
  return text.replaceAll("fill:#4a4c4e", "fill:#ffffff");
}

const wanted = [
  ...Object.entries(manifest.tiers).map(([k, url]) => [join(iconsOut, `tier_${k}.png`), url, null]),
  ...Object.entries(manifest.divisions).map(([k, url]) => [join(iconsOut, `div_${k}.png`), url, null]),
  ...Object.entries(manifest.roles).map(([k, url]) => [join(iconsOut, `role_${k}.svg`), url, null]),
  [join(brandOut, "battlenet.svg"), "https://commons.wikimedia.org/wiki/Special:FilePath/Battle.net_logo.svg", battlenetEmblem],
  [join(brandOut, "overwatch.svg"), "https://commons.wikimedia.org/wiki/Special:FilePath/Overwatch_circle_logo.svg", overwatchEmblem],
];
let fetched = 0, failed = 0;
for (const [path, url, transform] of wanted) {
  if (existsSync(path)) continue;
  try {
    const body = await get(url);
    writeFileSync(path, transform ? transform(body) : body);
    fetched++;
  } catch (e) {
    failed++;
    console.log(`could not fetch ${path.slice(root.length)}: ${e.message}`);
  }
}
console.log(`icons: ${fetched} fetched, ${wanted.length - fetched - failed} already there, ${failed} failed -> temp/rank-icons, temp/brand`);
process.exit(failed ? 1 : 0);
