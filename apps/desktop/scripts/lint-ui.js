#!/usr/bin/env node
// Fails the build if any file under src/ (other than src/styles/tokens.css)
// contains a hex color or rgb()/hsl() literal, or if ANY file under src/
// contains an emoji. Run via `pnpm lint:ui`.
import { readFileSync, readdirSync, statSync } from "node:fs";
import { extname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = fileURLToPath(new URL(".", import.meta.url));
const projectRoot = join(scriptDir, "..");
const srcDir = join(projectRoot, "src");

const TOKENS_FILE = join("src", "styles", "tokens.css").split("\\").join("/");
const TEXT_EXTENSIONS = new Set([".ts", ".tsx", ".css", ".html", ".json"]);

// #abc / #abcd / #aabbcc / #aabbccdd, as a color literal (word-boundary
// delimited so it doesn't fire on private class fields like #myField).
const HEX_COLOR = /#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})\b/g;
const FUNCTIONAL_COLOR = /\b(?:rgb|rgba|hsl|hsla)\s*\(/gi;
// Unicode's own "is this an emoji" property -- does not match CJK text
// (kanji/hiragana/katakana), unlike naive surrogate-pair ranges would.
const EMOJI = /\p{Extended_Pictographic}/u;

function* walk(dir) {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    const stat = statSync(full);
    if (stat.isDirectory()) {
      yield* walk(full);
    } else {
      yield full;
    }
  }
}

function toPosix(p) {
  return p.split("\\").join("/");
}

let violations = [];

for (const file of walk(srcDir)) {
  if (!TEXT_EXTENSIONS.has(extname(file))) continue;
  const relPath = toPosix(relative(projectRoot, file));
  const text = readFileSync(file, "utf8");
  const lines = text.split("\n");

  const checkHexAndFunctional = relPath !== TOKENS_FILE;

  lines.forEach((line, idx) => {
    if (EMOJI.test(line)) {
      violations.push({ file: relPath, line: idx + 1, rule: "emoji", snippet: line.trim() });
    }
    if (checkHexAndFunctional) {
      const hexMatches = line.match(HEX_COLOR);
      if (hexMatches) {
        violations.push({
          file: relPath,
          line: idx + 1,
          rule: `hex color literal (${hexMatches.join(", ")})`,
          snippet: line.trim(),
        });
      }
      const fnMatches = line.match(FUNCTIONAL_COLOR);
      if (fnMatches) {
        violations.push({
          file: relPath,
          line: idx + 1,
          rule: `rgb()/hsl() literal (${fnMatches.join(", ")})`,
          snippet: line.trim(),
        });
      }
    }
  });
}

if (violations.length > 0) {
  console.error(`lint:ui found ${violations.length} violation(s):\n`);
  for (const v of violations) {
    console.error(`  ${v.file}:${v.line}  [${v.rule}]\n    ${v.snippet}`);
  }
  process.exit(1);
}

console.log("lint:ui: no hex/rgb/hsl literals outside tokens.css, no emoji under src/.");
