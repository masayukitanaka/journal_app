#!/usr/bin/env bash
# 生成した SQLite ファイル（5 つ）と SHA256SUMS を S3 にアップロードし、サイズを照合する（docs/CREATE_DB.md 3.5）。
# アプリはここからファイルを取得する。
#
#   scripts/db/upload_s3.sh
#
# 環境変数:
#   OUT      SQLite のあるディレクトリ（既定: src-tauri/resources/db）
#   S3_DEST  アップロード先プレフィックス（既定: s3://970173663066-public-us-east-1/journal_app/）
#
# 書き込み権限のある AWS 認証情報（インスタンスロール等）が必要。
set -euo pipefail
cd "$(dirname "$0")/../.."
OUT=${OUT:-src-tauri/resources/db}
S3_DEST=${S3_DEST:-s3://970173663066-public-us-east-1/journal_app/}
S3_DEST=${S3_DEST%/}/
FILES=(field17_2006_2010.sqlite field17_2011_2015.sqlite field17_2016_2020.sqlite field17_2021_2025.sqlite field17_2026_2030.sqlite)

log() { echo "[$(date '+%F %T')] $*"; }

for f in "${FILES[@]}"; do
  [ -f "$OUT/$f" ] || { log "見つかりません: $OUT/$f"; exit 1; }
done
aws sts get-caller-identity --query Arn --output text >/dev/null \
  || { log "AWS 認証情報がありません（インスタンスロールの付与か aws configure が必要）"; exit 1; }

log "SHA256SUMS を作成"
(cd "$OUT" && sha256sum "${FILES[@]}" > SHA256SUMS)
cat "$OUT/SHA256SUMS"

for f in "${FILES[@]}" SHA256SUMS; do
  log "アップロード: $f ($(du -h "$OUT/$f" | cut -f1)) → $S3_DEST$f"
  aws s3 cp --only-show-errors "$OUT/$f" "$S3_DEST$f"
done

log "サイズ照合"
ng=0
for f in "${FILES[@]}" SHA256SUMS; do
  local_size=$(stat -c %s "$OUT/$f")
  remote_size=$(aws s3api head-object --bucket "$(echo "$S3_DEST" | cut -d/ -f3)" \
    --key "$(echo "$S3_DEST" | cut -d/ -f4-)$f" --query ContentLength --output text)
  if [ "$local_size" = "$remote_size" ]; then log "  OK $f $local_size bytes"; else log "  NG $f local=$local_size s3=$remote_size"; ng=1; fi
done
[ $ng = 0 ] || exit 1
log "アップロード完了: $S3_DEST"
