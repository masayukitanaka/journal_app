// API 応答の型（旧 worker/db.ts の PaperRow, worker/graph.ts の SeedGraph, src/types.ts に対応）。
// フロントの src/types.ts と同じ shape を維持する（camelCase はフロント側の wrapper で揃える）。

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct PaperSummary {
    pub id: i64,
    pub field: i64,
    pub title: String,
    pub year: f64, // 月単位精度のため REAL（migrations/0002: year + (month-1)/12）
    pub cit: i64,
    pub fwci: Option<f64>,
    pub subfield: Option<i64>,
    pub child_cnt: i64,
    pub doi: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphNode {
    pub id: i64,
    pub y: f64,
    pub t: String,
    pub c: i64,
    pub k: i64,
    // 隣接する年代レンジの DB から補完されたノードか（省略時/false はメイン DB 由来）。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub a: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeedGraph {
    pub seed: i64,
    pub field: i64,
    #[serde(rename = "generatedAt")]
    pub generated_at: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<(i64, i64)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct KeywordInfo {
    pub name: String,
    pub score: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaperDetail {
    pub paper: PaperSummary,
    pub lineage: Vec<PaperSummary>,
    pub authors: Vec<String>,
    pub keywords: Vec<KeywordInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldInfo {
    pub field: i64,
    pub name: String,
    #[serde(rename = "yearRanges")]
    pub year_ranges: Vec<String>,
    #[serde(rename = "defaultYearRange")]
    pub default_year_range: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttributionSource {
    pub name: String,
    pub url: String,
    pub license: String,
    pub note: String,
}
