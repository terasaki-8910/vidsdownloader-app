<div align="right">

[![English](https://img.shields.io/badge/lang-English-2b7489?style=flat-square)](./README.en.md)

</div>

# vidsdownloader

yt-dlp が対応しているサイトから、動画と音声を保存する Windows / macOS 向けのデスクトップアプリ
（Tauri 2 + React）。個人用に作っているものです。

インストーラーと更新ファイルは、別のリポジトリ
[vidsdownloader-releases](https://github.com/terasaki-8910/vidsdownloader-releases) に置いています。

## できること

- キューと画質の指定（最高画質・1080p まで・720p まで・互換性優先・音声のみ・コメントだけ）
- アプリの中のブラウザでサイトにログインし、そのログインのまま保存する（Cookie はアプリの画面には渡しません）
- 一覧ページから動画の URL を集める。サイトごとの規則はユーザーが書き換えられ、サイト専用のコードは持ちません
- yt-dlp が扱えないページは、見えない窓で開いて、ページが読み込んだ動画（m3u8・mpd・mp4・webm）を探します
- ニコニコのコメントと bilibili の弾幕（動画なしでも取り出せます）
- 保存した動画の一覧、フォルダとファイル名の組み立て（OS の長さ制限の中で）
- アプリ内アップデート（署名付き）

## 決めていること

- yt-dlp・ffmpeg・Deno はアプリが初回に取得し、SHA-256 を確かめてから使います。PATH のものは使いません
- DRM の回避や、ペイウォール・アクセス制限の回避はしません
- アプリの中で開いた外部ページに、アプリの機能（IPC）は渡しません
- Cookie はジョブごとの一時ファイルとして渡し、終わったら消します。ユーザー自身の cookies.txt は書き換えません

## 作り方

必要なもの：Rust（stable）、Node.js、pnpm。

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

手元のビルドは `--config src-tauri/tauri.local.conf.json` を付けます（更新ファイルの署名鍵が要らなくなります）。

```sh
pnpm tauri build --debug --no-bundle --config src-tauri/tauri.local.conf.json
```

## 仕様と決定の記録

`SPEC.md`（範囲・設計・受け入れ条件・計画）と、作る前の確認シートは、作者の非公開リポジトリに置いています。
このリポジトリには、ビルドに必要なものと、エージェント向けの作業の決まり（`AGENTS.md`）が入っています。
