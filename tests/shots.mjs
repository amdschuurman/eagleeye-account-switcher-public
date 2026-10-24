// Screenshots and behaviour checks of the mock app (plain browser, no Tauri) at the real window size, 960 x 620, scale 2.
// Serve a build first:  npm run build && npm run preview      (http://127.0.0.1:1421/)
// Then:                 npm run test:shots
// Env: BASE_URL (default http://127.0.0.1:1421/), SHOTS_DIR (default temp/<today>-ui-shots), CHROMIUM_PATH.
// Uses the headless shell Playwright keeps in ~/.cache/ms-playwright (npx playwright install chromium).
// The shots show the real UI only (no overlays). Player icons load from Blizzard's CDN (mock.ts), so the run needs internet.
import { existsSync, mkdirSync, readdirSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { chromium } from "playwright-core";

const base = process.env.BASE_URL || "http://127.0.0.1:1421/";
const out = process.env.SHOTS_DIR || new URL(`../temp/${new Date().toISOString().slice(0, 10)}-ui-shots/`, import.meta.url).pathname;
mkdirSync(out, { recursive: true });
// The mock reads Blizzard's rank art from temp/rank-icons (fetched for local dev by `npm run icons`, never committed); a
// built preview does not serve temp/, so the test serves that folder by route. Without it the UI draws its placeholder.
const iconSrc = new URL("../temp/rank-icons/", import.meta.url).pathname;
// The Battle.net and Overwatch emblems are bundled only when temp/brand held them at build time (src/emblems.ts).
// The test assumes the build and this folder agree: `npm run icons` before `npm run build`, or neither.
const haveBrand = existsSync(new URL("../temp/brand/battlenet.svg", import.meta.url).pathname);

function findChromium() {
  if (process.env.CHROMIUM_PATH) return process.env.CHROMIUM_PATH;
  const root = join(homedir(), ".cache/ms-playwright");
  for (const d of existsSync(root) ? readdirSync(root).sort().reverse() : []) {
    const p = join(root, d, "chrome-headless-shell-linux64/chrome-headless-shell");
    if (d.startsWith("chromium_headless_shell") && existsSync(p)) return p;
  }
  throw new Error("No headless Chromium found; set CHROMIUM_PATH");
}

let pass = 0, fail = 0;
const check = (ok, what) => { if (ok) pass++; else { fail++; console.log("FAIL", what); } };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const browser = await chromium.launch({ executablePath: findChromium(), args: ["--disable-gpu"] });
const ctx = await browser.newContext({ viewport: { width: 960, height: 620 }, deviceScaleFactor: 2 });
const errors = [];
async function open(query = "") {
  const page = await ctx.newPage();
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => { if (m.type() === "error" && !/favicon|404|Not Found/.test(m.text())) errors.push(m.text()); });
  await page.route("**/temp/rank-icons/*", (route) => {
    const file = join(iconSrc, decodeURIComponent(new URL(route.request().url()).pathname.split("/").pop()));
    if (!existsSync(file)) return route.fulfill({ status: 404, body: "" }); // the UI draws its placeholder
    return route.fulfill({ status: 200, contentType: file.endsWith(".svg") ? "image/svg+xml" : "image/png", body: readFileSync(file) });
  });
  await page.goto(base + query);
  await page.waitForSelector(".acct-card, .banner", { timeout: 8000 });
  await sleep(800); // entrance fades settle
  return page;
}
const avatarsLoaded = (page) => page.waitForFunction(() => {
  const imgs = [...document.querySelectorAll(".acct-card img.avatar-img")];
  return imgs.length > 0 && imgs.every((i) => i.complete && i.naturalWidth > 0);
}, null, { timeout: 10000 });
// A card by its exact label (hasText would also match inside other words).
const card = (page, label) => page.locator(`.acct-card[aria-label^="${label},"]`);
const shot = (page, name) => page.screenshot({ path: join(out, name + ".png") });
const away = (page) => page.mouse.move(480, 610);
const labels = (page) => page.locator(".acct-card .card-label").allInnerTexts();

// ---- 1. Accounts page ----
let page = await open();
check((await page.locator(".acct-card").count()) === 8, "8 account cards");
check((await page.locator(".add-card").count()) === 1, "add card last");
check((await page.locator(".card.signed-in .tag-signed").count()) === 1, "one signed-in card with its tag");
check((await page.locator(".acct-card.needs .card-state.warn").first().innerText()) === "Log in once", "needs_login card says Log in once");
check((await page.locator(".heading h1").innerText()) === "Battle.net", "Battle.net heading");
check((await page.locator(".heading .heading-emblem").count()) === (haveBrand ? 2 : 0), haveBrand ? "Battle.net and Overwatch emblems by the heading" : "no emblems in a build without them");
const broken = await page.locator("img").evaluateAll((els) => els.filter((e) => e.complete && e.naturalWidth === 0).length);
check(broken === 0, `no broken images (${broken})`);
check((await page.locator(".history-btns button[aria-label=Back]").isEnabled()) && !(await page.locator(".history-btns button[aria-label=Forward]").isEnabled()), "starts with Back on, Forward off");
const stack = await page.locator(".winctl > *").evaluateAll((els) => els.map((e) => e.getAttribute("aria-label") || e.className));
check(stack.join("|") === "Settings|Minimize|Close", `top-right stack (${stack.join("|")})`);
const sb = await page.locator(".tb-settings").boundingBox(), mb = await page.locator(".winctl button[aria-label=Minimize]").boundingBox();
check(sb.width === mb.width && sb.height === mb.height && Math.round(sb.x + sb.width) === Math.round(mb.x), `settings sits left of minimise, same size (${sb.width}x${sb.height})`);
await avatarsLoaded(page);
const avatarKinds = await page.locator(".acct-card .card-top > .avatar").evaluateAll((els) => els.map((e) => (e.tagName === "IMG" && e.complete && e.naturalWidth > 0 ? "img" : "other")));
check(avatarKinds.every((k) => k === "img"), `every card draws its player icon (${avatarKinds.join(",")})`);
const subs = await page.locator(".acct-card .card-sub").allInnerTexts();
check(subs.every((t) => /^[A-Z][a-z]+#\d{4}$/.test(t)), "card subtitles are BattleTags, never an email");
await page.keyboard.press("ArrowRight");
check((await page.locator(".acct-card.selected .card-label").innerText()) === "Nimbus", "ArrowRight selects the next card");
await page.keyboard.press("ArrowLeft");
await page.waitForSelector(".rank-badge");
const badges = (await page.locator(".acct-card .rank-badge").allInnerTexts()).map((t) => t.trim());
check(badges[0] === "Champion 3" && badges[2] === "Platinum 2" && badges[4] === "Unranked" && badges[5] === "Private", `badge texts (${badges.join(", ")})`);
await away(page);
await sleep(300);
await shot(page, "01-accounts");

// Mid-switch and finish line.
await card(page, "Nimbus").dblclick();
await sleep(2700);
const state = await page.locator(".acct-card.busy .card-state").innerText();
check(["Starting Battle.net", "Signing in"].includes(state), `mid-switch stage words (${state})`);
await page.waitForSelector(".acct-card.signed-in:has-text('Nimbus')", { timeout: 8000 });
await sleep(300);
check(/^Signed in, \d+\.\d s$/.test(await page.locator(".acct-card.signed-in .card-state").innerText()), "final line");

// Context menu, rename, remove (the remove leaves with a fade copy, then the card is gone).
await card(page, "Vantage").click({ button: "right" });
await sleep(250);
const items = (await page.locator(".ctx-menu button").allInnerTexts()).map((t) => t.trim().replace(/\s+/g, " ")).join("|");
check(items === "Rename|Edit ranks|Refresh ranks|Set as Main Now|Set as Smurf|Move left|Move right|Remove", `menu items (${items})`);
check((await page.locator(".ctx-menu .ctx-head").innerText()) === "va***@p***.me", "masked email only in the menu header");
await page.locator(".ctx-menu button", { hasText: "Rename" }).click();
await page.keyboard.press("Control+A");
await page.keyboard.type("Vantage EU");
await page.keyboard.press("Enter");
await sleep(250);
check((await page.locator(".card-label", { hasText: "Vantage EU" }).count()) === 1, "inline rename");
await card(page, "Vantage EU").click({ button: "right" });
await page.locator(".ctx-menu button", { hasText: "Remove" }).click();
await sleep(300);
await page.locator(".dialog .btn-danger").click();
await sleep(60);
check((await page.locator(".flip-ghost").count()) === 1, "the removed card fades out as a copy");
await sleep(400);
check((await page.locator(".acct-card").count()) === 7 && (await page.locator(".flip-ghost").count()) === 0, "remove done, fade copy cleaned up");
await page.close();

// ---- 2. Platforms page and history ----
page = await open();
await page.locator(".history-btns button[aria-label=Back]").click();
await sleep(400);
check((await page.locator(".page h1").innerText()) === "Platforms", "Back opens Platforms");
check((await page.locator(".page").count()) === 1, "the old page is gone after the transition");
const pf = (await page.locator(".platform-card").allInnerTexts()).map((t) => t.replace(/\s+/g, " "));
check(pf.length === 1 && pf[0].includes("Battle.net") && pf[0].includes("Overwatch, 8 accounts") && pf[0].includes("Signed in as Kestrel"), `one Battle.net card (${pf.join(" / ").replace(/\s+/g, " ")})`);
check(!(await page.locator(".history-btns button[aria-label=Back]").isEnabled()) && (await page.locator(".history-btns button[aria-label=Forward]").isEnabled()), "on Platforms: Back off, Forward on");
check((await page.locator(".platform-card .pf-emblem").count()) === (haveBrand ? 1 : 0) && (await page.locator(".platform-card .pf-initial").count()) === (haveBrand ? 0 : 1), "platform tile: emblem, or the initial without it");
await away(page);
await shot(page, "02-platforms");
await page.keyboard.press("Enter"); // the card has focus
await sleep(400);
check((await page.locator(".page h1").innerText()) === "Battle.net", "Enter on the card opens Battle.net");
check(!(await page.locator(".history-btns button[aria-label=Forward]").isEnabled()), "opening a page clears Forward");
await page.keyboard.press("Alt+ArrowLeft");
await sleep(400);
check((await page.locator(".page h1").innerText()) === "Platforms", "Alt+Left goes back");
await page.keyboard.press("Alt+ArrowRight");
await sleep(400);
check((await page.locator(".page h1").innerText()) === "Battle.net", "Alt+Right goes forward");
// Playwright cannot press the extra mouse buttons; send the events the webview would (button 3 = back, 4 = forward).
const xbutton = (button) => page.evaluate((b) => { for (const t of ["mousedown", "mouseup"]) document.body.dispatchEvent(new MouseEvent(t, { button: b, bubbles: true, cancelable: true })); }, button);
await xbutton(3);
await sleep(400);
check((await page.locator(".page h1").innerText()) === "Platforms", "mouse back button goes back");
await xbutton(4);
await sleep(400);
check((await page.locator(".page h1").innerText()) === "Battle.net", "mouse forward button goes forward");
await page.close();

// ---- 3. Categories, sort and the filter popover ----
page = await open();
await page.waitForSelector(".rank-badge");
const chipText = (await page.locator(".chips .chip").allInnerTexts()).map((t) => t.replace(/\s+/g, " ").trim()).join("|");
check(chipText === "All 8|Main 4|Smurf 4", `category chips (${chipText})`);
await page.locator(".chip", { hasText: "Smurf" }).click();
await sleep(40);
check((await page.locator(".flip-ghost").count()) === 4, "leaving cards fade out as copies");
await sleep(300);
check((await page.locator(".acct-card").count()) === 4 && (await page.locator(".flip-ghost").count()) === 0, "Smurf chip shows 4, copies gone");
await page.locator(".chip", { hasText: "All" }).click();
await sleep(300);
await page.locator(".tool-btn", { hasText: "Sort" }).click();
await sleep(150);
await page.locator(".pop-menu button", { hasText: "Highest rank" }).click();
await sleep(60);
const moving = await page.locator(".acct-card").evaluateAll((els) => els.filter((e) => e.getAnimations().some((a) => a.id === "flip")).length);
check(moving > 0, `sorting slides cards to their new spot (${moving} moving)`);
await sleep(300);
const order = await labels(page);
check(order.join(",") === "Kestrel,Nimbus,Corvid,Vantage,Tessel,Halcyon,Pebble,Lumen", `highest rank order (${order.join(",")})`);
await page.locator(".tool-btn", { hasText: "Sort" }).click();
await page.locator(".pop-menu button", { hasText: "Manual" }).click();
await sleep(300);
await page.locator(".tool-btn", { hasText: "Filter" }).click();
await sleep(200);
await page.locator(".seg-btn", { hasText: "Damage" }).click();
await page.locator(".rank-select", { hasText: "From" }).locator("select").selectOption({ label: "Gold 5" });
await page.locator(".rank-select", { hasText: "To" }).locator("select").selectOption({ label: "Platinum 1" });
await sleep(400);
const filtered = await labels(page);
check([...filtered].sort().join(",") === "Corvid,Halcyon", `damage Gold 5 to Platinum 1 (${filtered.join(",")})`);
check((await page.locator(".filter-chip").innerText()).includes("Damage, Gold 5 to Platinum 1"), "filter chip text");
await page.mouse.move(700, 600);
await sleep(200);
await shot(page, "03-filter");
await page.keyboard.press("Escape");
await page.locator(".filter-chip .chip-x").click();
await sleep(350);
check((await page.locator(".acct-card").count()) === 8, "clear chip shows all again");
await page.close();

// ---- 4. Settings: Check for updates, up to date ----
page = await open();
check((await page.locator(".update-pill").count()) === 0, "no update pill without an update");
await page.locator(".tb-settings").click();
await sleep(250);
const sp = await page.locator(".settings-panel").boundingBox();
check(Math.abs(sp.x + sp.width - (sb.x + sb.width)) <= 1 && sp.y >= sb.y + sb.height, "settings panel hangs under the cog");
await page.locator(".update-row .update-head .btn-grey", { hasText: "Check for updates" }).click();
await sleep(80);
check((await page.locator(".update-row .swap-in .update-row-text").innerText()) === "Checking for updates", "shows Checking for updates");
check(await page.locator(".update-row .update-head .btn-grey").isDisabled(), "button off while checking");
check((await page.locator(".update-row .swap-in .update-row-line.indef").count()) === 1, "drifting line while checking");
await sleep(800); // the mock answers after 400 ms; the row holds Checking for at least 1.1 s
check((await page.locator(".update-row .swap-in .update-row-text").innerText()) === "Checking for updates", "Checking held for at least 1.1 s");
await page.waitForSelector(".update-row .swap-in .update-row-text.ok", { timeout: 3000 });
await sleep(60);
check((await page.locator(".update-row .swap-out").count()) === 1, "crossfade: the old state fades out under the new one");
await sleep(300);
check((await page.locator(".update-row .swap-in .update-row-text").innerText()) === "You are up to date (v1.0.0)", "up to date text");
check(await page.locator(".update-row .update-head .btn-grey").isDisabled(), "check button off during the 60 s cooldown");
await shot(page, "04-settings-up-to-date");
await page.close();

// ---- 5. Settings: rate limited, live countdown ----
page = await open("?update=ratelimited");
await page.locator(".tb-settings").click();
await sleep(300);
const rl1 = await page.locator(".update-row .swap-in .update-row-text").innerText();
check(/^Checked a moment ago\. Try again in (4[3-5]) s$/.test(rl1), `rate limited text (${rl1})`);
check(await page.locator(".update-row .update-head .btn-grey").isDisabled(), "button off until retryAt");
await sleep(1300);
const rl2 = await page.locator(".update-row .swap-in .update-row-text").innerText();
check(rl2 !== rl1 && /Try again in \d+ s$/.test(rl2), `countdown runs (${rl1} then ${rl2})`);
await shot(page, "05-settings-rate-limited");
await page.close();

// ---- 6. Update pill ----
page = await open("?update");
await page.waitForSelector(".update-pill.available");
check((await page.locator(".update-pill .update-main").innerText()) === "Update to 1.1.0", "pill reads Update to 1.1.0");
const stack2 = await page.locator(".winctl > *").evaluateAll((els) => els.map((e) => e.getAttribute("aria-label") || e.className.split(" ")[0]));
check(stack2.join("|") === "update-pill|Settings|Minimize|Close", `pill left of settings (${stack2.join("|")})`);
await sleep(400);
await shot(page, "06-update-pill");
await page.locator(".update-pill .update-main").click();
await page.waitForSelector(".update-pill.downloading", { timeout: 3000 });
await sleep(1500);
check(/^Downloading 1\.1\.0, \d+%$/.test(await page.locator(".update-pill .update-text").innerText()), "download shows a percentage");
await page.waitForSelector(".update-pill.installing", { timeout: 8000 });
check((await page.locator(".update-pill .update-text").innerText()) === "Installing, the app restarts", "installing text");
await page.close();

page = await open("?update=available");
await page.locator(".update-pill .update-x").click();
await sleep(300);
check((await page.locator(".update-pill").count()) === 0, "the cross hides the pill");
await page.locator(".tb-settings").click();
await sleep(250);
check((await page.locator(".update-row .btn-orange").innerText()) === "Update now", "Settings still offers the update after hiding the pill");
await page.close();

page = await open("?update=error");
check((await page.locator(".update-pill.error .update-btn").innerText()) === "Retry", "failed install offers Retry");
check((await page.locator(".update-pill.error .update-text").innerText()) === "Update to 1.1.0 failed", "failed install line");
await page.close();

page = await open("?update=checkerror");
await page.locator(".tb-settings").click();
await sleep(250);
check((await page.locator(".update-row .swap-in .update-row-text.err").count()) === 1, "check error shows in the row");
check((await page.locator(".update-row .update-head .btn-grey").innerText()) === "Retry", "check error offers Retry");
await page.close();

// ---- 7. Hover card on a ranked account ----
const hoverCard = async (page, label) => {
  await away(page);
  await sleep(200);
  const b = await card(page, label).boundingBox();
  await page.mouse.move(b.x + 40, b.y + 30);
  await sleep(120);
  check((await page.locator(".rank-card").count()) === 0, `${label}: no card before the 250 ms delay`);
  await page.waitForSelector(".rank-card.shown", { timeout: 2000 });
  await sleep(350);
};
page = await open();
await page.waitForSelector(".rank-badge");
await avatarsLoaded(page);
await hoverCard(page, "Halcyon");
const txt = await page.locator(".rank-card").innerText();
for (const w of ["Tank", "Damage", "Support", "Open Queue", "Platinum 2", "Gold 3", "Platinum 4", "Gold 1", "Season 22", "updated 12 min ago"]) {
  check(txt.toLowerCase().includes(w.toLowerCase()), `hover card shows "${w}"`);
}
await hoverCard(page, "Kestrel");
check((await page.locator(".rank-card.side-right").count()) === 1, "the hover card opens beside the card, on the right");
await shot(page, "07-hover-card");
await hoverCard(page, "Vantage");
check((await page.locator(".rank-card").innerText()).includes("In placements, predicted Diamond 3"), "placement text");
await hoverCard(page, "Lumen");
check((await page.locator(".rank-card").innerText()).includes("Profile is private"), "private profile text");
await away(page);
await sleep(250);
check((await page.locator(".rank-card").count()) === 0, "hover card hides when the pointer leaves");
await page.close();

// A build without an update endpoint: no pill, no check button, Settings says updates are not set up.
page = await open("?update=disabled");
check((await page.locator(".update-pill").count()) === 0, "disabled: no update pill");
check((await page.locator(".winctl > *").count()) === 3, "disabled: the stack is settings, minimise, close");
await page.locator(".tb-settings").click();
await sleep(250);
check((await page.locator(".update-row .update-row-text").innerText()) === "Updates are not set up in this build", "disabled: Settings says updates are not set up");
check((await page.locator(".update-row button").count()) === 0, "disabled: no check button");
await shot(page, "08-update-disabled");
await page.close();

// Letters when there are no player icons; TcNo import banner on an empty list.
page = await open("?noavatars");
check((await page.locator(".acct-card img.avatar-img").count()) === 0 && (await page.locator(".acct-card div.avatar").count()) === 8, "no icons: every card shows its letter");
await page.close();
page = await open("?empty");
check((await page.locator(".banner .btn-orange").innerText()) === "Import 10 accounts", "import banner button");
await page.close();

// No em dash or en dash anywhere in the rendered text, on either page.
page = await open();
let text = await page.evaluate(() => document.body.innerText);
await page.locator(".history-btns button[aria-label=Back]").click();
await sleep(400);
text += await page.evaluate(() => document.body.innerText);
check(!/[\u2013\u2014]/.test(text), "no em or en dash in the UI");
await page.close();

check(errors.length === 0, "no page errors: " + errors.join(" | "));
await browser.close();
console.log(`shots: ${pass} passed, ${fail} failed -> ${out}`);
process.exit(fail ? 1 : 0);
