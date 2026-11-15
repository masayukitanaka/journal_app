// 実データ（EC2生成・S3配布のSQLite）を使った動作確認用テスト。CIでは実行しない
// （resources/db/*.sqlite は .gitignore 対象で配布物にしか存在しないため、ファイルが無い環境では自動skip）。
#[cfg(test)]
mod tests {
    use crate::db;
    use crate::graph::build_seed_graph;
    use crate::paper;
    use rusqlite::Connection;
    use std::path::PathBuf;

    fn db_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/db")
    }

    fn open_db(file_name: &str) -> Option<Connection> {
        let dir = db_dir();
        let path = dir.join(file_name);
        if !path.exists() {
            eprintln!("skip: {} not found (real data not downloaded)", path.display());
            return None;
        }
        Some(db::open_readonly(&dir, file_name).unwrap())
    }

    fn open_main() -> Option<Connection> {
        open_db("field17_2021_2025.sqlite")
    }

    #[test]
    fn fields_and_year_ranges_resolve() {
        let fields = db::list_fields();
        assert_eq!(fields, vec![17, 22]);
        for field in [17, 22] {
            let ranges = db::list_year_ranges(field);
            assert_eq!(ranges.len(), 5);
            assert_eq!(ranges[0].format(), "2006-2010");
            assert_eq!(ranges[4].format(), "2026-2030");
        }
    }

    #[test]
    fn build_seed_graph_on_field22_real_top_paper() {
        let Some(conn) = open_db("field22_2021_2025.sqlite") else { return };
        let seed_id: i64 = conn
            .query_row("SELECT id FROM papers WHERE field = 22 ORDER BY cit DESC LIMIT 1", [], |r| r.get(0))
            .unwrap();

        let adjacent_files = db::list_adjacent_db_files(22, db::YearRange { start: 2021, end: 2025 });
        let dir = db_dir();
        let adjacent: Vec<Connection> =
            adjacent_files.iter().map(|f| db::open_readonly(&dir, f).unwrap()).collect();

        let graph = build_seed_graph(&conn, &adjacent, seed_id, 1, 0).unwrap().expect("seed paper should exist");
        assert_eq!(graph.seed, seed_id);
        assert!(!graph.nodes.is_empty());
        println!("field22 seed={} nodes={} edges={}", graph.seed, graph.nodes.len(), graph.edges.len());
    }

    #[test]
    fn top_cited_query_returns_rows() {
        let Some(conn) = open_main() else { return };
        let mut stmt = conn
            .prepare("SELECT id, title, cit FROM papers WHERE field = 17 ORDER BY cit DESC LIMIT 5")
            .unwrap();
        let rows: Vec<(i64, String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(rows.len(), 5);
        // 降順であることを確認
        for w in rows.windows(2) {
            assert!(w[0].2 >= w[1].2);
        }
        println!("top cited: {:?}", rows);
    }

    #[test]
    fn build_seed_graph_on_real_top_paper() {
        let Some(conn) = open_main() else { return };
        // 被引用1位の論文をシードに depth=1 で BFS する
        let seed_id: i64 = conn
            .query_row("SELECT id FROM papers WHERE field = 17 ORDER BY cit DESC LIMIT 1", [], |r| r.get(0))
            .unwrap();

        let adjacent_files = db::list_adjacent_db_files(17, db::YearRange { start: 2021, end: 2025 });
        let dir = db_dir();
        let adjacent: Vec<Connection> =
            adjacent_files.iter().map(|f| db::open_readonly(&dir, f).unwrap()).collect();

        let graph = build_seed_graph(&conn, &adjacent, seed_id, 1, 0).unwrap().expect("seed paper should exist");
        assert_eq!(graph.seed, seed_id);
        assert!(!graph.nodes.is_empty());
        println!("seed={} nodes={} edges={}", graph.seed, graph.nodes.len(), graph.edges.len());
    }

    #[test]
    fn fetch_paper_detail_on_real_paper() {
        let Some(conn) = open_main() else { return };
        let seed_id: i64 = conn
            .query_row("SELECT id FROM papers WHERE field = 17 ORDER BY cit DESC LIMIT 1", [], |r| r.get(0))
            .unwrap();

        let (summary, paper_db) = paper::resolve_paper(&conn, &[], seed_id).unwrap().expect("paper should resolve");
        assert_eq!(summary.id, seed_id);

        let authors = paper::fetch_authors(paper_db, seed_id).unwrap();
        let keywords = paper::fetch_keywords(paper_db, seed_id).unwrap();
        println!("paper={} authors={} keywords={}", summary.title, authors.len(), keywords.len());
    }

    #[test]
    fn search_finds_known_title_substring() {
        let Some(conn) = open_main() else { return };
        let pattern = "%Transformer%";
        let mut stmt = conn
            .prepare("SELECT COUNT(*) FROM papers WHERE field = 17 AND title LIKE ?1")
            .unwrap();
        let count: i64 = stmt.query_row([pattern], |r| r.get(0)).unwrap();
        assert!(count > 0, "expected at least one paper matching Transformer");
        println!("search 'Transformer' matches: {}", count);
    }
}
