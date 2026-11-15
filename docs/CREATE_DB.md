# EC2上でのSQLiteデータベース生成手順（CREATE_DB.md）

- ステータス: 2026-11-15 に本番規模で実行し、S3 に保存済み（実行記録は docs/SESSION_CREATE_DB.md）
- 対象: journal_app が使う SQLite ファイル（分野ごとに5つ、`field{分野}_2006_2010.sqlite` 〜 `field{分野}_2026_2030.sqlite`）をEC2上で生成する手順
- 生成済みの分野: 17 = Computer Science（2026-11-15）、22 = Engineering（2026-11-15）
- 必要なスクリプト・データはすべて本リポジトリに含まれる（`scripts/db/`・`data/openalex/manifest.json`）。Cloudflare版リポジトリ `journal` は不要

---

## 0. 要約

| 項目 | 内容 |
|---|---|
| 分野 | OpenAlex の Field ID を `FIELD` で指定（既定 17 = Computer Science、22 = Engineering など）。分野ごとに独立して生成する |
| 生成するファイル | 分野ごとに5つ。field 17 の例: `field17_2006_2010.sqlite` / `field17_2011_2015.sqlite` / `field17_2016_2020.sqlite` / `field17_2021_2025.sqlite` / `field17_2026_2030.sqlite`（合計 12.46GB、2026-11-15 実績） |
| データ元 | OpenAlex S3スナップショット（`s3://openalex/data/parquet/works/`、Parquet形式、CC0、認証不要） |
| 処理場所 | EC2インスタンス（OpenAlexのS3バケットと同じ`us-east-1`） |
| 処理の流れ | stage（S3→中間Parquet）→ build（DBごとのテーブルをParquetで出力）→ export_sqlite（SQLiteへ変換して検証） |
| 実行方法 | `scripts/db/run_create_db.sh` で通しで実行（目安 約1時間10分） |
| 保存先 | `s3://970173663066-public-us-east-1/journal_app/`（全分野の`.sqlite`と、全分野で1つの`SHA256SUMS`）。アプリはここから取得する（3.6） |

### ファイル構成

| パス | 内容 |
|---|---|
| `scripts/db/run_create_db.sh` | 全工程を通しで実行する。状態・ログを作業フォルダに出力 |
| `scripts/db/stage.py` | Step 1: S3のParquetを1ファイルずつDuckDB httpfsで直接読み（ダウンロードしない）、分野・出版年で絞った中間Parquetを出力。再開可能 |
| `scripts/db/build.py` | Step 2: 中間Parquetを結合し、DB（分野×5年）ごとの `papers`/`edges`/`authorships`/`keywords`/`paper_keywords` と `stats.json`（整合性チェック）を出力 |
| `scripts/db/export_sqlite.py` | Step 3: build の出力をSQLiteファイルへ変換し、`stats.json` と突き合わせて検証 |
| `scripts/db/fix_misyeared_papers.py` | Step 4: 出版年の誤りで違う年代DBに入った被引用数上位の論文を、OpenAlex APIの値で正しいDBへ移動（4章） |
| `scripts/db/upload_s3.sh` | Step 5: 分野1つ分のSQLiteファイルと`SHA256SUMS`（他分野の行は残して差し替え）をS3へアップロードし、サイズを照合 |
| `data/openalex/manifest.json` | S3スナップショットのファイル一覧（2026-10-14版、1,898ファイル）。S3の `s3://openalex/data/parquet/works/manifest.json` の複製 |
| `data/openalex/work/field{分野}/` | 作業フォルダ（git管理外）。stage・out・ログ。分野ごとに分ける（stage の抽出条件が違うため）。※ 2026-11-15 の field 17 は `data/openalex/work/` 直下で実行した |

`stage.py`・`build.py` は Cloudflare版（`journal/scripts/ingest/`）からの複製で、処理内容は同一。ただし `build.py` は著者順を保持するよう修正している（3.3）。

---

## 1. 処理の考え方

- **ダウンロードしない**: スナップショット全体（works だけで約744GB）は落とさず、DuckDB の httpfs で必要な列だけを S3 から Range 読みする。
- **抽出条件**: `primary_topic.field.id = 17`（計算機科学）かつ `publication_year` が 2006〜2026。`is_xpac`（2025年以降の拡張収録分。データセット・ソフトウェア等で、被引用・参考文献がほぼ無い）は OpenAlex API の既定と同じく除外する。
- **DBの区切り**: 2011年を起点に5年ごと（2006-2010, 2011-2015, …, 2026-2030）。論文の所属DBは出版年で決まる。
- **edges**: 引用する側（citing）の年代のDBに置く。引用される側（cited）は取り込んだ全年代のどこかにあればよく、別DB（過去の年代）にあるエッジも含む。アプリはDBをまたいでエッジを辿る。
- **child_cnt**: 全年代の edges から数えた直接の子の数（DBをまたぐ子も数える）。
- **year**: `year + (month-1)/12`（月単位精度）。月が欠損している場合は id から決定的に割り当てる。

---

## 2. SQLiteスキーマ

Cloudflare版の migrations 0001〜0006 と同一。`scripts/db/export_sqlite.py` がこのスキーマでファイルを作る。

```sql
CREATE TABLE papers (
  id        INTEGER PRIMARY KEY,   -- OpenAlexのW番号（数値部分）。分野をまたいで一意
  field     INTEGER NOT NULL,      -- OpenAlex Field ID（計算機科学は17）
  title     TEXT NOT NULL,
  year      REAL NOT NULL,         -- year + (month-1)/12（月単位精度）
  cit       INTEGER NOT NULL,      -- 被引用数
  fwci      REAL,
  subfield  INTEGER,
  child_cnt INTEGER NOT NULL DEFAULT 0,
  doi       TEXT
);
CREATE INDEX idx_papers_field_year ON papers(field, year);
CREATE INDEX idx_papers_field_cit  ON papers(field, cit DESC);
CREATE INDEX idx_papers_field_childcnt ON papers(field, child_cnt DESC);

CREATE TABLE edges (
  cited   INTEGER NOT NULL,   -- 祖先
  citing  INTEGER NOT NULL,   -- 子孫
  PRIMARY KEY (cited, citing)
) WITHOUT ROWID;
CREATE INDEX idx_edges_citing ON edges(citing);

CREATE TABLE authorships (
  paper_id    INTEGER NOT NULL,
  author_name TEXT NOT NULL     -- rowid 順 = 著者順（アプリは ORDER BY rowid で並べる）
);
CREATE INDEX idx_authorships_paper ON authorships(paper_id);
CREATE INDEX idx_authorships_name  ON authorships(author_name);

CREATE TABLE keywords (
  id   TEXT PRIMARY KEY,
  name TEXT NOT NULL
);

CREATE TABLE paper_keywords (
  paper_id   INTEGER NOT NULL,
  keyword_id TEXT NOT NULL,
  score      REAL,
  PRIMARY KEY (paper_id, keyword_id)
) WITHOUT ROWID;
CREATE INDEX idx_paper_keywords_keyword ON paper_keywords(keyword_id);
```

アプリ側（`src-tauri/src/graph.rs`・`paper.rs`・`commands.rs`）はこのスキーマ・列名を前提に実装済みなので、**列名・型を変更しないこと**。変更する場合はRust側の対応するクエリと `export_sqlite.py` の `TABLE_DDL`・`INDEX_DDL`・`TABLES` も合わせて直す。

---

## 3. 手順

### 3.1 EC2インスタンスの準備

| 項目 | 推奨 |
|---|---|
| リージョン | `us-east-1`（OpenAlexのS3バケットと同一。帯域・転送料の面で有利） |
| スペック | 2 vCPU・メモリ7.6GB・EBS 500GB で実績あり（Cloudflare版の本番投入時）。2026-11-15 の検証環境は 4 vCPU・15GB・200GB。作業領域は約5GB＋出力約13GBなので、ディスクは50GB以上あれば足りる |
| OS | Amazon Linux 2023 |
| 必要ツール | Python 3.9+、`duckdb`（pip、1.4.5で確認）、`sqlite3` CLI（手動確認用）、`aws` CLI、`git` |
| IAM | 保存先バケット `970173663066-public-us-east-1` の `journal_app/*` への `s3:PutObject`・`s3:GetObject`（head-object 用）権限を持つインスタンスロール（読み取り元の OpenAlex は匿名アクセスなので不要） |

```bash
# EC2上
sudo dnf install -y python3 python3-pip sqlite git
pip3 install duckdb
git clone <journal_app のリポジトリ> && cd journal_app
```

- DuckDBの `httpfs`・`sqlite` 拡張は**pipパッケージに同梱されていない**。初回に `INSTALL` でネットワークから取得する必要があるが、`run_create_db.sh` が自動で行う。
- OpenAlex の S3 は `--no-sign-request`（匿名）で読めるため、stage〜export に AWS 認証情報は不要。3.5 のアップロードにだけ必要（`aws sts get-caller-identity` で確認できる）。

**注意点**:
- 長時間処理はセッションから切り離す: `setsid nohup <コマンド> > /dev/null 2>&1 < /dev/null &` で起動し、`*.status`・`*.log` で進捗を確認する（SSH/Claude Codeのセッションが切れても止まらない）。
- EC2は使い捨て想定。**作業完了後、生成したSQLiteファイルをS3等へ転送してから必ずインスタンスを終了する**（課金継続の事故を避ける）。

### 3.2 通しで実行する

```bash
cd journal_app

# 段階①: 動作確認（S3 の最新 2 ファイルだけ、1 分未満）。出力先は本番と分ける
WORK=/tmp/create_db_test/work OUT=/tmp/create_db_test/db STAGE_ARGS="--limit-files 2" scripts/db/run_create_db.sh
cat /tmp/create_db_test/work/create_db.status   # DONE になること

# 本番（全 1,898 ファイル、目安 約1時間10分）。UPLOAD=1 で最後に S3 へのアップロード（3.5）まで行う
UPLOAD=1 setsid nohup scripts/db/run_create_db.sh > /dev/null 2>&1 < /dev/null &
cat data/openalex/work/field17/create_db.status   # RUNNING <step> / DONE / FAILED <step>
tail -f data/openalex/work/field17/create_db.log

# 別の分野（例: 22 = Engineering）
FIELD=22 UPLOAD=1 setsid nohup scripts/db/run_create_db.sh > /dev/null 2>&1 < /dev/null &
```

| 環境変数 | 既定値 | 内容 |
|---|---|---|
| `FIELD` | `17` | OpenAlex の分野ID（22 = Engineering など） |
| `WORK` | `data/openalex/work/field{FIELD}` | 作業フォルダ（stage・out・ログ） |
| `OUT` | `src-tauri/resources/db` | SQLite の出力先 |
| `MANIFEST` | `data/openalex/manifest.json` | スナップショットのファイル一覧 |
| `FETCH_MANIFEST` | `0` | `1` なら S3 から最新の manifest を取得して `MANIFEST` を上書きする（5章-5） |
| `STAGE_ARGS` | なし | `stage.py` への追加引数（`--limit-files N`・`--min-year` など） |
| `FIX_MISYEARED` | `1` | `1` なら export 後に `fix_misyeared_papers.py`（4章）を実行する。OpenAlex API を使う |
| `UPLOAD` | `0` | `1` なら検証OKの後に `upload_s3.sh` で S3 へアップロードする |
| `S3_DEST` | `s3://970173663066-public-us-east-1/journal_app/` | アップロード先 |

実行される処理:

1. （`FETCH_MANIFEST=1` または manifest が無い場合）S3 から manifest を取得
2. DuckDB 拡張（httpfs・sqlite）のインストール
3. `stage.py`（失敗ファイルがあれば最大3回まで再試行。完了済みファイルはスキップ）
4. `build.py`
5. `export_sqlite.py`（変換と検証。1つでもNGなら `FAILED export_sqlite`）
6. （`FIX_MISYEARED=1` の場合）`fix_misyeared_papers.py`（4章。移動後の検証NGなら `FAILED fix_misyeared`）
7. （`UPLOAD=1` の場合）`upload_s3.sh`

ログは `create_db.log`（各ステップの出力）と `create_db_mem.log`（30秒ごとのメモリ・ディスク使用量）。

### 3.3 各ステップの詳細（個別に実行する場合）

各スクリプトの既定のパスは `data/openalex/` 以下・`src-tauri/resources/db` なので、リポジトリのルートから引数なしで実行できる。

**Step 1: stage**（実績 約62分、作業領域 約2.4GB）

```bash
python3 scripts/db/stage.py                  # 既定 --field 17 --min-year 2006 --max-year 2026、全1,898ファイル
python3 scripts/db/stage.py --limit-files 2  # 動作確認（更新日の新しい側から N ファイル）
```

- 出力: `data/openalex/work/stage/{updated_date}__{part}.parquet`（入力1ファイルにつき1つ。該当0件でも空ファイルを書く）
- 抽出条件を `stage_params.json` に記録し、既存の stage と条件が違う場合は停止する（やり直す場合は `stage/` を削除）。
- 接続ごとのメモリ上限 `--memory-limit 1GB` × 並列 `--jobs 4`。7.6GBのマシンで OOM しないよう調整済み。

**Step 2: build**（実績 約5分、出力 約2.1GB）

```bash
python3 scripts/db/build.py
```

- 出力: `data/openalex/work/out/{DB名}/{table}.parquet`、`out/stats.json`
- 実行のたびに `out/` を作り直す。中間計算は `build.duckdb`（ディスク上）で行い、溢れた分は `tmp/` にスピルする。
- `stats.json` の `dbs.{DB名}.checks`（NOT NULL違反、年代外の論文、DB内にciting論文が無いエッジ、辞書に無いkeyword_id）と `child_cnt_sum_minus_edges` がすべて0であることを Step 3 が検証する。
- **Cloudflare版からの変更点**: `authorships` に著者順 `pos`（OpenAlex上の順、1始まり。同名の重複は最初の位置）を出力する。Cloudflare版は `list_distinct` で重複除去していたため著者順が保たれていなかった。

**Step 3: export_sqlite**（本番規模は未計測、5章-1）

```bash
python3 scripts/db/export_sqlite.py
python3 scripts/db/export_sqlite.py --verify-only   # 変換せず既存ファイルの検証だけ（4章の修正前のファイルに限る）
```

- テーブルは主キーだけで作成し、**データ投入後にセカンダリインデックスを作成**する。主キーを持つテーブル（`papers`・`edges`・`keywords`・`paper_keywords`）は**主キー順にソートして投入**する。1,000万行の `edges` での実測は、インデックスを先に作る方式の58秒に対し12.7秒（約4.5倍）で、ファイルサイズもやや小さい。最後に `ANALYZE`（`analysis_limit=1000`）を実行する。
- `authorships` は `(paper_id, pos)` 順に投入し、rowid 順 = 著者順にする（`pos` 自体はSQLiteに入れない）。
- 投入列は名前で明示しているため、build の出力列順に依存しない。
- 書き込み中は `{DB名}.sqlite.tmp` に書き、完了後に rename する（途中で止まっても不完全なファイルが残らない）。
- `stats.json` の `dbs` に含まれる DB だけを変換する。

### 3.4 検証

`export_sqlite.py` が変換直後に以下を自動で検証し、1つでもNGなら終了コード1で止まる（sqlite3 CLI 不要）。

- 5テーブルの件数が `out/stats.json` の `dbs.{DB名}` の値と一致すること
- `paper_keywords.keyword_id` が `keywords` にすべて存在すること（スキーマには外部キー制約を付けていないため明示的に確認）
- `stats.json` の `checks` がすべて0
- `PRAGMA quick_check` が `ok`

あわせて、件数・ファイルサイズが6章の参考値と大きく乖離していないか目視で確認する。手動で確認する場合は以下。

```bash
for db in field17_2006_2010 field17_2011_2015 field17_2016_2020 field17_2021_2025 field17_2026_2030; do
  echo "=== $db ==="
  sqlite3 "src-tauri/resources/db/$db.sqlite" <<'EOF'
SELECT 'papers', COUNT(*) FROM papers;
SELECT 'edges', COUNT(*) FROM edges;
SELECT 'authorships', COUNT(*) FROM authorships;
SELECT 'keywords', COUNT(*) FROM keywords;
SELECT 'paper_keywords', COUNT(*) FROM paper_keywords;
SELECT 'child_cnt_sum', SUM(child_cnt) FROM papers;
SELECT 'dangling_paper_keywords', COUNT(*) FROM paper_keywords pk
  LEFT JOIN keywords k ON k.id = pk.keyword_id WHERE k.id IS NULL;
EOF
done
```

### 3.5 S3への保存

検証OKの5ファイルを S3 に保存する。`UPLOAD=1` で実行した場合は `run_create_db.sh` が最後に行う。個別に実行する場合は以下。

```bash
scripts/db/upload_s3.sh
```

保存先（`S3_DEST`、既定 `s3://970173663066-public-us-east-1/journal_app/`）:

```
s3://970173663066-public-us-east-1/journal_app/field17_2006_2010.sqlite
s3://970173663066-public-us-east-1/journal_app/field17_2011_2015.sqlite
s3://970173663066-public-us-east-1/journal_app/field17_2016_2020.sqlite
s3://970173663066-public-us-east-1/journal_app/field17_2021_2025.sqlite
s3://970173663066-public-us-east-1/journal_app/field17_2026_2030.sqlite
s3://970173663066-public-us-east-1/journal_app/field22_2006_2010.sqlite  （field 22 も同じ形式）
...
s3://970173663066-public-us-east-1/journal_app/SHA256SUMS
```

- `upload_s3.sh` は `FIELD` の分野のファイルだけをアップロードする（`FIELD=22 scripts/db/upload_s3.sh`）。
- `SHA256SUMS` は全分野で1つ。S3上の既存の `SHA256SUMS` を取得して今回の分野の行だけ差し替えるので、他の分野の行は残る。
- `SHA256SUMS` は `sha256sum` 形式（`<hex>  <ファイル名>`）。マルチパートアップロードのため S3 の ETag は MD5 にならない。アプリ側でダウンロード後の検証に使う。
- アップロード後、各ファイルのサイズをローカルと `head-object` の `ContentLength` で照合し、不一致なら終了コード1。
- 既存のファイルは上書きされる。

**S3への保存を確認したら EC2 インスタンスを終了する**（課金継続を避けるため）。

### 3.6 アプリでの利用

アプリは 3.5 の S3 からファイルを取得して使う（ビルド成果物には同梱しない）。ファイル名は `src-tauri/src/db.rs` の `DB_REGISTRY` の `file_name` と一致させてある。

- 取得URL（バケットが公開読み取りの場合）: `https://970173663066-public-us-east-1.s3.us-east-1.amazonaws.com/journal_app/<ファイル名>`
- `DB_REGISTRY` には現状 field 17 の5ファイルだけが登録されている。field 22 を使うには `field22_*.sqlite` の5行を追加する。
- 1分野で十数GB以上になるため、取得は中断・再開（HTTP Range）に対応させ、`SHA256SUMS` で検証するのが望ましい。
- `src-tauri/resources/db/` は生成時の既定の出力先（`*.sqlite` は git 管理外）。`tauri.conf.json` の `bundle.resources` にこのディレクトリが入っているため、ここに `.sqlite` を置いたままローカルで `npm run tauri build` すると同梱されてしまう点に注意する。

---

## 4. 出版年の誤りの修正（`fix_misyeared_papers.py`）

2026-10-14 スナップショットでは、一部の著名論文の `publication_year` が誤っている（例: 「Attention Is All You Need」が2025年扱い。OpenAlex側のデータ品質問題で、APIでは修正済み）。誤った年のせいで違う年代DBに入った論文を、OpenAlex APIの最新値で正しいDBへ移動する。Cloudflare版の `fix_misyeared_papers.py`（D1用）をSQLite用に書き直したもの。

```bash
python3 scripts/db/fix_misyeared_papers.py --dry-run      # 判定のみ（data/openalex/work/fix_misyeared_plan.json に保存）
python3 scripts/db/fix_misyeared_papers.py                # 判定・移動・検証
python3 scripts/db/fix_misyeared_papers.py --verify-only  # 検証だけ
```

- **対象**: 各DBの `cit >= 1000`（`--min-cit`、計5,011件）。全1,144万件の再確認は現実的でないため、誤りが偏っていた被引用数上位に絞る。
- **判定**: OpenAlex APIに50件ずつ問い合わせ、`publication_year` から求めた年代DBが今のDBと違うものを移動対象にする。一括取得で見つからない（OpenAlexがWorkをマージした）IDは `/works/W{id}` のリダイレクトで解決する。
- **移動**: papers 1行（`year`・`cit` はAPIの最新値に更新）、`edges(citing = id)`、`authorships`（rowid順 = 著者順を保つ）、`paper_keywords`、移動先に無い `keywords`。移動元・移動先のDBを ATTACH し、1論文を1トランザクションで移す。`edges(cited = id)` は citing 側のDBに置く規則なので動かさない（`child_cnt` も変わらない）。
- **移動しないもの**: 正しい年が対象範囲外（2005年以前など）の論文。
- **検証**: 全DBで年代外の論文が0、`quick_check` ok、全DB合計の件数が `stats.json` と一致（移動はDB間の付け替えなので合計は不変）、移動した論文が移動先に揃い移動元に残っていないこと。DBごとの件数は変わるため、修正後のファイルに `export_sqlite.py --verify-only` は使えない。
- **再実行**: 安全（移動済みの論文は正しいDBにあるため対象にならない）。
- **API利用量**: 約200リクエスト（約$0.01）。APIキー無しの無料枠（$0.10/日）で足りる。環境変数 `OPENALEX_API_KEY` があれば使う。
- **既知の制約**: `cit < 1000` の論文は未検証。移動した論文だけ `cit` がAPIの最新値になる（他はスナップショット時点の値）。マージされた重複Workは両方が残る（例: 「Attention Is All You Need」は W2626778328 と W4385245566 の2件）。

2026-11-15 の実行結果（24件移動）は docs/SESSION_CREATE_DB.md 2.5 を参照。

---

## 5. 未確定事項

1. **公開読み取りの設定**: バケット名に public を含むが、`journal_app/` 以下が匿名で取得できるか（バケットポリシー）は未確認。
2. ~~本番規模での実行時間~~: 2026-11-15 に実行（stage 44.6分・build 5分・export 9分・S3 アップロード 約2.5分）。件数は6章と完全一致、合計 12.46GB（docs/SESSION_CREATE_DB.md）。
3. **WAL・journalモード**: 生成ファイルはデフォルトのjournalモード（DELETE）。アプリは読み取り専用で開くため問題ない想定。
4. **`VACUUM`の要否**: 主キー順で新規ファイルに投入するため断片化は小さい見込みで、現状は実行していない。配布サイズが6章の参考値より大きく乖離した場合に検討する。
5. ~~出版年の誤り~~: 2026-11-15 に4章の修正を実施（24件移動）。`cit < 1000` は未検証。
6. **スナップショットの選定**: 2026-11-15 時点で S3 の最新スナップショットは同梱の `manifest.json` と同じ 2026-10-14（内容も一致を確認済み）。OpenAlex は週次で更新し、古いスナップショットのファイルは S3 から消えていく。そのため、manifest 記載のファイルが無くなった後は `FETCH_MANIFEST=1` で最新の manifest を取得して実行する（件数は6章の参考値と一致しなくなる。取得した manifest はコミットしておくと再現性が保てる）。

---

## 6. 参考: Cloudflare版での実績（2026-10-14 スナップショット、2026-11-11 実行）

2 vCPU・7.6GB の EC2 で、stage 61.9分（1,898ファイル、失敗0、メモリ約4.5GBで安定）→ build 5分。作業領域は stage 2.4GB / out 2.1GB。重複id 0件、build の整合性チェックはすべて0。

| DB | papers | edges | authorships | keywords | paper_keywords | D1サイズ（インデックス込み） |
|---|---|---|---|---|---|---|
| field17_2006_2010 | 1,932,850 | 3,072,398 | 4,426,213 | 443,391 | 10,565,552 | 1.64 GB |
| field17_2011_2015 | 2,436,353 | 10,291,552 | 5,850,778 | 518,022 | 13,771,688 | 2.37 GB |
| field17_2016_2020 | 2,693,374 | 19,159,162 | 7,319,358 | 575,258 | 16,011,926 | 3.19 GB |
| field17_2021_2025 | 3,340,772 | 32,930,571 | 11,148,218 | 636,741 | 22,825,086 | 4.82 GB |
| field17_2026_2030 | 1,034,662 | 4,405,995 | 3,206,170 | 411,488 | 7,093,028 | 1.26 GB |
| **合計** | **11,438,011** | **69,859,678** | **31,950,737** | — | **70,267,280** | **13.28 GB** |

- 同じスナップショットと同じ条件で実行すれば、件数はこの表と一致するはず（authorships の件数も、重複除去の方法が同値なので変わらない）。D1サイズは4章の移動分を含み、SQLiteファイルとは完全には一致しない。
- edges のうち cited が別DBにあるものは 4,709万件（全体の約67%）。参考文献の総数1億4,683万件のうち、両端が取り込み範囲内のものが約48%。
