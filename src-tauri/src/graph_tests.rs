// build_seed_graph の動作確認用テスト（docs/native.md の移植が正しいかの検証）。
#[cfg(test)]
mod tests {
    use crate::graph::build_seed_graph;
    use rusqlite::Connection;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE papers (
                id INTEGER PRIMARY KEY, field INTEGER NOT NULL, title TEXT NOT NULL,
                year REAL NOT NULL, cit INTEGER NOT NULL, fwci REAL, subfield INTEGER,
                child_cnt INTEGER NOT NULL DEFAULT 0, doi TEXT
            );
            CREATE TABLE edges (cited INTEGER NOT NULL, citing INTEGER NOT NULL, PRIMARY KEY (cited, citing)) WITHOUT ROWID;

            INSERT INTO papers (id, field, title, year, cit, fwci, subfield, child_cnt, doi) VALUES
                (1, 17, 'Seed Paper', 2022.0, 100, 5.0, 1701, 2, NULL),
                (2, 17, 'Child One', 2023.0, 50, 2.0, 1701, 1, NULL),
                (3, 17, 'Child Two', 2023.5, 30, 1.0, 1701, 0, NULL),
                (4, 17, 'Grandchild', 2024.0, 10, 0.5, 1701, 0, NULL);

            INSERT INTO edges (cited, citing) VALUES (1, 2), (1, 3), (2, 4);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn builds_seed_graph_with_depth_1() {
        let conn = setup_test_db();
        let graph = build_seed_graph(&conn, &[], 1, 1, 0).unwrap().expect("seed paper should exist");

        assert_eq!(graph.seed, 1);
        assert_eq!(graph.field, 17);
        // depth=1: seed(1) + 直接の子(2,3) の3ノード、grandchild(4)は含まれない
        let ids: std::collections::HashSet<i64> = graph.nodes.iter().map(|n| n.id).collect();
        assert_eq!(ids, std::collections::HashSet::from([1, 2, 3]));
        assert_eq!(graph.edges.len(), 2);
    }

    #[test]
    fn builds_seed_graph_with_depth_2_includes_grandchild() {
        let conn = setup_test_db();
        let graph = build_seed_graph(&conn, &[], 1, 2, 0).unwrap().unwrap();
        let ids: std::collections::HashSet<i64> = graph.nodes.iter().map(|n| n.id).collect();
        assert_eq!(ids, std::collections::HashSet::from([1, 2, 3, 4]));
        assert_eq!(graph.edges.len(), 3);
    }

    #[test]
    fn returns_none_for_missing_seed() {
        let conn = setup_test_db();
        let graph = build_seed_graph(&conn, &[], 9999, 1, 0).unwrap();
        assert!(graph.is_none());
    }

    #[test]
    fn min_cit_filters_low_citation_nodes() {
        let conn = setup_test_db();
        // min_cit=40: child_two(cit=30)は除外されるが、seed自身は常に含まれる
        let graph = build_seed_graph(&conn, &[], 1, 1, 40).unwrap().unwrap();
        let ids: std::collections::HashSet<i64> = graph.nodes.iter().map(|n| n.id).collect();
        assert!(ids.contains(&1));
        assert!(ids.contains(&2)); // cit=50 >= 40
        assert!(!ids.contains(&3)); // cit=30 < 40
    }
}
