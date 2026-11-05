# Project: vidsdownloader

Shared instructions for every coding agent on this repository (Claude Code and Codex).
Claude-only additions live in `CLAUDE.md`, which imports this file.

## What this is
A desktop app for Windows and macOS (Tauri 2) that downloads video and audio from the
sites yt-dlp supports. Beyond a queue and quality presets it covers what existing GUIs
don't for this user: logging in to sites from inside the app, collecting video URLs
from channels and listing pages through reusable rules (no per-site code, no LLM per
download), user-editable folder and file naming kept within OS limits, niconico
comments, and a library of what was saved. yt-dlp, ffmpeg and Deno are managed by the
app: downloaded and SHA-256-verified on first run, never bundled. Personal use first;
installers and update files go to a separate public releases repository.

## Source of truth
- `SPEC.md` holds scope, architecture, acceptance criteria and the plan. Read it before
  any work. Agreed decisions are appended to its "Decisions" section, dated, in the
  user's words; a later heading overrides an earlier one. Don't start competing
  planning documents.
- Choices that belong to the user (the reading of an ambiguous request, anything the
  user will see, scope, open tradeoffs, conflicting instructions) are settled before
  anything is built, following `docs/decision-sheet.md`. Nothing is built without a GO
  recorded in SPEC.md.

## Workflow
- One feature per branch, each in its own worktree under
  `../vidsdownloader.worktrees/<name>`; merge to `main` with `--no-ff` once its checks
  pass. Don't parallelize work that shares files or has a dependency order.
- Prototypes and decision-sheet samples stay off `main` (`spike/*` branches).

## Stack
Tauri 2.12.x (not Tauri 3, still alpha), a Rust core crate with no Tauri dependency,
React + Vite + TypeScript for the app UI, pnpm, SQLite. Before Tauri work read
`docs/recipes/tauri-desktop-app.md` and `~/.claude/rules/tauri.md`.

## Boundaries that must hold
- External pages opened in the in-app browser never get IPC: no remote URLs in any
  capability. Results are pulled by the Rust side, and the user confirms before
  anything is queued.
- Tools are never taken from PATH; every downloaded tool is SHA-256-verified.
- Cookies never reach the app's webview. Per-job cookie files are temporary,
  user-readable only, and deleted after the job; a user's own cookies.txt is never
  modified.
- No DRM circumvention and nothing that defeats paywalls or access controls.
- No site-specific code compiled into the app; site knowledge lives in yt-dlp and in
  user-editable rules.

## Language
Code, comments, docs and commit messages in English. UI copy in Japanese and English
(switchable). Chat follows the user.

## Commands
From the repository root (Cargo workspace: `crates/core`, `apps/desktop/src-tauri`):
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all --check`

In `apps/desktop` (pnpm):
- `pnpm typecheck` — TypeScript, no emit
- `pnpm lint:ui` — no hex/rgb/hsl literals outside `src/styles/tokens.css`, no emoji
  under `src/`
- `pnpm test:collector` — the in-app browser's collector script on the pages in
  `collector-fixtures/`, in headless Edge (needs Edge or Chrome; it cannot start inside
  a sandbox that forbids child processes)
- `pnpm tauri dev` — run the app (Vite on port 1420); `pnpm tauri build --debug
  --no-bundle --config src-tauri/tauri.local.conf.json` — a debug exe with the UI embedded.
  Every local build passes `--config src-tauri/tauri.local.conf.json`: it turns the updater
  artifacts off (`bundle.createUpdaterArtifacts: false`), which would otherwise need the
  signing key
- `pnpm dev` in a browser with `#mockUpdate=available` (other values in
  `src/lib/debugHash.ts`) — the update dialog and Settings > アップデート on the mock

Releases are built only by `.github/workflows/release.yml`, on a pushed `v*` tag (or run by
hand), never on this PC: it builds the Windows and macOS installers, the signed updater
bundles and `latest.json` and publishes them to `terasaki-8910/vidsdownloader-releases`. For a
new version, set it in `apps/desktop/src-tauri/tauri.conf.json` (and `Cargo.toml`,
`package.json`), add `docs/release-notes/<version>.md` ("- " lines, shown in the update
dialog) and tag `v<version>`. `.github/workflows/ci.yml` runs the checks above on Windows and
macOS for every push and pull request.

## UI rules
Read `~/.claude/rules/ui.md` before UI work. Colors only via design tokens, never
hardcoded hex; no emoji in UI or source; WCAG AA; full keyboard access.

## Do not touch
Design tokens (change only with the user's explicit approval); generated files.

## Git
Local only; never push unless asked. Commit work in progress early. No AI attribution
or session trailers in commits.
