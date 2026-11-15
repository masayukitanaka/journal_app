// SQLiteファイルの配置ディレクトリを保持するアプリ状態（docs/native.md 4.2, 6章）。
// コマンドは毎回ここから必要なDBファイルを読み取り専用で開く（接続プールは持たない。
// 単一ユーザーのローカルアプリであり、クエリ自体がマイクロ秒〜数msで終わるため、
// 接続オーバーヘッドより実装の単純さを優先している）。

use std::path::PathBuf;

pub struct AppState {
    pub db_dir: PathBuf,
}
