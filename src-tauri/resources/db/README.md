# SQLiteデータファイルの配置場所

EC2インスタンス上で生成した以下の5ファイルを、ビルド前にこのディレクトリへ配置してください
（手順は docs/CREATE_DB.md）。本リポジトリにはサイズの都合でコミットしません（`.gitignore` 参照）。

- `field17_2006_2010.sqlite`
- `field17_2011_2015.sqlite`
- `field17_2016_2020.sqlite`
- `field17_2021_2025.sqlite`
- `field17_2026_2030.sqlite`

配置後、`npm run tauri dev` / `npm run tauri build` で自動的にバンドルされます
（`src-tauri/tauri.conf.json` の `bundle.resources` 設定による）。
