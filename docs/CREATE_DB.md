# EC2上でのSQLiteデータベース生成手順（CREATE_DB.md）

- ステータス: ドラフト
- 対象: journal_app（スタンドアロン版）に同梱する5つのSQLiteファイル（`field17_2006_2010.sqlite` 〜 `field17_2026_2030.sqlite`）をEC2上で生成する手順
- 前提: [docs/native.md](./native.md) 6章（データ準備の方針）
- 参照元（Cloudflare版リポジトリ `journal/` の既存ドキュメント・スクリプト）:
  - [journal/docs/init-ingest.md](../../journal/docs/init-ingest.md) — S3スナップショットからの抽出設計の全体像
  - [journal/docs/SESSION_EC2.md](../../journal/docs/SESSION_EC2.md) — 実際にEC2で本番データを投入した際の実績・数値
  - [journal/scripts/ingest/](../../journal/scripts/ingest/) — `stage.py`・`build.py`・`export_sql.py`・`load_d1.py`・`fix_misyeared_papers.py`

---

## 0. 要約

| 項目 | 内容 |
|---|---|
| 生成するファイル | `field17_2006_2010.sqlite` / `field17_2011_2015.sqlite` / `field17_2016_2020.sqlite` / `field17_2021_2025.sqlite` / `field17_2026_2030.sqlite`（合計 約13.3GB、`journal`の投入実績ベース） |
| データ元 | OpenAlex S3スナップショット（`s3://openalex/data/parquet/works/`、Parquet形式、CC0） |
| 処理場所 | EC2インスタンス（OpenAlexのS3バケットと同じ`us-east-1`推奨） |
| 既存資産の再利用 | `journal/scripts/ingest/stage.py`・`build.py`はそのまま流用可能。**D1投入用の`export_sql.py`・`load_d1.py`は使わず、代わりにDuckDBの`sqlite`拡張でParquet→SQLiteへ直接変換する**（D1固有の制約が無いぶんシンプルになる、docs/native.md 6.1） |
| 本書のゴール | `journal/`のEC2作業で確立済みのstage→buildパイプラインを前提に、**D1投入の代わりにSQLiteファイルを直接作る**部分だけを新規に詰める |

---

## 1. 前提: なぜ `journal/` のスクリプトがそのまま使えるのか

`journal/scripts/ingest/stage.py`（S3→中間Parquet）と`build.py`（中間Parquet→DBごとのテーブル）は、**D1 にもSQLiteにも依存しない中間生成物（Parquetファイル）を作る**工程であり、出力スキーマ（`papers`/`edges`/`authorships`/`keywords`/`paper_keywords`の列構成）はmigrations 0001〜0005と一致させてある。journal_app側のSQLiteスキーマも同じmigrationsをそのまま使う前提（下記2章）なので、**stage.py・build.pyの実行手順はjournal側のドキュメント（init-ingest.md 3.1〜3.3）をそのまま踏襲してよい**。

変わるのは最終段（D1へのSQL投入）だけで、ここをSQLiteファイルへの直接書き込みに置き換える。

---

## 2. SQLiteスキーマ（migrations相当）

journal（Cloudflare D1）側の `journal/migrations/0001〜0006` と同一のスキーマを使う。EC2上でSQLiteファイルを新規作成する際は、以下のDDLをそのまま使う。

```sql
CREATE TABLE papers (
  id        INTEGER PRIMARY KEY,   -- OpenAlexのW番号（数値部分）。分野をまたいで一意
  field     INTEGER NOT NULL,      -- OpenAlex Field ID（計算機科学は17）
  title     TEXT NOT NULL,
  year      REAL NOT NULL,         -- year + (month-1)/12（月単位精度、migrations/0002）
  cit       INTEGER NOT NULL,      -- 被引用数
  fwci      REAL,
  subfield  INTEGER,
  child_cnt INTEGER NOT NULL DEFAULT 0,
  doi       TEXT
);
CREATE INDEX idx_papers_field_year ON papers(field, year);
CREATE INDEX idx_papers_field_cit  ON papers(field, cit DESC);
CREATE INDEX idx_papers_field_childcnt ON papers(field, child_cnt DESC);  -- migrations/0006

CREATE TABLE edges (
  cited   INTEGER NOT NULL,   -- 祖先
  citing  INTEGER NOT NULL,   -- 子孫
  PRIMARY KEY (cited, citing)
) WITHOUT ROWID;
CREATE INDEX idx_edges_citing ON edges(citing);

CREATE TABLE authorships (
  paper_id    INTEGER NOT NULL,
  author_name TEXT NOT NULL
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

journal_app側（`src-tauri/src/graph.rs`・`paper.rs`・`commands.rs`）はこのスキーマ・列名を前提に実装済みなので、**列名・型を変更しないこと**。変更する場合はRust側の対応するクエリも合わせて直す必要がある。

---

## 3. 手順

### 3.1 EC2インスタンスの準備

`journal/docs/SESSION_EC2.md` 1章・`init-ingest.md` 3.5の実績に準じる。

| 項目 | 推奨 |
|---|---|
| リージョン | `us-east-1`（OpenAlexのS3バケットと同一。帯域・転送料の面で有利） |
| スペック | 実績: 2 vCPU・メモリ7.6GB・EBS 500GB、Amazon Linux 2023（`journal`本番投入時と同条件） |
| 必要ツール | Python 3.9+、`duckdb`（pip、`sqlite`拡張・`httpfs`拡張は自動同梱） |

```bash
# EC2上
sudo yum install -y python3 python3-pip   # Amazon Linux の場合
pip3 install duckdb
```

**注意点（`SESSION_EC2.md`より）**:
- 長時間処理はセッションから切り離す: `setsid nohup <コマンド> > /dev/null 2>&1 < /dev/null &` で起動し、`*.status`・`*.log` で進捗を確認する（SSH/Claude Codeのセッションが切れても止まらない）。
- EC2は使い捨て想定。**作業完了後、生成したSQLiteファイルをS3等へアップロードしてから必ずインスタンスを終了する**（課金継続の事故を避ける）。

### 3.2 stage → build（journalのスクリプトをそのまま実行）

`journal`リポジトリを（またはそのEC2インスタンス上にcloneして）用意し、`scripts/ingest/`をそのまま使う。

```bash
cd journal

# Step 1: S3 → 中間Parquet（init-ingest.md 3.3 Step 1）
python3 scripts/ingest/stage.py   # 既定 --field 17 --min-year 2006 --max-year 2026、全1,898ファイル
cat data/openalex/work/pipeline.status   # DONE になるまで待つ（実績: 約62分）

# Step 2: 中間Parquet → DBごとのテーブル（init-ingest.md 3.3 Step 2）
python3 scripts/ingest/build.py   # 出力: data/openalex/work/out/{DB名}/{table}.parquet（実績: 約5分）
```

- 実績（`SESSION_EC2.md` 4.3）: stage 61.9分、build 5分。作業領域 stage 2.4GB / out 2.1GB。
- `out/stats.json` に整合性チェック（NOT NULL違反、年代外の論文、参照整合性など）が出力される。**全項目が0であることを確認してから次に進む**（3.4参照）。
- 小規模データでまず動作確認してから本番規模に進む段階実行の方針は `init-ingest.md` 3.0 を踏襲する（`--limit-files`・`--min-year`で範囲を絞って試す）。

### 3.3 build出力 → SQLiteファイル（D1投入の代わり、新規手順）

`journal`では、ここから`export_sql.py`（SQL文生成）→`load_d1.py`（`wrangler d1 execute`でD1投入・検証）という2段階を踏んでいたが、journal_appではD1を経由しないため、**DuckDBの`sqlite`拡張でParquetを直接SQLiteファイルへコピーする**。これにより中間SQLファイル生成・wrangler経由の投入・D1のレート制限対策（リトライ、チャンク分割）がすべて不要になる。

DuckDBの`sqlite`拡張は、SQLite用の`ATTACH`と`COPY`で直接テーブルを書き込める（参考: https://duckdb.org/docs/extensions/sqlite）。

`scripts/export_sqlite.py`として新規スクリプトを用意する想定（journal_appリポジトリ側に追加する、またはこのまま手元のワンショットスクリプトとして実行する）。

```python
#!/usr/bin/env python3
"""build.py の出力（out/{DB名}/{table}.parquet）を SQLite ファイルへ直接変換する。
D1 の制約（バインド変数100件上限、SQL文長上限、レート制限）が無いため、
export_sql.py・load_d1.py を経由せずそのまま COPY できる。

使い方:
  python3 export_sqlite.py --work /path/to/journal/data/openalex/work --out /path/to/journal_app/src-tauri/resources/db
"""
import argparse
import os
import duckdb

SCHEMA_SQL = """
CREATE TABLE papers (
  id INTEGER PRIMARY KEY, field INTEGER NOT NULL, title TEXT NOT NULL,
  year REAL NOT NULL, cit INTEGER NOT NULL, fwci REAL, subfield INTEGER,
  child_cnt INTEGER NOT NULL DEFAULT 0, doi TEXT
);
CREATE INDEX idx_papers_field_year ON papers(field, year);
CREATE INDEX idx_papers_field_cit ON papers(field, cit DESC);
CREATE INDEX idx_papers_field_childcnt ON papers(field, child_cnt DESC);

CREATE TABLE edges (
  cited INTEGER NOT NULL, citing INTEGER NOT NULL, PRIMARY KEY (cited, citing)
) WITHOUT ROWID;
CREATE INDEX idx_edges_citing ON edges(citing);

CREATE TABLE authorships (paper_id INTEGER NOT NULL, author_name TEXT NOT NULL);
CREATE INDEX idx_authorships_paper ON authorships(paper_id);
CREATE INDEX idx_authorships_name ON authorships(author_name);

CREATE TABLE keywords (id TEXT PRIMARY KEY, name TEXT NOT NULL);

CREATE TABLE paper_keywords (
  paper_id INTEGER NOT NULL, keyword_id TEXT NOT NULL, score REAL,
  PRIMARY KEY (paper_id, keyword_id)
) WITHOUT ROWID;
CREATE INDEX idx_paper_keywords_keyword ON paper_keywords(keyword_id);
"""

TABLES = ["papers", "edges", "authorships", "keywords", "paper_keywords"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--work", required=True, help="journal の data/openalex/work ディレクトリ")
    ap.add_argument("--out", required=True, help="出力先ディレクトリ（journal_app の resources/db）")
    args = ap.parse_args()

    out_root = os.path.join(args.work, "out")
    os.makedirs(args.out, exist_ok=True)

    con = duckdb.connect()
    con.execute("INSTALL sqlite; LOAD sqlite;")

    for db_name in sorted(d for d in os.listdir(out_root) if os.path.isdir(os.path.join(out_root, d))):
        sqlite_path = os.path.join(args.out, f"{db_name}.sqlite")
        if os.path.exists(sqlite_path):
            os.remove(sqlite_path)

        # 空の SQLite ファイルをスキーマ付きで作成してから ATTACH する
        import sqlite3
        raw = sqlite3.connect(sqlite_path)
        raw.executescript(SCHEMA_SQL)
        raw.commit()
        raw.close()

        con.execute(f"ATTACH '{sqlite_path}' AS sq (TYPE sqlite)")
        for table in TABLES:
            parquet = os.path.join(out_root, db_name, f"{table}.parquet")
            print(f"{db_name}: {table} を投入中...", flush=True)
            con.execute(f"INSERT INTO sq.{table} SELECT * FROM read_parquet('{parquet}')")
        con.execute("DETACH sq")
        print(f"{db_name}: 完了 → {sqlite_path}", flush=True)

    con.close()


if __name__ == "__main__":
    main()
```

```bash
# EC2上で実行
python3 export_sqlite.py \
  --work journal/data/openalex/work \
  --out  journal_app/src-tauri/resources/db
```

- `build.py`の出力列はmigrationsのスキーマ（`papers`: id, field, title, year, cit, fwci, subfield, child_cnt, doi の順）と一致しているため、`SELECT * FROM read_parquet(...)` の列順がそのまま対応する。**列の順序がずれていないか最初の1DBで必ず確認すること**（`sqlite3 <file>.sqlite ".schema papers"` と `PRAGMA table_info(papers);` で確認）。
- `INSERT INTO sq.{table} SELECT * FROM read_parquet(...)` は1トランザクションでまとめて書き込まれる。D1投入時のような「1文90KB・1ファイル100MB」というチャンク制約は不要（SQLiteのINSERTにそのような上限は無い）。
- 投入順序（papers→edges→authorships→keywords→paper_keywords）はTABLESのリスト順で担保しているが、SQLiteには外部キー制約を設定していないため厳密な順序依存は無い（念のため元の順序を踏襲）。

### 3.4 検証

`journal`の`load_d1.py`が行っていた検証（`init-ingest.md` 3.6）を、SQLite版として実施する。

```bash
for db in field17_2006_2010 field17_2011_2015 field17_2016_2020 field17_2021_2025 field17_2026_2030; do
  echo "=== $db ==="
  sqlite3 "journal_app/src-tauri/resources/db/$db.sqlite" <<'EOF'
SELECT 'papers', COUNT(*) FROM papers;
SELECT 'edges', COUNT(*) FROM edges;
SELECT 'authorships', COUNT(*) FROM authorships;
SELECT 'keywords', COUNT(*) FROM keywords;
SELECT 'paper_keywords', COUNT(*) FROM paper_keywords;
-- child_cnt の合計とedges件数の整合性（build.py の stats.json の値と突き合わせる）
SELECT 'child_cnt_sum', SUM(child_cnt) FROM papers;
-- 参照整合性: paper_keywords.keyword_id が keywords.id に存在するか
SELECT 'dangling_paper_keywords', COUNT(*) FROM paper_keywords pk
  LEFT JOIN keywords k ON k.id = pk.keyword_id WHERE k.id IS NULL;
EOF
done
```

- `out/stats.json`（build.py出力）の`dbs.{DB名}.papers`・`edges`等の件数と、上記`SELECT COUNT(*)`の値が一致することを確認する。
- `dangling_paper_keywords`が0であることを確認する（D1と違いSQLiteは外部キー制約を有効化できるが、`journal`のスキーマは意図的に外部キー制約を付けていないため、ここでの明示的な検証が必要）。
- ファイルサイズも`SESSION_EC2.md`の実績（4.4、4.5）と大きく乖離していないか確認する（目安: 5DB合計で約13.3GB。D1のインデックス込みサイズとほぼ一致する見込み）。

### 3.5 EC2からの転送

SQLiteファイルが5つ揃ったら、EC2インスタンスを終了する前にファイルを転送する。

```bash
# 例: S3経由（EC2 → S3 → ローカルのビルド環境）
aws s3 cp journal_app/src-tauri/resources/db/ s3://<バケット名>/journal-app-db/ --recursive

# ローカル側で取得
aws s3 cp s3://<バケット名>/journal-app-db/ ./journal_app/src-tauri/resources/db/ --recursive
```

または直接`scp`でも良い（13GB規模の転送になるため、S3経由の方が中断時の再開がしやすく安定しやすい）。

**この時点でEC2インスタンスを終了する**（`docs/native.md` 6.1、`SESSION_EC2.md`と同様、課金継続を避けるため）。

### 3.6 Tauriビルドへの組み込み

転送したファイルを`journal_app/src-tauri/resources/db/`に配置すれば、`tauri.conf.json`の`bundle.resources`設定により自動的にビルド成果物へ同梱される（`docs/native.md` 6.2、既存の`src-tauri/resources/db/README.md`参照）。

```bash
ls journal_app/src-tauri/resources/db/
# field17_2006_2010.sqlite
# field17_2011_2015.sqlite
# field17_2016_2020.sqlite
# field17_2021_2025.sqlite
# field17_2026_2030.sqlite

cd journal_app
npm run tauri build
```

---

## 4. 任意の後処理（データ品質の修正）

`journal`では本番投入後に出版年の誤り（OpenAlex S3スナップショット側のデータ品質問題、`SESSION_EC2.md` 5.2）が見つかり、`scripts/ingest/fix_misyeared_papers.py`で被引用数上位（`cit >= 1000`）の論文をOpenAlex APIで再検証・DB間移動する対応を行った。

journal_app向けに同じ修正を行う場合、このスクリプトはD1（`wrangler d1 execute`）への書き込みを前提にしているため、SQLite直接操作用に書き換えが必要になる（本書の範囲外、必要になった時点で別途対応する）。**最新のOpenAlex S3スナップショットを使う場合、同じ誤りが残っているかは未確認**なので、まず3.4の検証で明らかな異常（著名論文の出版年が極端にずれている等）が無いか確認し、問題が見つかった場合にのみ対応を検討する。

---

## 5. 未確定事項

1. **`export_sqlite.py`の実機未検証**: 本書のスクリプトはDuckDBの`sqlite`拡張のAPI仕様に基づいて設計したが、実際にEC2上で実行して動作確認はまだ行っていない。特に大規模テーブル（`edges`が最大7,000万行規模）の`INSERT INTO sq.{table} SELECT ...`がメモリ・時間の面でどの程度かかるかは未計測。
2. **DuckDBの`sqlite`拡張のバージョン互換性**: `journal`のEC2実績はduckdb 1.4.5（pip）。`sqlite`拡張が同バージョンで問題なく使えるかは未確認（`INSTALL sqlite`でネットワーク取得が必要な場合、EC2のネットワーク設定次第で失敗する可能性がある）。
3. **WAL・journalモードの扱い**: 生成したSQLiteファイルはデフォルトのjournalモード（DELETE）になる想定。配布用の読み取り専用ファイルとしては問題ないはずだが、ファイルサイズ・生成時間に影響する可能性があるため、`PRAGMA journal_mode`の明示設定を検討してもよい。
4. **`VACUUM`の要否**: DuckDB経由でのバルクINSERT後、SQLiteファイルの断片化・余剰領域が発生するか未確認。配布サイズを最小化したい場合は生成の最後に`VACUUM`を検討する。
5. **最新のOpenAlexスナップショットを使うか、`journal`投入時と同じスナップショット（2026-10-14時点）を使うか**: 新しいスナップショットを使う場合、`journal/docs/SESSION_EC2.md`の実績件数（論文1,144万件等）とは一致しなくなる想定。本書は手順の再現性を優先し、スナップショットの選定自体は対象外とする。
