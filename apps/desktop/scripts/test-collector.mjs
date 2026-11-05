// Runs the in-app browser's collector script (src-tauri/src/collector.js) on the fixture pages in
// collector-fixtures/ in a real headless browser (layout matters: card sizes, image sizes) and
// compares each scan with <fixture>.expected.json.
//
// Expected file: an array of cases. Each case may set
//   query          extra query for the fixture URL (includeAds=1, shape=<pattern>)
//   shape          the chosen shape
//   urls           item URLs (path + query), in order
//   ads            ad URLs left out, in order
//   thumbnails     item thumbnail paths (or null), in order
//   titles         item titles, in order
//   indexes        item indexes (page images' numbers; null for links), in order
//   page           page facts ({ title, uploader, publishedAt, id }), compared field by field
//   groupsInclude  shapes that must be among the groups
//   groupsExclude  prefixes no group shape may start with
//
// The browser: $COLLECTOR_BROWSER, else Microsoft Edge or Google Chrome at their usual places.

import { execFile } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const fixtures = join(root, "collector-fixtures");
const collectorScript = join(root, "src-tauri", "src", "collector.js");
const flowRecorderScript = join(root, "src-tauri", "src", "flow_recorder.js");

function findBrowser() {
  const candidates = [
    process.env.COLLECTOR_BROWSER,
    "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
    "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  ];
  return candidates.find((path) => path && existsSync(path));
}

function serve() {
  const server = createServer((request, response) => {
    const path = new URL(request.url, "http://localhost").pathname;
    let file = null;
    if (path === "/collector.js") file = collectorScript;
    else if (path === "/flow-recorder.js") file = flowRecorderScript;
    else if (/^\/[\w.-]+\.(html|js)$/.test(path)) file = join(fixtures, path.slice(1));
    if (!file || !existsSync(file)) {
      response.writeHead(404).end();
      return;
    }
    const type = extname(file) === ".html" ? "text/html; charset=utf-8" : "text/javascript; charset=utf-8";
    response.writeHead(200, { "content-type": type, "cache-control": "no-store" }).end(readFileSync(file));
  });
  return new Promise((done) => server.listen(0, "127.0.0.1", () => done(server)));
}

function dumpDom(browser, url) {
  const profile = mkdtempSync(join(tmpdir(), "vdl-collector-test-"));
  const args = [
    "--headless=new",
    "--disable-gpu",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-extensions",
    "--disable-sync",
    "--inprivate",
    `--user-data-dir=${profile}`,
    "--window-size=1280,900",
    "--dump-dom",
    url,
  ];
  return new Promise((done, fail) => {
    execFile(browser, args, { timeout: 60_000, maxBuffer: 64 * 1024 * 1024 }, (error, stdout) => {
      rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
      if (error && !stdout) fail(error);
      else done(stdout);
    });
  });
}

function decodeEntities(text) {
  return text
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&nbsp;/g, "\u00a0")
    .replace(/&amp;/g, "&");
}

function pathOf(url) {
  if (url === null) return null;
  const parsed = new URL(url);
  return parsed.pathname + parsed.search;
}

function sameList(actual, expected) {
  return actual.length === expected.length && actual.every((value, index) => value === expected[index]);
}

function check(result, expected) {
  const problems = [];
  if (expected.step) {
    for (const [field, wanted] of Object.entries(expected.step)) {
      if (result[field] !== wanted) {
        problems.push(`${field}: expected ${JSON.stringify(wanted)}, actual ${JSON.stringify(result[field])}`);
      }
    }
    return problems;
  }
  const items = result.items ?? [];
  const compare = (label, actual, wanted) => {
    if (wanted !== undefined && !sameList(actual, wanted)) {
      problems.push(`${label}\n      expected ${JSON.stringify(wanted)}\n      actual   ${JSON.stringify(actual)}`);
    }
  };
  if (expected.shape !== undefined && result.shape !== expected.shape) {
    problems.push(`shape: expected ${JSON.stringify(expected.shape)}, actual ${JSON.stringify(result.shape)}`);
  }
  compare("urls", items.map((item) => pathOf(item.url)), expected.urls);
  compare("ads", (result.ads ?? []).map(pathOf), expected.ads);
  compare("thumbnails", items.map((item) => pathOf(item.thumbnail)), expected.thumbnails);
  compare("titles", items.map((item) => item.title), expected.titles);
  compare("indexes", items.map((item) => item.index ?? null), expected.indexes);
  for (const [field, wanted] of Object.entries(expected.page ?? {})) {
    const actual = result.page?.[field];
    if (actual !== wanted) {
      problems.push(`page.${field}: expected ${JSON.stringify(wanted)}, actual ${JSON.stringify(actual)}`);
    }
  }
  const shapes = (result.groups ?? []).map((group) => group.shape);
  for (const shape of expected.groupsInclude ?? []) {
    if (!shapes.includes(shape)) problems.push(`groups lack ${shape} (groups: ${JSON.stringify(shapes)})`);
  }
  for (const prefix of expected.groupsExclude ?? []) {
    const hit = shapes.find((shape) => shape.startsWith(prefix));
    if (hit) problems.push(`groups contain ${hit}, which starts with ${prefix}`);
  }
  return problems;
}

const browser = findBrowser();
if (!browser) {
  console.error("No Edge or Chrome found; set COLLECTOR_BROWSER to a Chromium browser.");
  process.exit(2);
}
const server = await serve();
const base = `http://127.0.0.1:${server.address().port}`;
let failures = 0;
let passes = 0;
try {
  for (const name of readdirSync(fixtures).filter((file) => file.endsWith(".html")).sort()) {
    const expectedFile = join(fixtures, name.replace(/\.html$/, ".expected.json"));
    if (!existsSync(expectedFile)) continue;
    for (const [index, expected] of JSON.parse(readFileSync(expectedFile, "utf8")).entries()) {
      const label = `${name} #${index + 1}${expected.query ? ` (${expected.query})` : ""}`;
      const dom = await dumpDom(browser, `${base}/${name}${expected.query ? `?${expected.query}` : ""}`);
      const match = dom.match(/<pre id="vdl-result">([\s\S]*?)<\/pre>/);
      const problems = match ? check(JSON.parse(decodeEntities(match[1])), expected) : ["no scan result in the page"];
      if (problems.length) {
        failures += 1;
        console.log(`FAIL ${label}`);
        for (const problem of problems) console.log(`    ${problem}`);
      } else {
        passes += 1;
        console.log(`ok   ${label}`);
      }
    }
  }
} finally {
  server.close();
}
console.log(`\n${passes} passed, ${failures} failed`);
process.exit(failures ? 1 : 0);
