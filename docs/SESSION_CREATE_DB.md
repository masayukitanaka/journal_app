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

| ステップ | 時刻 | 所要 | 結果 |
|---|---|---|---|
| stage | 07:05〜07:50 | 44.6 分 | 1,898 ファイル、失敗 0、11,438,011 件。メモリ 約 4.7GB で安定 |
| build | 07:50〜07:55 | 5 分 | 重複 id 0、参考文献 146,828,653 件、edges 69,859,678 件（うち別 DB 参照 47,087,509）、keywords 異なり語 884,807。checks・`child_cnt_sum_minus_edges` すべて 0 |
| export_sqlite | 07:55〜08:04 | 9 分 | 5 DB とも検証 OK（件数・dangling・checks・quick_check） |

| DB | papers | edges | authorships | keywords | paper_keywords | SQLite サイズ | export 所要 |
|---|---|---|---|---|---|---|---|
| field17_2006_2010 | 1,932,850 | 3,072,398 | 4,426,213 | 443,391 | 10,565,552 | 1.53 GB | 60 秒 |
| field17_2011_2015 | 2,436,353 | 10,291,552 | 5,850,778 | 518,022 | 13,771,688 | 2.22 GB | 87 秒 |
| field17_2016_2020 | 2,693,374 | 19,159,162 | 7,319,358 | 575,258 | 16,011,926 | 2.99 GB | 124 秒 |
| field17_2021_2025 | 3,340,772 | 32,930,571 | 11,148,218 | 636,741 | 22,825,086 | 4.53 GB | 183 秒 |
| field17_2026_2030 | 1,034,662 | 4,405,995 | 3,206,170 | 411,488 | 7,093,028 | 1.19 GB | 55 秒 |
| **合計** | **11,438,011** | **69,859,678** | **31,950,737** | — | **70,267,280** | **12.46 GB** | 9 分 |

- **全テーブルの件数が Cloudflare 版の実績（CREATE_DB.md 6 章）と完全に一致**。各 DB の `child_cnt` 合計・被引用数合計も Cloudflare 版の投入直後の検証値と一致（例: field17_2016_2020 は 22,963,322 / 32,317,917）。
- 合計サイズ 12.46 GB（D1 の 13.28 GB より小さい）。作業領域は最大 約 12.9 GB（stage 2.4GB・out 2.1GB・build.duckdb 等）。
- 抜き取り確認: 著者順は OpenAlex の順（例: "Deep Residual Learning for Image Recognition" → Kaiming He, Xiangyu Zhang, Shaoqing Ren, Jian Sun）。hubs 用クエリは `idx_papers_field_childcnt` を使用。
- **既知の出版年の誤りは残っている**: "Attention Is All You Need"（W2626778328、被引用 24,487）が year 2025.58 で field17_2021_2025 に入っている（CREATE_DB.md 4 章、未対応）。

### 2.3 保存先の追加（2026-11-15 07:40、本番 stage 実行中）

- 生成物は S3 `s3://970173663066-public-us-east-1/journal_app/` に 5 ファイルのまま保存し、アプリがそこから取得する方針になった（1 ファイルへの統合は行わない。`DB_REGISTRY` は変更不要）。
- `scripts/db/upload_s3.sh`（SHA256SUMS 作成・アップロード・サイズ照合）を追加し、`run_create_db.sh` に `UPLOAD=1` オプションを追加。実行中のパイプラインに影響しないよう、`run_create_db.sh` は新ファイルを作って `mv` で置き換えた（実行中の bash は旧ファイルを読み続ける）。今回の本番実行は旧版で動いているため、アップロードは完了後に `upload_s3.sh` を個別に実行する。
- この EC2 には AWS 認証情報が無い（`aws sts get-caller-identity` が NoCredentials）。アップロードには書き込み権限のあるインスタンスロールの付与が必要。

### 2.4 S3 への保存（2026-11-15 08:04〜08:07）

- EC2 にインスタンスロール `ec2-s3-journal-app` が付与された（PutObject・GetObject、後に DeleteObject を追加）。書き込み確認用の `journal_app/_write_test.txt` は削除済み。
- 本番実行は `UPLOAD` オプション追加前の版で起動していたため、export 完了を待って `scripts/db/upload_s3.sh` を切り離して実行した（ログ `data/openalex/work/upload_s3.log`）。所要 約 2.5 分、6 ファイルともサイズ照合 OK。

```
2026-11-15 07:26:22          0 
2026-11-15 08:07:01        455 SHA256SUMS
2026-11-15 08:05:18 1534185472 field17_2006_2010.sqlite
2026-11-15 08:05:31 2221461504 field17_2011_2015.sqlite
2026-11-15 08:05:49 2994405376 field17_2016_2020.sqlite
2026-11-15 08:06:14 4532142080 field17_2021_2025.sqlite
2026-11-15 08:06:51 1185673216 field17_2026_2030.sqlite
```

SHA256SUMS:

```
5a472ef73f8b4ca98484ce2e2891853f9d5eeac85c13d334beaa6d83d55988c2  field17_2006_2010.sqlite
2457d70f5eacf69e965166056185ffb3684f26d21b9ec76b9aa49fd82972aaa8  field17_2011_2015.sqlite
f8e3dab56ffc5bef8c327a2acffce4946e9628ce7b1fa9c49de6b92dffe1233a  field17_2016_2020.sqlite
2b9f00d9f5fdba2cc6ed6a29faf1a6fb09a0da1120962201b90a8afe6594db25  field17_2021_2025.sqlite
74750a8ccacbe15cf78fa565f5f2a08848aa7cfd0778bee39f3bdbd68c4bb254  field17_2026_2030.sqlite
```

- 匿名アクセスでの取得は 403（`journal_app/*` は公開読み取りになっていない）。アプリが匿名で取得する場合はバケットポリシーの設定が必要。

## 3. 残作業

- アプリ側: S3 からの取得処理（中断・再開と SHA256SUMS による検証）。匿名取得にするならバケットポリシーで `journal_app/*` の `s3:GetObject` を公開する。
- 出版年の誤りの修正（CREATE_DB.md 4 章）は未実施。
- EC2 を終了すると `data/openalex/work/`（ログ・中間データ）と `src-tauri/resources/db/*.sqlite` は消える。SQLite は S3 にあり、中間データは同じ manifest から再生成できる（約 1 時間）。
