// Tauri commands（旧 worker/api/*.ts の Hono ルートに対応、docs/native.md 4.4）。
// フロントの src/api.ts は fetch(url) の代わりにこれらを invoke() で呼ぶ。

use tauri::State;

use crate::constants::{
    HUB_MIN_CHILD_DEFAULT, SEARCH_LIMIT_DEFAULT, SEARCH_LIMIT_MAX, SEED_DEPTH_DEFAULT, SEED_DEPTH_MAX,
    TOP_LIMIT_DEFAULT, TOP_LIMIT_MAX,
};
use crate::db::{self, ResolveFieldDbError, YearRange};
use crate::graph;
use crate::paper;
use crate::state::AppState;
use crate::types::{AttributionSource, FieldInfo, PaperDetail, PaperSummary, SeedGraph};

fn field_display_name(field: i64) -> String {
    match field {
        17 => "Computer Science".to_string(),
        12 => "Arts and Humanities".to_string(),
        other => format!("Field {other}"),
    }
}

fn resolve_db_error_to_string(err: ResolveFieldDbError) -> String {
    match err {
        ResolveFieldDbError::InvalidField => "invalid 'field' parameter".to_string(),
        ResolveFieldDbError::InvalidYearRange => "invalid 'yearRange' parameter (expected 'YYYY-YYYY')".to_string(),
        ResolveFieldDbError::NotFound(msg) => msg,
    }
}

fn open_main_and_adjacent(
    state: &AppState,
    field: i64,
    year_range_param: Option<&str>,
) -> Result<(rusqlite::Connection, Vec<rusqlite::Connection>, i64, YearRange), String> {
    let resolved = db::resolve_field_db(field, year_range_param).map_err(resolve_db_error_to_string)?;
    let conn = db::open_readonly(&state.db_dir, resolved.file_name).map_err(|e| e.to_string())?;
    let adjacent_files = db::list_adjacent_db_files(resolved.field, resolved.year_range);
    let adjacent_conns = adjacent_files
        .into_iter()
        .map(|file_name| db::open_readonly(&state.db_dir, file_name).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((conn, adjacent_conns, resolved.field, resolved.year_range))
}

fn row_to_summary(row: &rusqlite::Row) -> rusqlite::Result<PaperSummary> {
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

// 分野・年代レンジの一覧（フロントの切り替えUI用）。
#[tauri::command]
pub fn list_fields_cmd() -> Vec<FieldInfo> {
    db::list_fields()
        .into_iter()
        .map(|field| {
            let default_year_range = db::list_default_year_range(field);
            FieldInfo {
                field,
                name: field_display_name(field),
                year_ranges: db::list_year_ranges(field).into_iter().map(|r| r.format()).collect(),
                default_year_range: default_year_range.map(|r| r.format()),
            }
        })
        .collect()
}

// 被引用の上位論文。
#[tauri::command]
pub fn fetch_top(
    state: State<AppState>,
    field: i64,
    year_range: Option<String>,
    from: Option<i64>,
    limit: Option<i64>,
) -> Result<Vec<PaperSummary>, String> {
    let (conn, _adjacent, field, _yr) = open_main_and_adjacent(&state, field, year_range.as_deref())?;
    let limit = limit.unwrap_or(TOP_LIMIT_DEFAULT).min(TOP_LIMIT_MAX);
    if limit <= 0 {
        return Err("invalid 'limit' parameter".to_string());
    }

    let papers = if let Some(from_year) = from {
        let mut stmt = conn
            .prepare("SELECT id, field, title, year, cit, fwci, subfield, child_cnt, doi FROM papers WHERE field = ? AND year >= ? ORDER BY cit DESC LIMIT ?")
            .map_err(|e| e.to_string())?;
        stmt.query_map((field, from_year, limit), row_to_summary)
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?
    } else {
        let mut stmt = conn
            .prepare("SELECT id, field, title, year, cit, fwci, subfield, child_cnt, doi FROM papers WHERE field = ? ORDER BY cit DESC LIMIT ?")
            .map_err(|e| e.to_string())?;
        stmt.query_map((field, limit), row_to_summary)
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?
    };
    Ok(papers)
}

// 分岐数の上位論文（「ハブ」）。
#[tauri::command]
pub fn fetch_hubs(
    state: State<AppState>,
    field: i64,
    year_range: Option<String>,
    min_child: Option<i64>,
    limit: Option<i64>,
) -> Result<Vec<PaperSummary>, String> {
    let (conn, _adjacent, field, _yr) = open_main_and_adjacent(&state, field, year_range.as_deref())?;
    let min_child = min_child.unwrap_or(HUB_MIN_CHILD_DEFAULT);
    let limit = limit.unwrap_or(TOP_LIMIT_DEFAULT).min(TOP_LIMIT_MAX);
    if min_child < 0 {
        return Err("invalid 'minChild' parameter".to_string());
    }
    if limit <= 0 {
        return Err("invalid 'limit' parameter".to_string());
    }

    let mut stmt = conn
        .prepare("SELECT id, field, title, year, cit, fwci, subfield, child_cnt, doi FROM papers WHERE field = ? AND child_cnt >= ? ORDER BY child_cnt DESC LIMIT ?")
        .map_err(|e| e.to_string())?;
    let papers = stmt
        .query_map((field, min_child, limit), row_to_summary)
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    Ok(papers)
}

// 論文名・著者名・キーワードの検索。LIKE方式（旧 worker/api/search.ts と同じSQL構造）。
// 著者名・キーワードはEXISTSサブクエリで判定する（1論文に複数マッチして行が増えるのを避けるため）。
#[tauri::command]
pub fn search_papers(
    state: State<AppState>,
    field: i64,
    year_range: Option<String>,
    q: String,
    limit: Option<i64>,
) -> Result<Vec<PaperSummary>, String> {
    let (conn, _adjacent, field, _yr) = open_main_and_adjacent(&state, field, year_range.as_deref())?;
    if q.trim().is_empty() {
        return Err("'q' parameter is required".to_string());
    }
    let limit = limit.unwrap_or(SEARCH_LIMIT_DEFAULT).min(SEARCH_LIMIT_MAX);
    if limit <= 0 {
        return Err("invalid 'limit' parameter".to_string());
    }

    let escaped = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    let pattern = format!("%{escaped}%");

    let mut stmt = conn
        .prepare(
            "SELECT p.id, p.field, p.title, p.year, p.cit, p.fwci, p.subfield, p.child_cnt, p.doi
             FROM papers p
             WHERE p.field = ?
               AND (
                 p.title LIKE ? ESCAPE '\\'
                 OR EXISTS (SELECT 1 FROM authorships a WHERE a.paper_id = p.id AND a.author_name LIKE ? ESCAPE '\\')
                 OR EXISTS (
                   SELECT 1 FROM paper_keywords pk JOIN keywords k ON k.id = pk.keyword_id
                   WHERE pk.paper_id = p.id AND k.name LIKE ? ESCAPE '\\'
                 )
               )
             ORDER BY p.cit DESC LIMIT ?",
        )
        .map_err(|e| e.to_string())?;
    let papers = stmt
        .query_map((field, &pattern, &pattern, &pattern, limit), row_to_summary)
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    Ok(papers)
}

// シード周辺のグラフ（旧 worker/api/seed.ts。R2キャッシュ層は不要と判断し省略、docs/native.md 2.3）。
#[tauri::command]
pub fn fetch_seed_graph(
    state: State<AppState>,
    field: i64,
    year_range: Option<String>,
    seed_id: i64,
    depth: Option<u32>,
    min_cit: Option<i64>,
) -> Result<SeedGraph, String> {
    let (conn, adjacent, _field, _yr) = open_main_and_adjacent(&state, field, year_range.as_deref())?;
    let depth = depth.unwrap_or(SEED_DEPTH_DEFAULT).min(SEED_DEPTH_MAX);
    if depth == 0 {
        return Err("invalid 'depth' parameter".to_string());
    }
    let min_cit = min_cit.unwrap_or(0);
    if min_cit < 0 {
        return Err("invalid 'minCit' parameter".to_string());
    }

    graph::build_seed_graph(&conn, &adjacent, seed_id, depth, min_cit)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "paper not found".to_string())
}

// 論文の詳細と系譜（祖先の列）。
#[tauri::command]
pub fn fetch_paper(
    state: State<AppState>,
    field: i64,
    year_range: Option<String>,
    id: i64,
) -> Result<PaperDetail, String> {
    let (conn, adjacent, _field, _yr) = open_main_and_adjacent(&state, field, year_range.as_deref())?;

    let (paper_summary, paper_db) =
        paper::resolve_paper(&conn, &adjacent, id).map_err(|e| e.to_string())?.ok_or_else(|| "paper not found".to_string())?;

    let lineage = paper::resolve_lineage(paper_db, &adjacent, id).map_err(|e| e.to_string())?;
    let authors = paper::fetch_authors(paper_db, id).map_err(|e| e.to_string())?;
    let keywords = paper::fetch_keywords(paper_db, id).map_err(|e| e.to_string())?;

    Ok(PaperDetail { paper: paper_summary, lineage, authors, keywords })
}

// 利用データの出典表示（静的データ、旧 worker/api/attribution.ts）。
#[tauri::command]
pub fn fetch_attribution() -> Vec<AttributionSource> {
    vec![
        AttributionSource {
            name: "OpenAlex".to_string(),
            url: "https://openalex.org".to_string(),
            license: "CC0".to_string(),
            note: "論文・分野・被引用の基本データ".to_string(),
        },
        AttributionSource {
            name: "Semantic Scholar".to_string(),
            url: "https://www.semanticscholar.org".to_string(),
            license: "利用条件は個別確認".to_string(),
            note: "引用意図・影響力のある引用の判定（補助データ、未連携）".to_string(),
        },
    ]
}
