<div align="right">

[![日本語](https://img.shields.io/badge/lang-日本語-2b7489?style=flat-square)](./README.md)

</div>

# vidsdownloader

A desktop app for Windows and macOS (Tauri 2 + React) that saves video and audio from the sites
yt-dlp supports. A personal project.

Installers and update files live in a separate repository,
[vidsdownloader-releases](https://github.com/terasaki-8910/vidsdownloader-releases).

## What it does

- A download queue with quality presets (best, up to 1080p, up to 720p, H.264 for compatibility,
  audio only, comments only)
- Signing in to sites in the in-app browser and downloading with that login (cookies never reach
  the app's own UI webview)
- Collecting video URLs from listing pages through user-editable rules; no site-specific code
- For pages yt-dlp cannot read, a hidden window opens the page and looks for the media it loads
  (m3u8, mpd, mp4, webm)
- niconico comments and bilibili danmaku, with or without the video
- A library of what was saved, and user-editable folder and file naming within OS limits
- Signed in-app updates

## Boundaries

- yt-dlp, ffmpeg and Deno are downloaded on first run and verified by SHA-256; never taken from PATH
- No DRM circumvention, and nothing that defeats paywalls or access controls
- External pages opened in the in-app browser never get IPC
- Per-job cookie files are temporary and deleted afterwards; a user's own cookies.txt is never modified

## Building

Requires Rust (stable), Node.js and pnpm.

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check

cd apps/desktop
pnpm install
pnpm typecheck
pnpm lint:ui
pnpm tauri dev
```

Local builds pass `--config src-tauri/tauri.local.conf.json`, which turns off the updater
artifacts so no signing key is needed.

```sh
pnpm tauri build --debug --no-bundle --config src-tauri/tauri.local.conf.json
```

## Spec and decision record

`SPEC.md` (scope, architecture, acceptance criteria and the plan) and the decision sheets are
kept in the author's private repository. This repository holds what is needed to build the app,
plus the working rules for coding agents (`AGENTS.md`).
