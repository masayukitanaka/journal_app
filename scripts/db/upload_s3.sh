#!/usr/bin/env bash
# 生成した分野 1 つ分の SQLite ファイル（field{FIELD}_*.sqlite）を S3 にアップロードし、サイズを照合する
# （docs/CREATE_DB.md 3.5）。アプリはここからファイルを取得する。
#
#   FIELD=22 scripts/db/upload_s3.sh
#
# SHA256SUMS は全分野で 1 つ。S3 上の既存の SHA256SUMS を取得し、今回の分野の行だけ差し替えて
# アップロードする（他の分野の行は残る）。ローカルの $OUT/SHA256SUMS も同じ内容に更新する。
#
# 環境変数:
#   FIELD    分野（既定: 17）
#   OUT      SQLite のあるディレクトリ（既定: src-tauri/resources/db）
#   S3_DEST  アップロード先プレフィックス（既定: s3://970173663066-public-us-east-1/journal_app/）
#
# 書き込み権限のある AWS 認証情報（インスタンスロール等）が必要。
set -euo pipefail
cd "$(dirname "$0")/../.."
FIELD=${FIELD:-17}
OUT=${OUT:-src-tauri/resources/db}
S3_DEST=${S3_DEST:-s3://970173663066-public-us-east-1/journal_app/}
S3_DEST=${S3_DEST%/}/
BUCKET=$(echo "$S3_DEST" | cut -d/ -f3)
PREFIX=$(echo "$S3_DEST" | cut -d/ -f4-)

log() { echo "[$(date '+%F %T')] $*"; }

mapfile -t FILES < <(cd "$OUT" && ls "field${FIELD}"_*.sqlite 2>/dev/null | sort)
[ ${#FILES[@]} -gt 0 ] || { log "見つかりません: $OUT/field${FIELD}_*.sqlite"; exit 1; }
log "対象（field $FIELD）: ${FILES[*]}"
aws sts get-caller-identity --query Arn --output text >/dev/null \
  || { log "AWS 認証情報がありません（インスタンスロールの付与か aws configure が必要）"; exit 1; }

log "SHA256SUMS を作成（S3 上の他分野の行は残し、field $FIELD の行を差し替える）"
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
aws s3 cp --only-show-errors "${S3_DEST}SHA256SUMS" "$tmp" 2>/dev/null || : > "$tmp"
{
  grep -v "  field${FIELD}_" "$tmp" || true
  (cd "$OUT" && sha256sum "${FILES[@]}")
} | sort -k2 > "$OUT/SHA256SUMS"
cat "$OUT/SHA256SUMS"

for f in "${FILES[@]}" SHA256SUMS; do
  log "アップロード: $f ($(du -h "$OUT/$f" | cut -f1)) → $S3_DEST$f"
  aws s3 cp --only-show-errors "$OUT/$f" "$S3_DEST$f"
done

log "サイズ照合"
ng=0
for f in "${FILES[@]}" SHA256SUMS; do
  local_size=$(stat -c %s "$OUT/$f")
  remote_size=$(aws s3api head-object --bucket "$BUCKET" --key "$PREFIX$f" --query ContentLength --output text)
  if [ "$local_size" = "$remote_size" ]; then log "  OK $f $local_size bytes"; else log "  NG $f local=$local_size s3=$remote_size"; ng=1; fi
done
[ $ng = 0 ] || exit 1
log "アップロード完了: $S3_DEST"
