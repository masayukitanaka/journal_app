#!/usr/bin/env python3
"""出版年の誤りを修正する（docs/CREATE_DB.md 4 章）。export_sqlite.py で SQLite を作った後に実行する。

背景: OpenAlex の S3 スナップショット（2026-10-14 時点）では、一部の著名論文（"Attention Is All You Need" 等）の
publication_year が誤っている（OpenAlex 側のデータ品質問題。OpenAlex API では修正済み）。誤った年のせいで本来と違う
年代 DB に入った論文を、API の最新値で正しい年代 DB へ移動する。

Cloudflare 版リポジトリ journal の scripts/ingest/fix_misyeared_papers.py（D1 / wrangler 用）を SQLite 用に書き直したもの。
判定・移動の規則は同じ。違いは以下。
  - 2 つの DB ファイルを ATTACH し、1 論文の移動（移動先への INSERT と移動元からの DELETE）を 1 トランザクションで行う
  - authorships は rowid 順（= 著者順）を保って移動する
  - API 問い合わせ（判定）と移動を分け、判定結果を {work}/fix_misyeared_plan.json に保存する
  - 移動後に整合性を検証する（--verify-only で検証だけも可）

対象: 各 DB で cit >= --min-cit（既定 1000、全 DB で約 5,000 件）の論文。出版年の誤りは被引用数が突出した古典論文に
偏っていた（Cloudflare 版での実測）という前提の絞り込み。

移動の規則:
  - 正しい年代 DB が現在の DB と違う論文を移動する。正しい年が対象範囲外（2005 年以前など）の DB になる場合は移動しない
  - 移動するもの: papers 1 行（year・cit は API の最新値に更新）、edges(citing = id)、authorships、paper_keywords、
    移動先に無い keywords。edges(cited = id) は citing 側の DB に置く規則なので動かさない（child_cnt も不変）
  - OpenAlex が Work をマージしていて一括取得で見つからない場合は /works/W{id} のリダイレクトで解決する
  - 再実行しても安全（移動済みの論文は正しい DB にあるため対象にならない）

使い方:
  分野・DB 一覧は --work の out/stats.json から読む（field 17 以外も同じ手順）。
  python3 scripts/db/fix_misyeared_papers.py --dry-run        # 判定のみ（DB は変更しない）
  python3 scripts/db/fix_misyeared_papers.py                  # 判定して移動し、検証する
  python3 scripts/db/fix_misyeared_papers.py --verify-only    # 検証だけ
  環境変数 OPENALEX_API_KEY があれば使う（無くても無料枠 $0.10/日 で足りる。今回の問い合わせは約 $0.01）
"""
from __future__ import annotations

import argparse
import json
import math
import os
import sqlite3
import sys
import time

import requests

DEFAULT_WORK = os.path.join(os.path.dirname(__file__), "../../data/openalex/work")
DEFAULT_DB_DIR = os.path.join(os.path.dirname(__file__), "../../src-tauri/resources/db")
# 分野・年代区切り・DB 一覧は build.py の stats.json から読む（configure() で設定。分野ごとに作業フォルダが分かれる）
FIELD = 0
BUCKET_ANCHOR = 2011
BUCKET_YEARS = 5
DB_NAMES: list[str] = []
TABLES = ["papers", "edges", "authorships", "keywords", "paper_keywords"]
BATCH = 50


def configure(stats_path: str) -> dict:
    global FIELD, BUCKET_ANCHOR, BUCKET_YEARS, DB_NAMES
    stats = json.load(open(stats_path))
    FIELD = stats["params"]["field"]
    BUCKET_ANCHOR = stats["params"]["bucket_anchor"]
    BUCKET_YEARS = stats["params"]["bucket_years"]
    DB_NAMES = sorted(stats["dbs"])
    return stats


def log(msg: str) -> None:
    print(f"[{time.strftime('%F %T')}] {msg}", flush=True)


def db_for_year(year_int: int) -> str:
    b = BUCKET_ANCHOR + BUCKET_YEARS * math.floor((year_int - BUCKET_ANCHOR) / BUCKET_YEARS)
    return f"field{FIELD}_{b}_{b + BUCKET_YEARS - 1}"


def bucket_of(db: str) -> tuple[int, int]:
    _, start, end = db.split("_")
    return int(start), int(end)


def compute_year(pub_year: int, pub_date: str | None, paper_id: int) -> float:
    # stage.py と同じ規則（月が無ければ id から決定的に割り当てる）
    month = int(pub_date.split("-")[1]) if pub_date else (paper_id % 12) + 1
    return pub_year + (month - 1) / 12.0


class OpenAlex:
    SELECT = "id,title,publication_year,publication_date,cited_by_count"

    def __init__(self, api_key: str | None):
        self.s = requests.Session()
        self.api_key = api_key
        self.requests = 0

    def _get(self, url: str, params: dict) -> requests.Response:
        if self.api_key:
            params = {**params, "api_key": self.api_key}
        for attempt in range(1, 6):
            try:
                r = self.s.get(url, params=params, timeout=30)
                self.requests += 1
                if r.status_code == 429 or r.status_code >= 500:
                    raise requests.HTTPError(f"HTTP {r.status_code}: {r.text[:200]}")
                return r
            except requests.RequestException as e:
                if attempt == 5:
                    raise
                log(f"  OpenAlex API 失敗（{attempt} 回目）、{10 * attempt} 秒後に再試行: {e}")
                time.sleep(10 * attempt)
        raise AssertionError

    def batch(self, ids: list[int]) -> dict[int, dict]:
        r = self._get("https://api.openalex.org/works", {
            "filter": "openalex_id:" + "|".join(f"W{i}" for i in ids),
            "per-page": str(len(ids)), "select": self.SELECT,
        })
        r.raise_for_status()
        want = set(ids)
        out = {}
        for w in r.json()["results"]:
            wid = int(w["id"].rsplit("/", 1)[-1].lstrip("W"))
            if wid in want:
                out[wid] = w
        return out

    def single(self, paper_id: int) -> dict | None:
        # マージ済みの Work は新しい ID のレコードへリダイレクトされる
        r = self._get(f"https://api.openalex.org/works/W{paper_id}", {"select": self.SELECT})
        if r.status_code == 404:
            return None
        r.raise_for_status()
        return r.json()


def db_path(db_dir: str, db: str) -> str:
    return os.path.join(db_dir, f"{db}.sqlite")


def plan_moves(args) -> dict:
    api = OpenAlex(os.environ.get("OPENALEX_API_KEY") or None)
    candidates: list[tuple[str, int, float, int]] = []
    for db in DB_NAMES:
        c = sqlite3.connect(f"file:{db_path(args.db_dir, db)}?mode=ro", uri=True)
        rows = c.execute("SELECT id, year, cit FROM papers WHERE cit >= ? ORDER BY id", (args.min_cit,)).fetchall()
        c.close()
        log(f"{db}: cit >= {args.min_cit} の論文 {len(rows):,} 件")
        candidates += [(db, *r) for r in rows]
    log(f"検証対象 合計 {len(candidates):,} 件（{BATCH} 件ずつ OpenAlex API に問い合わせ）")

    moves, out_of_range, not_found, merged = [], [], [], []
    for i in range(0, len(candidates), BATCH):
        chunk = candidates[i:i + BATCH]
        fetched = api.batch([c[1] for c in chunk])
        for db, pid, year, cit in chunk:
            w = fetched.get(pid)
            if w is None:
                w = api.single(pid)
                if w is None:
                    not_found.append({"id": pid, "db": db})
                    continue
                merged.append({"id": pid, "merged_into": w["id"].rsplit("/", 1)[-1], "db": db})
            if w.get("publication_year") is None:
                not_found.append({"id": pid, "db": db, "reason": "publication_year なし"})
                continue
            correct = db_for_year(w["publication_year"])
            if correct == db:
                continue
            item = {
                "id": pid, "title": w.get("title"), "from_db": db, "to_db": correct,
                "old_year": year, "new_year": compute_year(w["publication_year"], w.get("publication_date"), pid),
                "old_cit": cit, "new_cit": w["cited_by_count"],
            }
            (moves if correct in DB_NAMES else out_of_range).append(item)
            log(f"  [{'MOVE' if correct in DB_NAMES else '範囲外'}] W{pid} {str(w.get('title'))[:60]!r}: "
                f"{db}(year={year:.2f}) -> {correct}(year={item['new_year']:.2f})")
        if (i // BATCH) % 20 == 0:
            log(f"  {min(i + BATCH, len(candidates)):,}/{len(candidates):,} 件確認")
        time.sleep(0.2)

    plan = {
        "created_at": time.strftime("%F %T"), "min_cit": args.min_cit, "checked": len(candidates),
        "api_requests": api.requests, "moves": moves, "out_of_range": out_of_range,
        "not_found": not_found, "merged": merged,
    }
    log(f"判定完了: 検証 {len(candidates):,} 件 / 移動対象 {len(moves)} 件 / 正しい年が範囲外 {len(out_of_range)} 件 / "
        f"API で見つからず {len(not_found)} 件 / マージ済み {len(merged)} 件 / API リクエスト {api.requests} 回")
    return plan


def move_paper(db_dir: str, m: dict) -> dict:
    pid = m["id"]
    c = sqlite3.connect(db_path(db_dir, m["to_db"]), isolation_level=None)
    c.execute("ATTACH ? AS src", (db_path(db_dir, m["from_db"]),))
    try:
        c.execute("BEGIN")
        if not c.execute("SELECT 1 FROM src.papers WHERE id = ?", (pid,)).fetchone():
            c.execute("ROLLBACK")
            return {"skipped": "移動元に papers が無い（移動済み？）"}
        c.execute("""INSERT OR REPLACE INTO main.papers (id, field, title, year, cit, fwci, subfield, child_cnt, doi)
                     SELECT id, field, title, ?, ?, fwci, subfield, child_cnt, doi FROM src.papers WHERE id = ?""",
                  (m["new_year"], m["new_cit"], pid))
        n = {}
        n["edges"] = c.execute("INSERT OR IGNORE INTO main.edges (cited, citing) "
                               "SELECT cited, citing FROM src.edges WHERE citing = ?", (pid,)).rowcount
        c.execute("DELETE FROM main.authorships WHERE paper_id = ?", (pid,))
        n["authorships"] = c.execute("INSERT INTO main.authorships (paper_id, author_name) "
                                     "SELECT paper_id, author_name FROM src.authorships WHERE paper_id = ? "
                                     "ORDER BY rowid", (pid,)).rowcount
        n["keywords_added"] = c.execute(
            "INSERT OR IGNORE INTO main.keywords (id, name) SELECT k.id, k.name FROM src.keywords k "
            "WHERE k.id IN (SELECT keyword_id FROM src.paper_keywords WHERE paper_id = ?)", (pid,)).rowcount
        n["paper_keywords"] = c.execute("INSERT OR REPLACE INTO main.paper_keywords (paper_id, keyword_id, score) "
                                        "SELECT paper_id, keyword_id, score FROM src.paper_keywords WHERE paper_id = ?",
                                        (pid,)).rowcount
        for sql in ["DELETE FROM src.papers WHERE id = ?", "DELETE FROM src.edges WHERE citing = ?",
                    "DELETE FROM src.authorships WHERE paper_id = ?", "DELETE FROM src.paper_keywords WHERE paper_id = ?"]:
            c.execute(sql, (pid,))
        # 移動元で参照されなくなった keywords は残しても害は無い（アプリは paper_keywords から引く）ので消さない
        c.execute("COMMIT")
        return n
    except Exception:
        c.execute("ROLLBACK")
        raise
    finally:
        c.close()


def verify(db_dir: str, stats_path: str, moves: list[dict]) -> list[str]:
    """移動で変わるのは移動した論文の行だけで、それ以外は export_sqlite.py で検証済み。
    全体には軽いチェック（年代外の論文・全 DB 合計件数・quick_check）だけを行い、移動した論文は個別に確認する
    （全行の NOT EXISTS 結合は、キャッシュが冷えた EBS だと 10 分以上かかったため）。"""
    errors = []
    stats = json.load(open(stats_path))
    totals = {t: 0 for t in TABLES if t != "keywords"}
    conns = {}
    for i, db in enumerate(DB_NAMES, 1):
        start, end = bucket_of(db)
        size = os.path.getsize(db_path(db_dir, db)) / 1e9
        log(f"  [{i}/{len(DB_NAMES)}] {db}（{size:.2f} GB）の検証開始")
        t0 = time.time()
        c = sqlite3.connect(f"file:{db_path(db_dir, db)}?mode=ro", uri=True)
        c.execute("PRAGMA cache_size = -500000")
        conns[db] = c
        for t in totals:
            t1 = time.time()
            n = c.execute(f"SELECT count(*) FROM {t}").fetchone()[0]
            totals[t] += n
            log(f"    {t}: {n:,} 行（{time.time() - t1:.0f}s）")
        v = c.execute("SELECT count(*) FROM papers WHERE field = ? AND (year < ? OR year >= ?)",
                      (FIELD, start, end + 1)).fetchone()[0]
        log(f"    年代外の論文: {v:,}")
        if v:
            errors.append(f"{db}: year_out_of_bucket = {v:,}")
        t1 = time.time()
        # quick_check はファイル全体を読むため一番時間がかかる（キャッシュが冷えた EBS では数 MB/s）
        log(f"    quick_check 開始（ファイル全体を読む。進捗は /proc/{os.getpid()}/io の read_bytes で確認できる）")
        qc = c.execute("PRAGMA quick_check").fetchone()[0]
        log(f"    quick_check: {qc}（{time.time() - t1:.0f}s）")
        if qc != "ok":
            errors.append(f"{db}: quick_check = {qc}")
        log(f"  [{i}/{len(DB_NAMES)}] {db} の検証完了（{time.time() - t0:.0f}s）")
    log("  移動した論文の個別確認")
    # 移動は DB 間の付け替えなので、全 DB 合計は build 時（stats.json）から変わらない
    for t, v in totals.items():
        expected = sum(d[t] for d in stats["dbs"].values())
        if v != expected:
            errors.append(f"全 DB 合計 {t}: {v:,} != stats.json {expected:,}")

    for m in moves:
        pid, src, dst = m["id"], conns[m["from_db"]], conns[m["to_db"]]
        r = m.get("result", {})
        if "skipped" in r:
            continue
        for t, col in [("papers", "id"), ("edges", "citing"), ("authorships", "paper_id"), ("paper_keywords", "paper_id")]:
            left = src.execute(f"SELECT count(*) FROM {t} WHERE {col} = ?", (pid,)).fetchone()[0]
            if left:
                errors.append(f"W{pid}: 移動元 {m['from_db']} に {t} が {left} 行残っている")
        row = dst.execute("SELECT year, cit FROM papers WHERE id = ?", (pid,)).fetchone()
        if row is None or abs(row[0] - m["new_year"]) > 1e-9:
            errors.append(f"W{pid}: 移動先 {m['to_db']} の papers が無いか year が違う: {row}")
        for t, col in [("edges", "citing"), ("authorships", "paper_id"), ("paper_keywords", "paper_id")]:
            n = dst.execute(f"SELECT count(*) FROM {t} WHERE {col} = ?", (pid,)).fetchone()[0]
            if t in r and n != r[t]:
                errors.append(f"W{pid}: 移動先 {t} {n} 行 != 移動した {r[t]} 行")
        d = dst.execute("SELECT count(*) FROM paper_keywords pk LEFT JOIN keywords k ON k.id = pk.keyword_id "
                        "WHERE pk.paper_id = ? AND k.id IS NULL", (pid,)).fetchone()[0]
        if d:
            errors.append(f"W{pid}: 移動先で dangling paper_keywords {d} 行")
    for c in conns.values():
        c.close()
    return errors


def main() -> None:
    ap = argparse.ArgumentParser(description="出版年の誤りで違う年代 DB に入った論文を正しい DB へ移動する")
    ap.add_argument("--work", default=DEFAULT_WORK, help="作業フォルダ（out/stats.json と判定結果の保存先）")
    ap.add_argument("--db-dir", default=DEFAULT_DB_DIR, help="SQLite ファイルのあるディレクトリ")
    ap.add_argument("--min-cit", type=int, default=1000, help="検証対象とする被引用数の下限")
    ap.add_argument("--dry-run", action="store_true", help="判定のみ行い、DB は変更しない")
    ap.add_argument("--verify-only", action="store_true", help="判定・移動をせず検証だけ行う")
    args = ap.parse_args()
    stats_path = os.path.join(args.work, "out", "stats.json")
    plan_path = os.path.join(args.work, "fix_misyeared_plan.json")
    configure(stats_path)
    log(f"field {FIELD}: 対象 DB {', '.join(DB_NAMES)}")

    if args.verify_only:
        plan = json.load(open(plan_path)) if os.path.exists(plan_path) else {"moves": []}
    else:
        plan = plan_moves(args)
        with open(plan_path, "w") as f:
            json.dump(plan, f, indent=1, ensure_ascii=False)
        log(f"判定結果を保存: {plan_path}")
        if args.dry_run:
            return
        for m in plan["moves"]:
            n = move_paper(args.db_dir, m)
            m["result"] = n
            log(f"  moved W{m['id']} {str(m['title'])[:50]!r}: {m['from_db']} -> {m['to_db']} "
                f"year {m['old_year']:.2f} -> {m['new_year']:.2f} cit {m['old_cit']} -> {m['new_cit']} {n}")
        with open(plan_path, "w") as f:
            json.dump(plan, f, indent=1, ensure_ascii=False)
        log(f"移動完了: {len(plan['moves'])} 件")

    log("検証開始")
    errors = verify(args.db_dir, stats_path, plan["moves"] if not args.verify_only or "moves" in plan else [])
    if errors:
        for e in errors:
            log(f"検証NG: {e}")
        sys.exit(1)
    log("検証OK（全 DB で年代外の論文 0・quick_check ok・全 DB 合計件数が stats.json と一致、"
        f"移動した {len(plan['moves'])} 件は移動先に揃い移動元に残っていない）")


if __name__ == "__main__":
    main()
