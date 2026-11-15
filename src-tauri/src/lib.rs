mod commands;
mod constants;
mod db;
mod graph;
#[cfg(test)]
mod graph_tests;
mod paper;
#[cfg(test)]
mod real_data_check;
mod state;
mod types;

use tauri::Manager;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // SQLiteファイルは resources/db/ に同梱する想定（docs/native.md 6章）。
            // EC2上で生成したファイルは、ビルド前に src-tauri/resources/db/ へ配置する。
            let db_dir = app
                .path()
                .resolve("db", tauri::path::BaseDirectory::Resource)
                .expect("resource directory should be resolvable");
            app.manage(AppState { db_dir });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_fields_cmd,
            commands::fetch_top,
            commands::fetch_hubs,
            commands::search_papers,
            commands::fetch_seed_graph,
            commands::fetch_paper,
            commands::fetch_attribution,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
