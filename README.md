# Citation Phylogeny (journal_app)

論文引用関係系統樹可視化アプリ（スタンドアロン版）。Cloudflare Workers + D1 で動いていた
PoC（`journal/`）を、Tauri + Rust + SQLite 構成に作り直したもの。詳細な方針は
[docs/native.md](./docs/native.md) を参照。

## 必要なもの

- Node.js / npm
- Rust（[rustup](https://rustup.rs/) 推奨）
- macOS の場合は Xcode Command Line Tools

## データの準備

このアプリは `src-tauri/resources/db/` に置かれた5つの SQLite ファイルを読み込んで動作する。

```
src-tauri/resources/db/
  field17_2006_2010.sqlite
  field17_2011_2015.sqlite
  field17_2016_2020.sqlite
  field17_2021_2025.sqlite
  field17_2026_2030.sqlite
```

これらのファイルは EC2 上で生成し、本リポジトリには含めていない（サイズが大きいため `.gitignore` 対象）。
入手方法・生成手順は [docs/CREATE_DB.md](./docs/CREATE_DB.md) を参照。既に配布された SQLite ファイルが
手元にある場合は、上記のパスへそのまま配置すればよい。

ファイルが無い状態で起動すると、各 API 呼び出しがエラーになる（UI 自体は起動する）。

## 起動方法（開発モード、推奨）

```bash
npm install
npm run tauri dev
```

初回は Rust のコンパイルが走るため 1〜2 分かかる。ビルドが終わるとアプリウィンドウが自動で開く。
フロントエンド（`src/`）・バックエンド（`src-tauri/src/`）のコードを変更すると自動で再読み込みされる。
終了するにはターミナルで `Ctrl+C`。

## ビルドして `.app` として起動する場合

配布物に近い形で確認したい場合。

```bash
npm run tauri build -- --debug   # リリースビルドより高速（最適化なし）
open src-tauri/target/debug/bundle/macos/journal.app
```

本番相当のビルドをする場合は `--debug` を外す（最適化のぶん時間がかかる）。

```bash
npm run tauri build
```

macOS の場合、`.dmg` パッケージング（`hdiutil`/Finder 経由）がローカル環境によっては失敗することがあるが、
`.app` 自体は `src-tauri/target/release/bundle/macos/journal.app`（または `debug/bundle/...`）に生成されているので、
それを直接 `open` すれば動作確認できる。

## テスト

```bash
cd src-tauri
cargo test
```

`resources/db/*.sqlite` が実際に配置されている場合、それを使った統合テスト（`src/real_data_check.rs`）も
実行される。ファイルが無い環境では自動的にスキップされる。
