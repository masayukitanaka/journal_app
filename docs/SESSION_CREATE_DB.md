# SQLite データベース生成 作業記録（SESSION_CREATE_DB.md）

docs/CREATE_DB.md の手順を EC2 上で実行した記録。作業フォルダのログ（`data/openalex/work/create_db*.log`）は git 管理外で EC2 とともに消えるため、主要な経緯・数値をここに残す。

## 1. 環境

| 項目 | 内容 |
|---|---|
| インスタンス | EC2 `us-east-1`、4 vCPU・メモリ 15GB・EBS 200GB、Amazon Linux 2023（kernel 6.18） |
| ツール | Python 3.9.25、duckdb 1.4.5（pip）、httpfs・sqlite 拡張（`INSTALL` で取得）、sqlite3 CLI、aws CLI |
| スナップショット | OpenAlex works 2026-10-14（`data/openalex/manifest.json`、1,898 ファイル、744 GB。S3 上の最新版と同一） |
| 条件 | field 17、出版年 2006〜2026、is_xpac 除外、DB は 2011 起点 5 年区切り |

## 2. 作業の経緯

### 2.1 事前確認・スクリプト整備（2026-11-15 06:50〜07:05）

- CREATE_DB.md（ドラフト）の実行可否を確認。ドキュメントとの差異として、DuckDB の httpfs 拡張が pip に同梱されていない（stage が全ファイル失敗）、`pipeline.status` は `stage.py` 単体では作られない、sqlite3 CLI が未導入、を確認。
- export 方式のベンチマーク（1,000 万行の edges）: インデックス先作り 58.1 秒 → 主キー順ソート投入＋インデックス後作り 12.7 秒。後者を採用。
- Cloudflare 版 `journal` から `stage.py`・`build.py`・`manifest.json` を複製し、`scripts/db/` に `run_create_db.sh`・`export_sqlite.py` を追加。journal_app 単体で完結する構成にした。
- `build.py` の authorships を著者順（pos）保持に修正（Cloudflare 版は `list_distinct` で順序が崩れていた）。
- 小規模データ（S3 最新 2 ファイル、21,587 件）で全工程を実行し、5 DB とも検証 OK。著者順は元データ 200 件と一致。`../journal` を退避した状態でも動作することを確認。

### 2.2 本番実行（2026-11-15 07:05 開始）

```bash
setsid nohup scripts/db/run_create_db.sh > /dev/null 2>&1 < /dev/null &
```

（進行に合わせて追記）

### 2.3 保存先の追加（2026-11-15 07:40、本番 stage 実行中）

- 生成物は S3 `s3://970173663066-public-us-east-1/journal_app/` に 5 ファイルのまま保存し、アプリがそこから取得する方針になった（1 ファイルへの統合は行わない。`DB_REGISTRY` は変更不要）。
- `scripts/db/upload_s3.sh`（SHA256SUMS 作成・アップロード・サイズ照合）を追加し、`run_create_db.sh` に `UPLOAD=1` オプションを追加。実行中のパイプラインに影響しないよう、`run_create_db.sh` は新ファイルを作って `mv` で置き換えた（実行中の bash は旧ファイルを読み続ける）。今回の本番実行は旧版で動いているため、アップロードは完了後に `upload_s3.sh` を個別に実行する。
- この EC2 には AWS 認証情報が無い（`aws sts get-caller-identity` が NoCredentials）。アップロードには書き込み権限のあるインスタンスロールの付与が必要。
