# EC2 作業まとめ（SESSION_EC2.md）

2026-11-15 に EC2 上で行った SQLite データベース生成作業のまとめと、次回（他分野の取り込みなど）の作業手順。
このインスタンスは作業後に破棄する。ログ・中間データ（`data/openalex/work/`）と手元の `.sqlite` は消えるため、必要な情報はここと git に残す。

- 手順書: [docs/CREATE_DB.md](./CREATE_DB.md)（スクリプトの仕様・スキーマ・各ステップの詳細）
- 詳細な作業記録: [docs/SESSION_CREATE_DB.md](./SESSION_CREATE_DB.md)（時系列、DB ごとの数値、移動した論文の一覧、SHA256）

---

## 1. 成果物

S3 `s3://970173663066-public-us-east-1/journal_app/` に保存済み（アプリはここから取得する）。

| 分野 | ファイル | 論文数 | 合計サイズ |
|---|---|---|---|
| 17 Computer Science | `field17_2006_2010.sqlite` 〜 `field17_2026_2030.sqlite`（5 ファイル） | 11,438,011 | 12.47 GB |
| 22 Engineering | `field22_2006_2010.sqlite` 〜 `field22_2026_2030.sqlite`（5 ファイル） | 22,217,336 | 26.88 GB |
| — | `SHA256SUMS`（全分野で 1 つ。`sha256sum -c` 形式） | | |

- データ元: OpenAlex S3 スナップショット 2026-10-14（`data/openalex/manifest.json`、1,898 ファイル・744 GB）。両分野とも同じスナップショット。
- 条件: 出版年 2006〜2026、`is_xpac` 除外、2011 年起点の 5 年区切りで 1 分野 5 DB。
- どちらも出版年の誤りの修正（CREATE_DB.md 4 章）済み。field 17 は 24 件、field 22 は 2 件を移動。
- `src-tauri/resources/db/SHA256SUMS` は S3 上のものと同じ内容（git 管理）。

---

## 2. 作業の流れ（2026-11-15）

| 時刻 | 内容 |
|---|---|
| 06:50〜07:05 | CREATE_DB.md（ドラフト）の実行可否を確認。Cloudflare 版 `journal` リポジトリからスクリプト・manifest を複製し、journal_app 単体で完結する構成（`scripts/db/`）に整理。小規模データで全工程を確認 |
| 07:05〜08:04 | field 17 本番（stage 44.6 分・build 5 分・export 9 分）。件数が Cloudflare 版の実績と完全一致 |
| 08:04〜08:07 | S3 へアップロード（インスタンスロール付与後） |
| 17:37〜17:52 | 出版年の誤りの修正スクリプトを SQLite 用に移植して field 17 に適用（24 件移動）、S3 を更新 |
| 18:01〜20:28 | スクリプトを分野指定（`FIELD`）に対応させ、field 22 を通しで実行（stage 44 分・build 14 分・export 29 分・出版年修正 52 分・アップロード） |

主な改修（すべて `scripts/db/` にコミット済み）:

- `export_sqlite.py`: 主キー順ソート投入＋インデックス後作成（1,000 万行で 58 秒 → 12.7 秒）、stats.json との自動検証
- `build.py`: 著者順（`pos`）を保持（Cloudflare 版は `list_distinct` で順序が崩れていた）
- `fix_misyeared_papers.py`: D1 用から SQLite 用に書き直し（2 DB を ATTACH して 1 論文 1 トランザクション）
- `run_create_db.sh`: `FIELD`・`FIX_MISYEARED`・`UPLOAD` 等の環境変数で全工程を通し実行、status/log を出力
- `upload_s3.sh`: 分野単位でアップロードし、`SHA256SUMS` は S3 上の他分野の行を残して差し替え

---

## 3. 結果の数値

### 3.1 所要時間・リソース（4 vCPU・15 GB・EBS 200 GB）

| ステップ | field 17 | field 22 | 備考 |
|---|---|---|---|
| stage | 44.6 分 | 44.4 分 | 全 1,898 ファイルを読むため分野によらずほぼ一定（S3 の読み込みが律速） |
| build | 5 分 | 14 分 | 論文数・参考文献数に比例 |
| export_sqlite | 9 分 | 29 分 | edges の件数に比例 |
| fix_misyeared | 約 11 分 ※ | 52 分 | ほぼ検証（quick_check で全ファイルを読む）の時間。ディスク読み込みが 約 6.5 MB/s に制限されていた |
| upload_s3 | 約 2.5 分 | 約 6 分 | 同一リージョン内の S3 |
| **合計** | 約 1 時間 15 分 | 約 2 時間 30 分 | |

※ field 17 の修正は本番とは別に後から実行（API 判定 1 分・移動 1 秒・検証 10 分）。

| 項目 | field 17 | field 22 |
|---|---|---|
| メモリ最大 | 約 4.8 GB | 約 6.2 GB |
| 作業領域（`data/openalex/work/field{N}`）最大 | 約 13 GB | 約 28 GB |
| SQLite 出力 | 12.47 GB | 26.88 GB |

### 3.2 件数

| 項目 | field 17 | field 22 |
|---|---|---|
| papers | 11,438,011 | 22,217,336 |
| edges | 69,859,678 | 157,299,553 |
| うち cited が別 DB | 47,087,509 | 109,782,658 |
| 参考文献の総数 | 146,828,653 | 306,265,770 |
| authorships | 31,950,737 | 76,783,433 |
| paper_keywords | 70,267,280 | 145,687,937 |
| keywords（異なり語数） | 884,807 | 1,130,920 |
| 重複 id | 0 | 0 |

field 22 の DB 別内訳:

| DB | papers | edges | authorships | keywords | paper_keywords | サイズ |
|---|---|---|---|---|---|---|
| field22_2006_2010 | 4,020,113 | 3,920,945 | 10,793,729 | 733,822 | 23,592,311 | 3.36 GB |
| field22_2011_2015 | 5,205,385 | 17,769,296 | 14,978,833 | 811,953 | 32,141,122 | 5.02 GB |
| field22_2016_2020 | 5,551,040 | 41,668,932 | 18,663,009 | 855,287 | 37,024,021 | 6.90 GB |
| field22_2021_2025 | 5,803,082 | 80,214,629 | 23,143,056 | 865,394 | 43,103,968 | 9.33 GB |
| field22_2026_2030 | 1,637,716 | 13,725,751 | 9,204,806 | 570,416 | 9,826,515 | 2.27 GB |

（出版年修正前の値。修正で DB 間を移動したのは 2 件のみ。field 17 の内訳は SESSION_CREATE_DB.md 2.2）

### 3.3 出版年の誤りの修正（`cit >= 1000` が対象）

| 項目 | field 17 | field 22 |
|---|---|---|
| 確認した論文 | 5,011 | 4,745 |
| 移動 | 24 | 2 |
| 正しい年が範囲外（移動せず） | 2 | 3 |
| API で見つからず | 11 | 3 |
| マージ済み Work | 99 | 30 |
| API リクエスト | 200 | 125 |

- field 17 の主な移動: Attention Is All You Need（2025 → 2017）、Adam（2026 → 2015）、Auto-Encoding Variational Bayes（2025 → 2013）ほか（一覧は SESSION_CREATE_DB.md 2.5）
- field 22 の移動: "YOLOv3: An Incremental Improvement"（W4293584584、2024 → 2018）、"Sparse and Redundant Representations"（W1591116419、2011 → 2010）
- field 22 の範囲外: Microflows and Nanoflows（2005）、Low-power CMOS digital design（1992）、W850470413（1979）

---

## 4. 次回: 他分野を取り込む手順

### 4.1 規模の目安

OpenAlex API の分野別件数（出版年 2006〜2026、xpac 除外、2026-11-15 時点）。スナップショットから抽出した実件数は API より多めになる（field 17 は API 966 万件に対し 1,144 万件、field 22 は 1,788 万件に対し 2,222 万件、およそ 1.2 倍）。

| ID | 分野 | 件数（API） | | ID | 分野 | 件数（API） |
|---|---|---|---|---|---|---|
| 33 | Social Sciences | 31,696,406 | | 31 | Physics and Astronomy | 4,855,085 |
| 27 | Medicine | 29,572,963 | | 16 | Chemistry | 3,892,076 |
| 12 | Arts and Humanities | 18,749,004 | | 19 | Earth and Planetary Sciences | 3,553,795 |
| **22** | **Engineering（済）** | 17,881,877 | | 26 | Mathematics | 2,568,730 |
| 11 | Agricultural and Biological Sciences | 13,089,153 | | 28 | Neuroscience | 2,123,205 |
| **17** | **Computer Science（済）** | 9,657,318 | | 36 | Health Professions | 2,088,139 |
| 14 | Business, Management and Accounting | 7,956,494 | | 21 | Energy | 1,997,423 |
| 23 | Environmental Science | 7,553,573 | | 24 | Immunology and Microbiology | 1,764,665 |
| 13 | Biochemistry, Genetics and Molecular Biology | 6,647,837 | | 18 | Decision Sciences | 1,647,380 |
| 32 | Psychology | 5,051,420 | | 35 | Dentistry | 1,105,854 |
| 25 | Materials Science | 4,933,115 | | 29 | Nursing | 1,046,835 |
| 20 | Economics, Econometrics and Finance | 4,930,226 | | 30 | Pharmacology, Toxicology and Pharmaceutics | 878,824 |
| | | | | 15 | Chemical Engineering | 737,961 |
| | | | | 34 | Veterinary | 257,349 |

見積もりの目安（field 22 の実績から論文数に比例させた概算。edges の密度は分野で大きく違うので幅を持たせること）:

- stage: 分野によらず 約 45 分（全スナップショットを読むため）
- build・export・検証: 論文 1,000 万件あたり それぞれ 約 6 分・約 13 分・約 23 分
- SQLite: 論文 1,000 万件あたり 約 12 GB、作業領域はその同程度
- Medicine（27）や Social Sciences（33）は field 22 の 1.7〜1.8 倍で、出力 50 GB 前後・全体 4 時間程度を見込む。build の `--memory-limit 5GB` で溢れた分はディスクにスピルされる

### 4.2 手順チェックリスト

1. **EC2 を起動**: `us-east-1`、Amazon Linux 2023、4 vCPU・16 GB 程度、EBS は出力＋作業領域の 3 倍程度（大きい分野なら 200 GB 以上）。gp3 で IOPS・スループットを上げておくと検証が速い（5.4）
2. **IAM ロールを付与**: `970173663066-public-us-east-1` の `journal_app/*` に対する `s3:PutObject`・`s3:GetObject`・`s3:DeleteObject`（今回は `ec2-s3-journal-app`）。`aws sts get-caller-identity` で確認
3. **準備**:
   ```bash
   sudo dnf install -y python3 python3-pip sqlite git
   pip3 install duckdb requests
   git clone git@github.com:masayukitanaka/journal_app.git && cd journal_app
   ```
4. **スナップショットを確認**: `aws s3 ls --no-sign-request s3://openalex/data/parquet/works/ | tail -3` で最新の更新日を見る。同梱の manifest（2026-10-14）のファイルが S3 から消えていたら `FETCH_MANIFEST=1` で最新を取得する（既存分野と年代がずれる点に注意。取得した manifest はコミットする）
5. **小規模で動作確認**（1 分程度）:
   ```bash
   FIELD=<ID> WORK=/tmp/t/work OUT=/tmp/t/db STAGE_ARGS="--limit-files 2" scripts/db/run_create_db.sh
   cat /tmp/t/work/create_db.status   # DONE
   ```
6. **本番を切り離して起動**:
   ```bash
   FIELD=<ID> UPLOAD=1 setsid nohup scripts/db/run_create_db.sh > /dev/null 2>&1 < /dev/null &
   ```
7. **進捗確認**:
   ```bash
   cat data/openalex/work/field<ID>/create_db.status         # RUNNING <step> / DONE / FAILED <step>
   tail -f data/openalex/work/field<ID>/create_db.log         # stage は 1 ファイルごとに残り時間を表示
   tail -f data/openalex/work/field<ID>/create_db_mem.log     # 30 秒ごとのメモリ・ディスク
   ```
   出版年修正の検証も DB ごと・処理ごとにログが出る（2026-11-15 以降のスクリプト）。quick_check 中は `/proc/<pid>/io` の `read_bytes` で読み込み量を確認できる
8. **結果を確認**: `create_db.log` の「全 DB 検証OK」「検証OK（…）」「OK <ファイル> <bytes>」、`out/stats.json` の件数。`fix_misyeared_plan.json` の移動内容
9. **アプリ側**: `src-tauri/src/db.rs` の `DB_REGISTRY` に `field<ID>_*.sqlite` の 5 行を追加（4.3）
10. **記録を残す**: 本書・SESSION_CREATE_DB.md に数値（stats.json の要約、所要時間、移動した論文）を追記してコミット・push。`data/openalex/work/` のログは EC2 とともに消える
11. **EC2 を終了**: push 済みと S3 のファイル・`SHA256SUMS` を確認してから

### 4.3 アプリ側でまだ必要な作業（2026-11-15 時点）

- `DB_REGISTRY`（`src-tauri/src/db.rs`）は field 17 の 5 ファイルのみ。field 22 の 5 行は未追加
- S3 の `journal_app/*` は匿名では取得できない（403）。アプリが認証なしで取得するならバケットポリシーで `s3:GetObject` を公開する
- 取得処理は 1 分野で十数〜数十 GB になるため、中断・再開（HTTP Range）と `SHA256SUMS` での検証を推奨

---

## 5. 注意点・教訓

1. **DuckDB の拡張は pip に同梱されていない**: `httpfs`・`sqlite` は初回に `INSTALL` が必要（`run_create_db.sh` が自動で行う）。無いと stage が全ファイル失敗する
2. **OpenAlex S3 は匿名で読める**: stage〜export に AWS 認証は不要。必要なのはアップロードだけ
3. **SQLite のインデックスは投入後に作る**: インデックスを先に作ると約 4.5 倍遅い。主キーを持つテーブルは主キー順に並べて投入する
4. **ディスクの読み込みが遅いと検証が長引く**: 再起動直後などページキャッシュが冷えた状態では読み込みが 約 6.5 MB/s しか出ず、quick_check（全ファイル読み）に field 22 で 50 分かかった。全行の NOT EXISTS 結合のような検証は避け、移動した行だけを個別に確認する設計にしている。次回は EBS のスループット・IOPS を上げる（gp3 で 250 MB/s 以上など）と大きく短縮できるはず
5. **長時間処理は切り離して起動する**: `setsid nohup ... &`。今回も Claude Code のセッションが途中で切れたが、処理は止まらなかった
6. **実行中のシェルスクリプトを直接編集しない**: bash は実行中にスクリプトを逐次読むため。変更が必要なら新しいファイルを作って `mv` で置き換える（Python スクリプトは起動時に読み込まれるので、起動前の次ステップなら編集が効く）
7. **OpenAlex API はキー無しで足りる**: 無料枠 $0.10/日（同じ IP で共有）。出版年修正は 1 分野 100〜200 リクエスト（約 $0.01〜0.02）。`OPENALEX_API_KEY` があれば使う
8. **IAM には DeleteObject も入れておく**: 書き込み確認用のテストファイルを消せず、後から追加した
9. **`SHA256SUMS` は全分野共通**: `upload_s3.sh` が S3 上の既存行を残して当該分野だけ差し替える。手でアップロードする場合は他分野の行を消さないこと
10. **件数の再現性**: 同じスナップショット・同じ条件なら件数は Cloudflare 版と完全一致した（field 17）。スナップショットが変わると一致しなくなる

---

## 6. 既知の制約・残課題

- 出版年の誤りは `cit >= 1000` の論文しか確認していない。移動した論文だけ `cit` が API の最新値（他はスナップショット時点）
- マージ済みの重複 Work は両方残る（例: "Attention Is All You Need" W2626778328 と W4385245566）
- 正しい年が 2005 年以前の論文は移動せず元の DB に残る
- `VACUUM` は未実施（主キー順の新規投入なので断片化は小さい想定）
- 2026-11-15 の field 17 の作業フォルダは `data/openalex/work/` 直下（分野指定対応前）、field 22 は `data/openalex/work/field22/`。次回からは `field{ID}/`
