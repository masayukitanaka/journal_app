// 論文詳細・系譜・著者・キーワード取得（旧 worker/api/paper.ts に対応）。

use rusqlite::Connection;

use crate::types::{KeywordInfo, PaperSummary};

pub fn fetch_paper_by_id(conn: &Connection, id: i64) -> rusqlite::Result<Option<PaperSummary>> {
    conn.query_row(
        "SELECT id, field, title, year, cit, fwci, subfield, child_cnt, doi FROM papers WHERE id = ?",
        [id],
        row_to_paper_summary,
    )
    .map(Some)
    .or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(None) } else { Err(e) })
}

fn fetch_papers_by_ids(conn: &Connection, ids: &[i64]) -> rusqlite::Result<Vec<PaperSummary>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT id, field, title, year, cit, fwci, subfield, child_cnt, doi FROM papers WHERE id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let params = rusqlite::params_from_iter(ids.iter());
    let rows = stmt.query_map(params, row_to_paper_summary)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn row_to_paper_summary(row: &rusqlite::Row) -> rusqlite::Result<PaperSummary> {
    Ok(PaperSummary {
        id: row.get(0)?,
        field: row.get(1)?,
        title: row.get(2)?,
        year: row.get(3)?,
        cit: row.get(4)?,
        fwci: row.get(5)?,
        subfield: row.get(6)?,
        child_cnt: row.get(7)?,
        doi: row.get(8)?,
    })
}

// 現在のDB・隣接DBの順に論文を探す（隣接DBから補完されたノードは現在のメインDBに存在しないことがある）。
pub fn resolve_paper<'a>(
    conn: &'a Connection,
    adjacent_conns: &'a [Connection],
    id: i64,
) -> rusqlite::Result<Option<(PaperSummary, &'a Connection)>> {
    if let Some(p) = fetch_paper_by_id(conn, id)? {
        return Ok(Some((p, conn)));
    }
    for adjacent in adjacent_conns {
        if let Some(p) = fetch_paper_by_id(adjacent, id)? {
            return Ok(Some((p, adjacent)));
        }
    }
    Ok(None)
}

// 著者一覧（OpenAlex の authorships 配列順。rowid が ingest 時の INSERT 順＝元データの著者順と一致する）。
pub fn fetch_authors(conn: &Connection, paper_id: i64) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT author_name FROM authorships WHERE paper_id = ? ORDER BY rowid")?;
    let rows = stmt.query_map([paper_id], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// キーワード一覧（関連度スコア降順）。
pub fn fetch_keywords(conn: &Connection, paper_id: i64) -> rusqlite::Result<Vec<KeywordInfo>> {
    let mut stmt = conn.prepare(
        "SELECT k.name, pk.score FROM paper_keywords pk JOIN keywords k ON k.id = pk.keyword_id
         WHERE pk.paper_id = ? ORDER BY pk.score DESC",
    )?;
    let rows = stmt
        .query_map([paper_id], |row| Ok(KeywordInfo { name: row.get(0)?, score: row.get(1)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// シードから起点までの祖先の列。最も被引用数の多い祖先を1本の経路として辿る
// （複数祖先がある場合の経路選択はPoCの簡易仕様）。
// 祖先（cited側）が現在のDBに無い場合、隣接DB（前後1年代レンジ）も探す。
// edges自体は常にciting側（現在のノード）のDBに保存されている前提のため、edgesの検索は
// 現在のDBだけで足りる（境界をまたぐのはpapersの参照のみ）。
pub fn resolve_lineage(conn: &Connection, adjacent_conns: &[Connection], start_id: i64) -> rusqlite::Result<Vec<PaperSummary>> {
    let mut lineage: Vec<PaperSummary> = Vec::new();
    let mut visited: std::collections::HashSet<i64> = std::collections::HashSet::from([start_id]);
    let mut current_id = start_id;

    for _ in 0..20 {
        let mut stmt = conn.prepare("SELECT cited FROM edges WHERE citing = ?")?;
        let candidate_edges: Vec<i64> =
            stmt.query_map([current_id], |row| row.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;

        if candidate_edges.is_empty() {
            break;
        }

        let candidate_ids: Vec<i64> = candidate_edges.into_iter().filter(|id| !visited.contains(id)).collect();
        if candidate_ids.is_empty() {
            break;
        }

        let mut candidates: Vec<PaperSummary> = Vec::new();
        let mut seen_candidate_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
        for db in std::iter::once(conn).chain(adjacent_conns.iter()) {
            for row in fetch_papers_by_ids(db, &candidate_ids)? {
                if seen_candidate_ids.insert(row.id) {
                    candidates.push(row);
                }
            }
        }
        if candidates.is_empty() {
            break;
        }

        // 最も被引用数の多い祖先を1本選ぶ（元のロジックと同じ方針）。
        candidates.sort_by(|a, b| b.cit.cmp(&a.cit));
        let ancestor = candidates.into_iter().next().unwrap();
        visited.insert(ancestor.id);
        current_id = ancestor.id;
        lineage.push(ancestor);
    }

    Ok(lineage)
}
