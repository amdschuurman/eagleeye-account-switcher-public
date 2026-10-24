// The README screenshots, taken from the mock app at 2x scale: real UI only, no overlays. The accounts and BattleTags
// are invented; the player icons are real Overwatch icons loaded from Blizzard's CDN (so the run needs internet).
// Icons and emblems: npm run icons                            (Blizzard art into temp/, never committed)
// Serve a build:       npm run build && npm run preview       (http://127.0.0.1:1421/; build after `npm run icons`)
// Then:                npm run shots:readme
// Writes PNGs to temp/readme-shots/; convert them to WebP for docs/screenshots/ (each under 400 KB), for example
// `cwebp -q 90 hero.png -o docs/screenshots/hero.webp` or Pillow (`Image.save(path, "WEBP", quality=90)`).
// Updates are off in this repository, so every shot uses the "disabled" update state a build from here shows.
// Env: BASE_URL, CHROMIUM_PATH (same as tests/shots.mjs).
import { existsSync, mkdirSync, readdirSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { chromium } from "playwright-core";

const base = process.env.BASE_URL || "http://127.0.0.1:1421/";
const out = new URL("../temp/readme-shots/", import.meta.url).pathname;
mkdirSync(out, { recursive: true });
const iconSrc = new URL("../temp/rank-icons/", import.meta.url).pathname;

function findChromium() {
  if (process.env.CHROMIUM_PATH) return process.env.CHROMIUM_PATH;
  const root = join(homedir(), ".cache/ms-playwright");
  for (const d of existsSync(root) ? readdirSync(root).sort().reverse() : []) {
    const p = join(root, d, "chrome-headless-shell-linux64/chrome-headless-shell");
    if (d.startsWith("chromium_headless_shell") && existsSync(p)) return p;
  }
  throw new Error("No headless Chromium found; set CHROMIUM_PATH");
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const browser = await chromium.launch({ executablePath: findChromium(), args: ["--disable-gpu"] });
const ctx = await browser.newContext({ viewport: { width: 960, height: 620 }, deviceScaleFactor: 2 });

async function open(query = "") {
  const page = await ctx.newPage();
  await page.route("**/temp/rank-icons/*", (route) => {
    const file = join(iconSrc, decodeURIComponent(new URL(route.request().url()).pathname.split("/").pop()));
    if (!existsSync(file)) return route.fulfill({ status: 404, body: "" });
    return route.fulfill({ status: 200, contentType: file.endsWith(".svg") ? "image/svg+xml" : "image/png", body: readFileSync(file) });
  });
  await page.goto(base + "?update=disabled" + query);
  await page.waitForSelector(".acct-card, .banner", { timeout: 8000 });
  // Player icons come from the CDN: wait until every one has drawn.
  await page.waitForFunction(() => {
    const imgs = [...document.querySelectorAll(".acct-card img.avatar-img")];
    return imgs.length > 0 && imgs.every((i) => i.complete && i.naturalWidth > 0);
  }, null, { timeout: 15000 });
  await sleep(900);
  await page.mouse.move(480, 610);
  return page;
}
const card = (page, label) => page.locator(`.acct-card[aria-label^="${label},"]`);
const shot = (page, name, clip) => page.screenshot({ path: join(out, name + ".png"), clip });

// Hero: the main grid, every card with its player icon and best rank.
let page = await open();
await shot(page, "hero");
// The rank card on the Champion account, top of the window only.
const b = await card(page, "Kestrel").boundingBox();
await page.mouse.move(b.x + 40, b.y + 30);
await page.waitForSelector(".rank-card.shown", { timeout: 2000 });
await sleep(450);
await shot(page, "rank-card", { x: 0, y: 0, width: 960, height: 430 });
await page.close();

// The rank filter: Damage between Gold 5 and Platinum 1, popover open.
page = await open();
await page.locator(".tool-btn", { hasText: "Filter" }).click();
await sleep(200);
await page.locator(".seg-btn", { hasText: "Damage" }).click();
await page.locator(".rank-select", { hasText: "From" }).locator("select").selectOption({ label: "Gold 5" });
await page.locator(".rank-select", { hasText: "To" }).locator("select").selectOption({ label: "Platinum 1" });
await sleep(500);
await page.mouse.move(700, 600);
await sleep(200);
await shot(page, "filter");
await page.close();

// Platforms: one step back from the Battle.net page.
page = await open();
await page.locator(".history-btns button[aria-label=Back]").click();
await sleep(500);
await page.mouse.move(480, 610);
await sleep(200);
await shot(page, "platforms");
await page.close();

// Settings.
page = await open();
await page.locator(".tb-settings").click();
await sleep(400);
await shot(page, "settings");
await page.close();

await browser.close();
console.log(`readme shots -> ${out}`);
