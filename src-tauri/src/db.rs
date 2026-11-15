// SQLiteファイルの解決（旧 worker/db.ts の DB_REGISTRY に対応、docs/native.md 4.2）。
// D1 binding の代わりに、分野・年代レンジ → SQLiteファイル名の対応表で管理する。
// ファイルは起動時にアプリのリソースディレクトリ（resources/db/）から読み取り専用で開く。

use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YearRange {
    pub start: i32,
    pub end: i32,
}

impl YearRange {
    pub fn format(&self) -> String {
        format!("{}-{}", self.start, self.end)
    }

    pub fn parse(value: &str) -> Option<YearRange> {
        let (start, end) = value.split_once('-')?;
        Some(YearRange {
            start: start.parse().ok()?,
            end: end.parse().ok()?,
        })
    }
}

struct DbRegistryEntry {
    field: i64,
    year_range: YearRange,
    file_name: &'static str,
}

// 分野・年代レンジ → SQLiteファイル名の対応表。新しいDBを追加したらここに1行足す。
// 先頭に書いたエントリが、yearRange 未指定時のその分野の既定値になる（resolve_entry 参照）。
const DB_REGISTRY: &[DbRegistryEntry] = &[
    DbRegistryEntry { field: 17, year_range: YearRange { start: 2021, end: 2025 }, file_name: "field17_2021_2025.sqlite" },
    DbRegistryEntry { field: 17, year_range: YearRange { start: 2006, end: 2010 }, file_name: "field17_2006_2010.sqlite" },
    DbRegistryEntry { field: 17, year_range: YearRange { start: 2011, end: 2015 }, file_name: "field17_2011_2015.sqlite" },
    DbRegistryEntry { field: 17, year_range: YearRange { start: 2016, end: 2020 }, file_name: "field17_2016_2020.sqlite" },
    DbRegistryEntry { field: 17, year_range: YearRange { start: 2026, end: 2030 }, file_name: "field17_2026_2030.sqlite" },
];

pub fn list_fields() -> Vec<i64> {
    let mut fields: Vec<i64> = DB_REGISTRY.iter().map(|e| e.field).collect();
    fields.sort_unstable();
    fields.dedup();
    fields
}

// 年代順（開始年の昇順）に並べて返す。UI の選択肢は年代順の方が分かりやすい。
pub fn list_year_ranges(field: i64) -> Vec<YearRange> {
    let mut ranges: Vec<YearRange> = DB_REGISTRY
        .iter()
        .filter(|e| e.field == field)
        .map(|e| e.year_range)
        .collect();
    ranges.sort_by_key(|r| r.start);
    ranges
}

// yearRange 未指定時に実際に使われる既定レンジ（DB_REGISTRY のその分野の先頭エントリ）。
pub fn list_default_year_range(field: i64) -> Option<YearRange> {
    DB_REGISTRY.iter().find(|e| e.field == field).map(|e| e.year_range)
}

fn resolve_entry(field: i64, year_range: Option<YearRange>) -> Option<&'static DbRegistryEntry> {
    match year_range {
        Some(yr) => DB_REGISTRY.iter().find(|e| e.field == field && e.year_range == yr),
        None => DB_REGISTRY.iter().find(|e| e.field == field),
    }
}

pub enum ResolveFieldDbError {
    InvalidField,
    InvalidYearRange,
    NotFound(String),
}

pub struct ResolvedFieldDb {
    pub field: i64,
    pub year_range: YearRange,
    pub file_name: &'static str,
}

// API コマンドの共通処理: field・yearRange（"2016-2026" 形式、省略可）を解決する。
pub fn resolve_field_db(field: i64, year_range: Option<&str>) -> Result<ResolvedFieldDb, ResolveFieldDbError> {
    if field <= 0 {
        return Err(ResolveFieldDbError::InvalidField);
    }
    let parsed_year_range = match year_range {
        Some(s) => Some(YearRange::parse(s).ok_or(ResolveFieldDbError::InvalidYearRange)?),
        None => None,
    };
    let entry = resolve_entry(field, parsed_year_range).ok_or_else(|| {
        ResolveFieldDbError::NotFound(format!(
            "no database registered for field={}{}",
            field,
            parsed_year_range.map(|yr| format!(" yearRange={}", yr.format())).unwrap_or_default()
        ))
    })?;
    Ok(ResolvedFieldDb { field: entry.field, year_range: entry.year_range, file_name: entry.file_name })
}

// 年代境界をまたぐ引用関係への最小対応（旧 worker/db.ts の listAdjacentDbs）。
// edges は citing 側の DB に保存されるため、祖先（cited）が1つ前の年代レンジのDBにいることがある。
// 登録済み全DBを毎回横断するのではなく、前後1年代レンジ分だけを「隣接DB」として返す。
pub fn list_adjacent_db_files(field: i64, year_range: YearRange) -> Vec<&'static str> {
    DB_REGISTRY
        .iter()
        .filter(|e| e.field == field)
        .filter(|e| e.year_range.end == year_range.start - 1 || e.year_range.start == year_range.end + 1)
        .map(|e| e.file_name)
        .collect()
}

// アプリのリソースディレクトリ（resources/db/）配下の SQLite ファイルを読み取り専用で開く。
// docs/native.md 6章: ファイルは EC2 上で生成し、ビルド時に src-tauri/resources/db/ に配置する想定。
pub fn open_readonly(db_dir: &Path, file_name: &str) -> rusqlite::Result<Connection> {
    let path: PathBuf = db_dir.join(file_name);
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}
