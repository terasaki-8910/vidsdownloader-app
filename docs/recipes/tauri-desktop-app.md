# Recipe: Tauri + pnpm desktop app

When to use this: SPEC calls for a cross-platform desktop GUI (macOS/Windows/Linux) with
a Node/TypeScript engine underneath. Read this during intake/design if the stack decision
lands here, and again before the release-automation step. Confirmed pattern from a real
project built on this template (claude-pipeline-template-voice-transcript, 2026-07);
every gotcha below cost real debugging time against a real CI matrix, not guessed.

## Stack shape
- pnpm workspace: a shared engine package with no UI/DB coupling, a thin CLI wrapper
  package (if there's also a CLI front end), and the Tauri app itself.
- Tauri v2. The Rust shell owns every privileged operation (filesystem, network, DB,
  secrets) -- the webview never touches them directly. The one safe exception: native
  file open/save dialogs (`@tauri-apps/plugin-dialog`), since they only return paths the
  user explicitly picked through an OS dialog, not arbitrary fs access.
- If the engine needs a full Node runtime (a DB client, existing Node-only business
  logic) that reimplementing in Rust isn't worth it, run it as a **Node sidecar**: bundle
  with esbuild (`--format=cjs`), package with `@yao-pkg/pkg` (not the original `pkg` --
  unmaintained, no prebuilt binaries past ~Node 18) into one executable per platform,
  declared under `tauri.conf.json`'s `bundle.externalBin`.

## Known gotchas (check these before they bite you)

1. **esbuild's CJS output silently empties `import.meta.url`.** Any code that resolves a
   path relative to itself (e.g. a sibling data folder) breaks once bundled for
   `@yao-pkg/pkg` -- esbuild *warns* about this at build time (`"import.meta" is not
   available with the "cjs" output format`); don't ignore that warning. Fix: pass such
   paths in explicitly (an env var set by Rust, or an esbuild text-loader static import)
   instead of computing them at runtime inside the bundle.
2. **Windows: `execFileSync("npx", ...)` fails with ENOENT.** `npx` resolves to
   `npx.cmd`, which needs `shell: true` to launch on Windows. Scope `shell: true` to
   `process.platform === "win32"` only -- elsewhere it just adds Node's DEP0190
   argument-escaping deprecation warning for no reason.
3. **tauri-cli's package-manager detection checks the app's OWN directory, not the
   workspace root**, for a lockfile -- silently falls back to npm in a pnpm workspace
   without a (even empty stub) `pnpm-lock.yaml` right there. Fix: commit a stub
   `pnpm-lock.yaml` in the app's own folder plus an explicit `"packageManager":
   "pnpm@<version>"` in its `package.json` (github.com/tauri-apps/tauri/issues/12706).
4. **`tauri.conf.json` needs an explicit `bundle.icon` array.** Without one, macOS's
   `.app`/`.dmg` bundler is lenient, but Linux's AppImage bundler panics ("couldn't find
   a square icon") and Windows' WiX/MSI bundler errors ("Couldn't find a .ico icon") --
   even with a complete `icons/` folder. List the standard set explicitly: `32x32.png`,
   `128x128.png`, `128x128@2x.png`, `icon.icns`, `icon.ico`.
5. **A release workflow needs `permissions: contents: write`** on the job (or repo-wide),
   or the default `GITHUB_TOKEN` 403s on release creation ("Resource not accessible by
   integration") -- even after the build itself succeeds on all platforms.
6. **macOS Gatekeeper: "\<App\> is damaged and can't be opened."** Without a paid Apple
   Developer ID + notarization (real money, may be out of scope), a browser-downloaded
   (quarantined) unsigned/ad-hoc-signed build gets this *misleading* message, not the
   older "unidentified developer, open anyway" prompt. State the tradeoff plainly in the
   README rather than silently shipping a broken-looking release; the practical
   workaround is `xattr -cr "/Applications/<App>.app"`. Windows may show an "unknown
   publisher" SmartScreen warning (Run anyway still works); Linux's `.AppImage`/`.deb`
   need no signature at all.
7. **The native OS menu bar does not automatically follow an in-app language/i18n
   toggle.** It's built once in Rust at startup -- a separate surface from the webview,
   easy to forget when testing only the visible UI. If the project has both a native menu
   and an i18n setting, wire an explicit rebuild-on-change (a Tauri command that
   rebuilds/swaps the `Menu` via `AppHandle::set_menu`, called from the app's top-level
   component on every language change -- including once on mount, to sync a persisted
   preference saved from a previous session). When adding i18n to a Tauri app, audit
   every "outside the webview" surface this way, not just the obvious one: menu bar, tray
   icon tooltip, window title, native notifications.
8. **A resource file a bundled sidecar needs at runtime (e.g. DB migration SQL) is not
   automatically "next to" the packaged binary.** Declare it under `tauri.conf.json`'s
   `bundle.resources`, copy it there as part of the same build step that produces the
   sidecar binary, and have Rust resolve the real path (`app.path().resource_dir()`) and
   pass it to the sidecar via an env var -- don't assume a relative-path trick survives
   `@yao-pkg/pkg` bundling (see gotcha 1). If the project also needs an ORM/lint rule
   banning raw SQL outside migration files, prefer the ORM's own migration runner (e.g.
   drizzle-orm's `migrate()`) over hand-rolled schema-creation SQL, so that hygiene rule
   stays honest.

## Reusable supporting patterns

- A **capability-reviewer subagent** that checks any diff to `capabilities/*.json` or
  `tauri.conf.json` against the actual regenerated ACL manifest -- catches over-broad
  grants, and confirms custom `#[tauri::command]`s need no `capabilities.json` entry at
  all (only plugin-exposed `<plugin>:<permission>` identifiers do).
- A **command-scaffold skill** for consistently wiring a new Rust `#[tauri::command]`
  together with its typed TS `invoke()` wrapper, so the JSON shape stays hand-synced
  correctly on both sides (there's no codegen for this by default).
- For screenshots in a README: capture real, privacy-checked screenshots from a running
  build rather than mocking up UI. On macOS,
  `CGWindowListCopyWindowInfo` + `screencapture -x -l<windowID>` captures one exact
  window precisely (never a broad screen capture that could show unrelated content);
  disambiguate between multiple same-named running instances by PID
  (`first process whose unix id is <pid>` in AppleScript, not `tell process "name"`).
