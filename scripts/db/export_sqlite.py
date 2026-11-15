#!/usr/bin/env python3
"""build.py の出力（{work}/out/{DB名}/{table}.parquet）を SQLite ファイルへ直接変換し、検証する
（docs/CREATE_DB.md 3.3〜3.4）。

Cloudflare 版（journal）では SQL 文を生成して D1 に投入していたが、SQLite には D1 の制約（バインド変数上限、
SQL 文長上限、レート制限）が無いため、DuckDB の sqlite 拡張で直接書き込む。

高速化:
  - テーブルは主キーだけで作り、セカンダリインデックスはデータ投入後に作る
  - 主キーを持つテーブルは主キー順にソートして投入する（B-tree への追記だけになる）
  実測（1,000 万行の edges）: インデックス先作り 58 秒 → 本方式 12.7 秒

  authorships は Rust 側が `ORDER BY rowid` で著者順を決めているため、(paper_id, pos) 順に投入して
  rowid 順 = 著者順にする（pos は build.py が出力する著者順。SQLite には列として入れない）。

検証（docs/CREATE_DB.md 3.4、sqlite3 CLI 不要）:
  - 各テーブルの件数が out/stats.json の値と一致すること
  - paper_keywords.keyword_id が keywords に存在すること
  - stats.json の checks が全て 0 であること
  - PRAGMA quick_check が ok であること
  1 つでも満たさなければ終了コード 1。

使い方:
  python3 scripts/db/export_sqlite.py --work data/openalex/work --out src-tauri/resources/db
  通常は scripts/db/run_create_db.sh から呼ばれる。
"""
from __future__ import annotations

import argparse
import json
import os
import sqlite3
import sys
import time

import duckdb

DEFAULT_WORK = os.path.join(os.path.dirname(__file__), "../../data/openalex/work")
DEFAULT_OUT = os.path.join(os.path.dirname(__file__), "../../src-tauri/resources/db")

# docs/CREATE_DB.md 2 章（Cloudflare 版の migrations 0001〜0006 と同一）。列名・型は Rust 側が前提にしているので変更しないこと
TABLE_DDL = {
    "papers": """CREATE TABLE papers (
  id INTEGER PRIMARY KEY, field INTEGER NOT NULL, title TEXT NOT NULL,
  year REAL NOT NULL, cit INTEGER NOT NULL, fwci REAL, subfield INTEGER,
  child_cnt INTEGER NOT NULL DEFAULT 0, doi TEXT
)""",
    "edges": """CREATE TABLE edges (
  cited INTEGER NOT NULL, citing INTEGER NOT NULL, PRIMARY KEY (cited, citing)
) WITHOUT ROWID""",
    "authorships": "CREATE TABLE authorships (paper_id INTEGER NOT NULL, author_name TEXT NOT NULL)",
    "keywords": "CREATE TABLE keywords (id TEXT PRIMARY KEY, name TEXT NOT NULL)",
    "paper_keywords": """CREATE TABLE paper_keywords (
  paper_id INTEGER NOT NULL, keyword_id TEXT NOT NULL, score REAL,
  PRIMARY KEY (paper_id, keyword_id)
) WITHOUT ROWID""",
}

INDEX_DDL = [
    "CREATE INDEX idx_papers_field_year ON papers(field, year)",
    "CREATE INDEX idx_papers_field_cit ON papers(field, cit DESC)",
    "CREATE INDEX idx_papers_field_childcnt ON papers(field, child_cnt DESC)",
    "CREATE INDEX idx_edges_citing ON edges(citing)",
    "CREATE INDEX idx_authorships_paper ON authorships(paper_id)",
    "CREATE INDEX idx_authorships_name ON authorships(author_name)",
    "CREATE INDEX idx_paper_keywords_keyword ON paper_keywords(keyword_id)",
]

# (列リスト, 投入時の ORDER BY)。列は明示して build.py 側の列順変更の影響を受けないようにする
TABLES = {
    "papers": ("id, field, title, year, cit, fwci, subfield, child_cnt, doi", "id"),
    "edges": ("cited, citing", "cited, citing"),
    "authorships": ("paper_id, author_name", "paper_id, pos"),
    "keywords": ("id, name", "id"),
    "paper_keywords": ("paper_id, keyword_id, score", "paper_id, keyword_id"),
}


def q(s: str) -> str:
    return "'" + s.replace("'", "''") + "'"


def log(msg: str) -> None:
    print(f"[{time.strftime('%F %T')}] {msg}", flush=True)


def export_db(con: duckdb.DuckDBPyConnection, src_dir: str, sqlite_path: str) -> None:
    tmp = sqlite_path + ".tmp"
    if os.path.exists(tmp):
        os.remove(tmp)

    raw = sqlite3.connect(tmp)
    raw.executescript(";\n".join(TABLE_DDL.values()) + ";")
    raw.close()

    con.execute(f"ATTACH {q(tmp)} AS sq (TYPE sqlite)")
    try:
        for table, (cols, order) in TABLES.items():
            t = time.time()
            parquet = os.path.join(src_dir, f"{table}.parquet")
            order_sql = f" ORDER BY {order}" if order else ""
            con.execute(f"INSERT INTO sq.{table} ({cols}) SELECT {cols} FROM read_parquet({q(parquet)}){order_sql}")
            log(f"  {table}: 投入 {time.time() - t:.1f}s")
    finally:
        con.execute("DETACH sq")

    raw = sqlite3.connect(tmp)
    raw.execute("PRAGMA cache_size = -1000000")  # 約 1GB（インデックス作成のソート用）
    for ddl in INDEX_DDL:
        t = time.time()
        raw.execute(ddl)
        log(f"  {ddl.split(' ON ')[0].split()[-1]}: {time.time() - t:.1f}s")
    raw.execute("PRAGMA analysis_limit = 1000")
    raw.execute("ANALYZE")
    raw.commit()
    raw.close()
    os.replace(tmp, sqlite_path)


def verify_db(sqlite_path: str, expected: dict) -> list[str]:
    errors = []
    c = sqlite3.connect(f"file:{sqlite_path}?mode=ro", uri=True)
    for table in TABLES:
        n = c.execute(f"SELECT count(*) FROM {table}").fetchone()[0]
        if n != expected[table]:
            errors.append(f"{table}: 件数 {n:,} != stats.json {expected[table]:,}")
    dangling = c.execute(
        "SELECT count(*) FROM paper_keywords pk LEFT JOIN keywords k ON k.id = pk.keyword_id WHERE k.id IS NULL"
    ).fetchone()[0]
    if dangling:
        errors.append(f"dangling_paper_keywords: {dangling:,}")
    for name, v in expected.get("checks", {}).items():
        if v:
            errors.append(f"stats.json checks.{name}: {v:,}")
    qc = c.execute("PRAGMA quick_check").fetchone()[0]
    if qc != "ok":
        errors.append(f"quick_check: {qc}")
    c.close()
    return errors


def main() -> None:
    ap = argparse.ArgumentParser(description="build.py の出力を SQLite ファイルへ変換して検証する")
    ap.add_argument("--work", default=DEFAULT_WORK, help="作業フォルダ（build.py の出力 out/ を含む）")
    ap.add_argument("--out", default=DEFAULT_OUT, help="SQLite の出力先ディレクトリ")
    ap.add_argument("--threads", type=int, default=2)
    ap.add_argument("--memory-limit", default="5GB")
    ap.add_argument("--verify-only", action="store_true", help="変換せず既存の SQLite ファイルの検証だけ行う")
    args = ap.parse_args()

    out_root = os.path.join(args.work, "out")
    stats = json.load(open(os.path.join(out_root, "stats.json")))
    os.makedirs(args.out, exist_ok=True)

    con = None
    if not args.verify_only:
        con = duckdb.connect()
        con.execute(f"SET threads TO {args.threads}")
        con.execute(f"SET memory_limit = {q(args.memory_limit)}")
        con.execute(f"SET temp_directory = {q(os.path.join(args.work, 'tmp'))}")
        con.execute("LOAD sqlite")

    failed = False
    for db_name in sorted(stats["dbs"]):
        sqlite_path = os.path.join(args.out, f"{db_name}.sqlite")
        if con is not None:
            t = time.time()
            log(f"{db_name}: 変換開始")
            export_db(con, os.path.join(out_root, db_name), sqlite_path)
            log(f"{db_name}: 変換完了 {time.time() - t:.1f}s")
        errors = verify_db(sqlite_path, stats["dbs"][db_name])
        size = os.path.getsize(sqlite_path) / 1e9
        if errors:
            failed = True
            log(f"{db_name}: 検証NG ({size:.2f} GB)")
            for e in errors:
                log(f"  - {e}")
        else:
            counts = ", ".join(f"{t}={stats['dbs'][db_name][t]:,}" for t in TABLES)
            log(f"{db_name}: 検証OK ({size:.2f} GB) {counts}")

    if con is not None:
        con.close()
    if failed:
        sys.exit(1)
    log("全 DB 検証OK")


if __name__ == "__main__":
    main()
