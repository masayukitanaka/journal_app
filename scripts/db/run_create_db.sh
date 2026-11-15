#!/usr/bin/env bash
# EC2 上で SQLite データベースを生成する一連の処理（stage → build → export_sqlite + 検証 → 出版年の誤りの修正）を通しで実行する
# （docs/CREATE_DB.md 3 章）。UPLOAD=1 なら最後に S3 へのアップロード（upload_s3.sh）も行う。EC2 の終了は含めない。
#
# Claude Code や SSH のセッションが切れても止まらないよう、切り離して起動すること:
#   FIELD=22 setsid nohup scripts/db/run_create_db.sh > /dev/null 2>&1 < /dev/null &
#
# 環境変数:
#   FIELD           OpenAlex の分野 ID（既定: 17 = Computer Science。22 = Engineering など）
#   WORK            作業フォルダ（既定: data/openalex/work/field{FIELD}。分野ごとに分ける）
#   OUT             SQLite の出力先（既定: src-tauri/resources/db）
#   MANIFEST        S3 スナップショットのファイル一覧（既定: data/openalex/manifest.json）
#   FETCH_MANIFEST  1 なら S3 から最新の manifest.json を取得して MANIFEST を上書きする（既定: 0。
#                   MANIFEST が無い場合は常に取得する）
#   STAGE_ARGS      stage.py への追加引数（例: "--limit-files 2" で動作確認）
#   FIX_MISYEARED   1 なら export 後に fix_misyeared_papers.py で出版年の誤りを修正する（既定: 1。OpenAlex API を使う）
#   UPLOAD          1 なら検証 OK の後に scripts/db/upload_s3.sh で S3 へアップロードする（既定: 0）
#   S3_DEST         アップロード先（既定: s3://970173663066-public-us-east-1/journal_app/）
#
# ログ:   $WORK/create_db.log（各ステップの出力）
#         $WORK/create_db_mem.log（30 秒ごとのメモリ・ディスク使用量）
# 状態:   $WORK/create_db.status（RUNNING <step> / DONE / FAILED <step>）
set -u
cd "$(dirname "$0")/../.."
APP_DIR=$PWD
SCRIPTS=$APP_DIR/scripts/db
FIELD=${FIELD:-17}
WORK=${WORK:-$APP_DIR/data/openalex/work/field$FIELD}
OUT=${OUT:-$APP_DIR/src-tauri/resources/db}
MANIFEST=${MANIFEST:-$APP_DIR/data/openalex/manifest.json}
FETCH_MANIFEST=${FETCH_MANIFEST:-0}
STAGE_ARGS=${STAGE_ARGS:-}
FIX_MISYEARED=${FIX_MISYEARED:-1}
UPLOAD=${UPLOAD:-0}
mkdir -p "$WORK"
WORK=$(cd "$WORK" && pwd)
LOG=$WORK/create_db.log
STATUS=$WORK/create_db.status

log() { echo "[$(date '+%F %T')] $*" >> "$LOG"; }

(while true; do
   echo "$(date '+%F %T') mem_used=$(free -m | awk '/Mem/{print $3}')MB disk_work=$(du -sm "$WORK" 2>/dev/null | cut -f1)MB disk_out=$(du -sm "$OUT" 2>/dev/null | cut -f1)MB" >> "$WORK/create_db_mem.log"
   sleep 30
 done) &
MON=$!
trap 'kill $MON 2>/dev/null' EXIT

run() {
  local name=$1; shift
  echo "RUNNING $name" > "$STATUS"
  log "=== $name 開始: $*"
  local t0=$SECONDS
  "$@" >> "$LOG" 2>&1
  local rc=$?
  log "=== $name 終了: exit=$rc $(( (SECONDS - t0) / 60 )) 分"
  return $rc
}

fail() { echo "FAILED $1" > "$STATUS"; log "=== 失敗: $1"; exit 1; }

log "=== 開始 FIELD=$FIELD WORK=$WORK OUT=$OUT MANIFEST=$MANIFEST FETCH_MANIFEST=$FETCH_MANIFEST STAGE_ARGS=$STAGE_ARGS"

# OpenAlex は S3 スナップショットを週次で更新する。既定ではリポジトリに含めた manifest（再現性優先）を使う
if [ "$FETCH_MANIFEST" = 1 ] || [ ! -f "$MANIFEST" ]; then
  mkdir -p "$(dirname "$MANIFEST")"
  run "manifest取得" aws s3 cp --no-sign-request s3://openalex/data/parquet/works/manifest.json "$MANIFEST" \
    || fail "manifest取得"
fi
log "manifest: $(python3 -c "import json,sys; m=json.load(open(sys.argv[1])); print(m['date'], len(m['files']), 'files')" "$MANIFEST")"

# DuckDB 拡張（httpfs・sqlite）は pip の duckdb に同梱されていないため、初回はネットワークから取得する
run "duckdb拡張" python3 -c "import duckdb; c = duckdb.connect(); c.execute('INSTALL httpfs'); c.execute('INSTALL sqlite')" \
  || fail "duckdb拡張"

# stage は失敗ファイルがあると exit 1（再実行で失敗分だけ処理）なので数回まで再試行する
ok=0
for i in 1 2 3; do
  # shellcheck disable=SC2086
  if run "stage (試行 $i)" python3 "$SCRIPTS/stage.py" --manifest "$MANIFEST" --work "$WORK" --field "$FIELD" $STAGE_ARGS; then ok=1; break; fi
  sleep 60
done
[ $ok = 1 ] || fail stage

run build python3 "$SCRIPTS/build.py" --work "$WORK" || fail build

# export_sqlite.py は stats.json の checks・件数・参照整合性の検証まで行い、NG なら exit 1
run export_sqlite python3 "$SCRIPTS/export_sqlite.py" --work "$WORK" --out "$OUT" || fail export_sqlite

# 出版年の誤りの修正（docs/CREATE_DB.md 4 章）。移動後の整合性検証まで行い、NG なら exit 1
if [ "$FIX_MISYEARED" = 1 ]; then
  run fix_misyeared python3 "$SCRIPTS/fix_misyeared_papers.py" --work "$WORK" --db-dir "$OUT" || fail fix_misyeared
fi

if [ "$UPLOAD" = 1 ]; then
  run upload_s3 env FIELD="$FIELD" OUT="$OUT" "$SCRIPTS/upload_s3.sh" || fail upload_s3
  echo "DONE" > "$STATUS"
  log "=== 全ステップ完了（S3 アップロード済み。EC2 の終了は未実施）"
else
  echo "DONE" > "$STATUS"
  log "=== 全ステップ完了（S3 へのアップロード・EC2 の終了は未実施）"
fi
