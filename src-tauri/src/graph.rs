// シード周辺グラフの構築（旧 worker/graph.ts の buildSeedGraph に対応、docs/native.md 4.3）。
//
// D1時代はバインド変数上限100・ネットワークラウンドトリップ・レート制限を回避するために
// チャンク分割（SQL_IN_CHUNK_SIZE=98）・並列バッチ（MAX_CONCURRENT_CHUNKS）・R2キャッシュを
// 導入していた。ネットワークラウンドトリップ・レート制限はローカルSQLiteには存在しないため
// 並列バッチ・R2キャッシュは撤去したが、IN句のチャンク分割は実データで検証した結果、
// 必要と判明した（2026-11-15）。rusqlite の bundled SQLite は SQLITE_LIMIT_VARIABLE_NUMBER の
// 既定値が 999 で、被引用数の多い論文（実測: 子76,658件）をシードにすると
// "too many SQL variables" で失敗する。D1 の 100 よりは大幅に緩いため、チャンクサイズは
// 900（999 に対して安全マージンを残す）に拡大している。

use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

use crate::types::{GraphNode, SeedGraph};

// SPEC.md 6.5「実データでは、描画ノード数を2,000件程度までに抑える」。
const MAX_NODES: usize = 2000;

// rusqlite(bundled) の SQLITE_LIMIT_VARIABLE_NUMBER 既定値（999）に対する安全マージン。
const SQL_IN_CHUNK_SIZE: usize = 900;

struct PaperLookupRow {
    id: i64,
    year: f64,
    title: String,
    cit: i64,
    child_cnt: i64,
}

struct PaperLookupRowWithSource {
    row: PaperLookupRow,
    adjacent: bool,
}

fn fetch_edges(conn: &Connection, ids: &[i64], column: &str) -> rusqlite::Result<Vec<(i64, i64)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut rows = Vec::new();
    for chunk in ids.chunks(SQL_IN_CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT cited, citing FROM edges WHERE {column} IN ({placeholders})");
        let mut stmt = conn.prepare(&sql)?;
        let params = rusqlite::params_from_iter(chunk.iter());
        rows.extend(
            stmt.query_map(params, |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    Ok(rows)
}

// edges は citing 側のDBに保存される方針のため、frontierノードをcited側として子孫を探す場合、
// citing側（子孫）が新しい年代レンジの隣接DBに保存されていることがある（祖先方向は不要）。
fn fetch_edges_with_adjacent(
    conn: &Connection,
    adjacent_conns: &[Connection],
    ids: &[i64],
    column: &str,
) -> rusqlite::Result<Vec<(i64, i64)>> {
    let mut rows = fetch_edges(conn, ids, column)?;
    if column == "citing" || adjacent_conns.is_empty() {
        return Ok(rows);
    }
    let mut seen: HashSet<(i64, i64)> = rows.iter().copied().collect();
    for adjacent in adjacent_conns {
        for edge in fetch_edges(adjacent, ids, column)? {
            if seen.insert(edge) {
                rows.push(edge);
            }
        }
    }
    Ok(rows)
}

fn fetch_papers(conn: &Connection, ids: &[i64], min_cit: i64) -> rusqlite::Result<Vec<PaperLookupRow>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // min_cit 用に1枠残す（D1時代の SQL_IN_CHUNK_SIZE 設計と同じ考え方）。
    let mut rows = Vec::new();
    for chunk in ids.chunks(SQL_IN_CHUNK_SIZE - 1) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT id, year, title, cit, child_cnt FROM papers WHERE id IN ({placeholders}) AND cit >= ?");
        let mut stmt = conn.prepare(&sql)?;
        let mut params: Vec<&dyn rusqlite::ToSql> = chunk.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        params.push(&min_cit);
        rows.extend(
            stmt.query_map(params.as_slice(), |row| {
                Ok(PaperLookupRow {
                    id: row.get(0)?,
                    year: row.get(1)?,
                    title: row.get(2)?,
                    cit: row.get(3)?,
                    child_cnt: row.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    Ok(rows)
}

// 現在のDBで見つからなかった論文を、隣接する年代レンジのDB（前後1つずつ）からも探す。
// edges は citing 側のDBに保存される方針のため、祖先（cited）が前の年代レンジのDBにいることがある。
fn fetch_papers_with_adjacent(
    conn: &Connection,
    adjacent_conns: &[Connection],
    ids: &[i64],
    min_cit: i64,
) -> rusqlite::Result<Vec<PaperLookupRowWithSource>> {
    let main_rows = fetch_papers(conn, ids, min_cit)?;
    let mut found_ids: HashSet<i64> = main_rows.iter().map(|r| r.id).collect();
    let mut rows: Vec<PaperLookupRowWithSource> =
        main_rows.into_iter().map(|row| PaperLookupRowWithSource { row, adjacent: false }).collect();

    let mut missing_ids: Vec<i64> = ids.iter().copied().filter(|id| !found_ids.contains(id)).collect();
    if missing_ids.is_empty() || adjacent_conns.is_empty() {
        return Ok(rows);
    }

    for adjacent in adjacent_conns {
        if missing_ids.is_empty() {
            break;
        }
        let extra = fetch_papers(adjacent, &missing_ids, min_cit)?;
        for row in extra {
            if found_ids.insert(row.id) {
                missing_ids.retain(|id| *id != row.id);
                rows.push(PaperLookupRowWithSource { row, adjacent: true });
            }
        }
    }
    Ok(rows)
}

// DBから祖先・子孫方向に depth まで BFS し、シード周辺グラフを構築する。
// adjacent_conns: 年代境界をまたぐ系譜を補完するための隣接DB（前後1レンジ分、省略時は補完なし）。
pub fn build_seed_graph(
    conn: &Connection,
    adjacent_conns: &[Connection],
    seed: i64,
    depth: u32,
    min_cit: i64,
) -> rusqlite::Result<Option<SeedGraph>> {
    let seed_paper = conn
        .query_row(
            "SELECT id, field, title, year, cit, child_cnt FROM papers WHERE id = ?",
            [seed],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .ok();

    let (seed_id, field, title, year, cit, child_cnt) = match seed_paper {
        Some(v) => v,
        None => return Ok(None),
    };

    let mut node_cit: HashMap<i64, i64> = HashMap::new();
    node_cit.insert(seed_id, cit);
    let mut edge_set: HashSet<(i64, i64)> = HashSet::new();
    let mut edges: Vec<(i64, i64)> = Vec::new();

    let mut frontier = vec![seed_id];
    for _ in 0..depth {
        if frontier.is_empty() || node_cit.len() >= MAX_NODES {
            break;
        }

        let ancestor_rows = fetch_edges_with_adjacent(conn, adjacent_conns, &frontier, "citing")?;
        let descendant_rows = fetch_edges_with_adjacent(conn, adjacent_conns, &frontier, "cited")?;

        let mut new_ids: Vec<i64> = Vec::new();
        for (cited, citing) in ancestor_rows.into_iter().chain(descendant_rows) {
            if edge_set.insert((cited, citing)) {
                edges.push((cited, citing));
            }
            if !node_cit.contains_key(&cited) {
                new_ids.push(cited);
            }
            if !node_cit.contains_key(&citing) {
                new_ids.push(citing);
            }
        }

        let mut unique_new_ids: Vec<i64> = new_ids.into_iter().collect::<HashSet<_>>().into_iter().collect();
        if unique_new_ids.is_empty() {
            break;
        }
        unique_new_ids.sort_unstable();

        // ノード数が上限を超える場合、新規ノードを被引用数上位に絞って打ち切る。
        let mut new_papers = fetch_papers_with_adjacent(conn, adjacent_conns, &unique_new_ids, 0)?;
        new_papers.sort_by(|a, b| b.row.cit.cmp(&a.row.cit));
        let remaining_capacity = MAX_NODES.saturating_sub(node_cit.len());
        new_papers.truncate(remaining_capacity);

        for p in &new_papers {
            node_cit.insert(p.row.id, p.row.cit);
        }
        frontier = new_papers.iter().map(|p| p.row.id).collect();
    }

    let id_list: Vec<i64> = node_cit.keys().copied().collect();
    let mut paper_rows = fetch_papers_with_adjacent(conn, adjacent_conns, &id_list, min_cit)?;

    let mut included_ids: HashSet<i64> = paper_rows.iter().map(|p| p.row.id).collect();
    // seed自身は min_cit に関わらず常に含める（メインDBの論文のはずなので adjacent: false）。
    if !included_ids.contains(&seed_id) {
        included_ids.insert(seed_id);
        paper_rows.push(PaperLookupRowWithSource {
            row: PaperLookupRow { id: seed_id, year, title: title.clone(), cit, child_cnt },
            adjacent: false,
        });
    }

    let filtered_edges: Vec<(i64, i64)> = edges
        .into_iter()
        .filter(|(cited, citing)| included_ids.contains(cited) && included_ids.contains(citing))
        .collect();

    Ok(Some(SeedGraph {
        seed: seed_id,
        field,
        generated_at: now_iso8601(),
        nodes: paper_rows
            .into_iter()
            .map(|p| GraphNode { id: p.row.id, y: p.row.year, t: p.row.title, c: p.row.cit, k: p.row.child_cnt, a: p.adjacent })
            .collect(),
        edges: filtered_edges,
    }))
}

fn now_iso8601() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S.%3fZ").to_string()
}
