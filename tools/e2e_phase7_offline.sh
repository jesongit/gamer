#!/usr/bin/env bash
# Phase 7 无设备 E2E：ffmpeg 生成小视频 → 导入 → 动作缝创建模板 → 离线匹配 →
# 草稿生成/保存 → automations 列表 → gamer.yaml stop 后制作动作 409。
# 临时端口 + 临时数据目录，用完即停。不触达任何设备/ADB。
# 注意：heredoc 内的 Python 一律用 bytes([...])/chr() 构造控制字节，不用反斜杠转义。
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PORT=18443
# Windows Python 需要真实路径（cygpath -m = C:/... 混合风格，MSYS 工具同样接受）
WORK="$(cygpath -m "$(mktemp -d)")"
DATA="$WORK/data"
mkdir -p "$DATA"
LOG="$WORK/server.log"
BASE="http://127.0.0.1:$PORT"
JAR="$WORK/cookies.txt"
FAIL=0

step() { printf '\n=== %s ===\n' "$1"; }
ok() { printf '  [OK] %s\n' "$1"; }
bad() { printf '  [FAIL] %s\n' "$1"; FAIL=1; }
# getf <file> <python-expr on d> —— 兼容有无 data 信封的动作响应
getf() {
  python -c "
import json, sys
d = json.load(open(sys.argv[1]))
if isinstance(d, dict) and isinstance(d.get('data'), dict): d = d['data']
print(eval(sys.argv[2]))
" "$1" "$2" 2>/dev/null
}
# qenc <path> —— URL 路径段百分号编码（模板文件名含 #）
qenc() { python -c "import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1]))" "$1"; }


cat > "$WORK/config.toml" <<TOML
port = $PORT
data_dir = "$DATA"
adb_path = "adb-missing-on-purpose"
ffmpeg_path = "ffmpeg"
scrcpy_server = "./assets/scrcpy-server.jar"
interval = "300ms"
threshold = 0.85
log_level = "info"
decode_frames = true
max_size = 0
bitrate_mbps = 12
fps = 15
encoder_name = ""
probe_encoder = false
idle_power_secs = 0
TOML

step "start server (port $PORT, temp data dir)"
cd "$ROOT/server"
GB_CONFIG="$WORK/config.toml" GAMER_ADMIN_PASSWORD="e2e-pass-1" \
  ./target/debug/gamer-server.exe > "$LOG" 2>&1 &
SRV=$!
cd "$ROOT"
UP=0
for i in $(seq 1 120); do
  curl -s -o /dev/null "$BASE/health/live" && UP=1 && break
  sleep 0.5
done
if [ "$UP" = "1" ]; then ok "server up"; else bad "server not up"; tail -30 "$LOG"; exit 1; fi

step "login"
CODE=$(curl -s -c "$JAR" -o "$WORK/login.json" -w '%{http_code}' -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"e2e-pass-1"}' "$BASE/api/login")
if [ "$CODE" = "200" ]; then ok "login 200"; else bad "login $CODE"; cat "$WORK/login.json"; fi

step "ffmpeg 生成小视频并导入"
ffmpeg -y -v error -f lavfi -i "color=c=0x404040:s=320x240:d=2:rate=30" \
  -vf "drawbox=x=100:y=100:w=24:h=24:color=white:t=fill" -c:v libx264 -pix_fmt yuv420p "$WORK/clip.mp4" \
  || { bad "ffmpeg 失败"; exit 1; }
CODE=$(curl -s -b "$JAR" -o "$WORK/media.json" -w '%{http_code}' \
  -X POST --data-binary @"$WORK/clip.mp4" -H 'Content-Type: application/octet-stream' \
  "$BASE/api/media/import?name=clip.mp4")
MEDIA_ID=$(python -c "import json;print(json.load(open('$WORK/media.json'))['id'])" 2>/dev/null)
if [ "$CODE" = "201" ] && [ -n "$MEDIA_ID" ]; then ok "import 201 media_id=$MEDIA_ID"; else bad "import $CODE"; cat "$WORK/media.json"; fi

step "解析指定帧身份（pts_us=500000 → 展示序帧）"
curl -s -b "$JAR" -o "$WORK/frames.json" "$BASE/api/media/$MEDIA_ID/frames?pts_us=500000"
INDEX=$(python -c "import json;print(json.load(open('$WORK/frames.json'))['current']['index'])" 2>/dev/null)
PTS=$(python -c "import json;print(json.load(open('$WORK/frames.json'))['current']['pts_us'])" 2>/dev/null)
if [ -n "$INDEX" ] && [ -n "$PTS" ]; then ok "frame identity: index=$INDEX pts_us=$PTS"; else bad "frames 解析失败"; cat "$WORK/frames.json" 2>/dev/null; fi

step "创建 Package"
CODE=$(curl -s -b "$JAR" -o "$WORK/pkg.json" -w '%{http_code}' -H 'Content-Type: application/json' \
  -d '{"id":"e2e-pkg","name":"E2E"}' "$BASE/api/packages")
if [ "$CODE" = "201" ] || [ "$CODE" = "409" ]; then ok "package ready ($CODE)"; else bad "package $CODE"; cat "$WORK/pkg.json"; fi

step "安装并启动 gamer.yaml（官方 .gplugin，免签名 + 权限确认头）"
GP="$ROOT/web/public/plugins/gamer.yaml-3.1.1.gplugin"
CODE=$(curl -s -b "$JAR" -o "$WORK/ext.json" -w '%{http_code}' -X POST \
  -H 'Content-Type: application/octet-stream' -H 'X-Gamer-Extension-Source: official' -H 'X-Gamer-Permission-Confirm: true' \
  --data-binary @"$GP" "$BASE/api/extensions")
STATE=$(python -c "import json;print(json.load(open('$WORK/ext.json')).get('state',''))" 2>/dev/null)
echo "  install -> $CODE state=$STATE"
if [ "$STATE" != "running" ]; then
  curl -s -b "$JAR" -o /dev/null -X POST "$BASE/api/extensions/gamer.yaml/enable"
  CODE=$(curl -s -b "$JAR" -o "$WORK/start.json" -w '%{http_code}' -X POST "$BASE/api/extensions/gamer.yaml/start")
  STATE=$(python -c "import json;print(json.load(open('$WORK/start.json')).get('state',''))" 2>/dev/null)
  echo "  start -> $CODE state=$STATE"
fi
if [ "$STATE" = "running" ]; then ok "gamer.yaml running"; else bad "gamer.yaml 未 running"; fi

step "动作缝 template.create_from_frame（帧上框选白块区域）"
curl -s -b "$JAR" -o "$WORK/frame.png" "$BASE/api/media/$MEDIA_ID/frame?pts_us=$PTS"
# 前端 TemplateStudio 在浏览器裁选框后再上传；E2E 用 ffmpeg 等价裁出 48x48 选框
#（内含 24x24 白块 + 灰底，纯色块会被 NCC 判 uniform 拒绝）
ffmpeg -y -v error -i "$WORK/frame.png" -vf "crop=48:48:88:88" "$WORK/crop.png" \
  || { bad "ffmpeg 裁剪失败"; exit 1; }
python - "$WORK/frame.png" "$WORK/crop.png" "$WORK/payload.json" "$MEDIA_ID" "$INDEX" "$PTS" <<'PY'
import base64, json, sys, struct
PNG_MAGIC = bytes([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])
frame = open(sys.argv[1], 'rb').read()
crop = open(sys.argv[2], 'rb').read()
assert frame[:8] == PNG_MAGIC and crop[:8] == PNG_MAGIC, 'not a png'
w, h = struct.unpack('>II', frame[16:24])  # region 相对全帧
payload = {
  "action": "template.create_from_frame",
  "values": {
    "package_id": "e2e-pkg",
    "name": "白块",
    "png_base64": base64.b64encode(crop).decode(),
    "region": [88/w, 88/h, 136/w, 136/h],
    "frame": {"media_id": sys.argv[4], "frame_index": int(sys.argv[5]), "pts_us": int(sys.argv[6])},
    "calibration": {"version": 1, "reference_size": [w, h], "rotation": 0},
  },
}
json.dump(payload, open(sys.argv[3], 'w'), ensure_ascii=False)
print('  frame %dx%d, crop 24x24 (%d bytes)' % (w, h, len(crop)))
PY
CODE=$(curl -s -b "$JAR" -o "$WORK/tpl.json" -w '%{http_code}' -H 'Content-Type: application/json' \
  --data @"$WORK/payload.json" "$BASE/api/extensions/gamer.yaml/call")
TPL_NAME=$(getf "$WORK/tpl.json" "d['name']")
if [ "$CODE" = "200" ] && [ -n "$TPL_NAME" ]; then ok "template created: $TPL_NAME"; else bad "create_template $CODE"; cat "$WORK/tpl.json" 2>/dev/null; fi

step "templates 列表出现且为合法 8-bit 灰度 PNG"
curl -s -b "$JAR" -o "$WORK/tpls.json" \
  "$BASE/api/packages/e2e-pkg/plugins/gamer.yaml/resources?prefix=templates%2F"
COUNT=$(python -c "import json;print(len(json.load(open('$WORK/tpls.json'))['resources']))" 2>/dev/null)
if [ -n "$COUNT" ] && [ "$COUNT" -ge 1 ]; then ok "templates count=$COUNT"; else bad "templates empty"; fi
TPL_PATH=$(python -c "import json;print(json.load(open('$WORK/tpls.json'))['resources'][0]['path'])" 2>/dev/null)
TPL_ENC=$(qenc "$TPL_PATH")
curl -s -b "$JAR" -o "$WORK/tpl.png" \
  "$BASE/api/packages/e2e-pkg/plugins/gamer.yaml/resources/${TPL_ENC}"
python - "$WORK/tpl.png" <<'PY'
import sys
PNG_MAGIC = bytes([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])
png = open(sys.argv[1], 'rb').read()
assert png[:8] == PNG_MAGIC, 'not a png'
# IHDR：签名 8B + 长度 4B + "IHDR" 4B → 数据自 16 起：宽 16..20 高 20..24 位深 24 色型 25
bit_depth, color_type = png[24], png[25]
assert (bit_depth, color_type) == (8, 0), 'not 8-bit gray: depth=%d color=%d' % (bit_depth, color_type)
print('  [OK] grayscale PNG verified (depth=8 color=0, %d bytes)' % len(png))
PY
if [ $? -eq 0 ]; then true; else bad "灰度校验失败"; fi

step "离线匹配命中（vision/test media_id+pts_us，不触设备）"
SHORT=$(getf "$WORK/tpl.json" "d['short_name']")
# body 用 python 组装（UTF-8 文件，避免 shell 内联非 ASCII 的编码歧义）
python - "$WORK/tpl.json" "$WORK/visionreq.json" "$MEDIA_ID" "$PTS" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
if isinstance(d, dict) and isinstance(d.get('data'), dict): d = d['data']
body = {"pkg": "e2e-pkg", "plugin": "gamer.yaml", "name": d['short_name'],
        "threshold": 0.8, "media_id": sys.argv[3], "pts_us": int(sys.argv[4])}
json.dump(body, open(sys.argv[2], 'w'), ensure_ascii=False)
PY
# 搜索区域 = 全帧（模板文件名自带区域 == 模板大小，NCC 需要 ≥ 模板+1px 的搜索余量）
python - "$WORK/visionreq.json" <<'PY'
import json, sys
body = json.load(open(sys.argv[1]))
body["region"] = [0, 0, 320, 240]
json.dump(body, open(sys.argv[1], 'w'), ensure_ascii=False)
PY
CODE=$(curl -s -b "$JAR" -o "$WORK/vision.json" -w '%{http_code}' -H 'Content-Type: application/json' \
  --data @"$WORK/visionreq.json" \
  "$BASE/api/capabilities/vision/test")
HIT=$(python -c "import json;print(json.load(open('$WORK/vision.json')).get('hit'))" 2>/dev/null)
FRAME_ID=$(python -c "import json;d=json.load(open('$WORK/vision.json')).get('frame') or {};print(d.get('frame_index'))" 2>/dev/null)
if [ "$CODE" = "200" ] && [ "$HIT" = "True" ] && [ "$FRAME_ID" = "$INDEX" ]; then
  ok "hit=true，帧身份一致（index=$FRAME_ID）"
else
  bad "vision hit=$HIT code=$CODE"; cat "$WORK/vision.json" 2>/dev/null
fi

step "伪造录制会话事件（离线草稿输入）"
REC_DIR="$DATA/media/rec-media-e2e/recording"
mkdir -p "$REC_DIR"
python - "$REC_DIR" <<'PY'
import json, sys, pathlib
d = pathlib.Path(sys.argv[1])
meta = {
  "id": "rec-e2e", "device_id": "dev-fake", "state": "completed",
  "started_at": "2026-09-07T00:00:00Z", "ended_at": "2026-09-07T00:00:05Z",
  "segments": [{"media_id": "rec-media-e2e", "start_us": 0, "duration_us": 5000000, "base_pts_us": 0, "reason": "normal"}],
  "event_count": 3, "error": None,
}
(d / "session.json").write_text(json.dumps(meta), encoding='utf-8')
def ev(eid, kind, t, payload):
    return {"schema_version": 1, "event_id": eid, "operation_id": "op-" + eid, "session_id": "rec-e2e",
            "source": "manual", "kind": kind, "timeline_us": t, "time_domain": "recording",
            "coordinate_space": "device-display", "display_size": {"width": 320, "height": 240},
            "payload": payload, "status": "accepted"}
rows = [
  ev("ev-1", "tap", 500000, {"x": 112, "y": 112}),
  ev("ev-2", "text", 800000, {"length": 9}),
  ev("ev-3", "tap", 1500000, {"x": 20, "y": 30}),
]
(d / "events-0001.jsonl").write_text(chr(10).join(json.dumps(r, ensure_ascii=False) for r in rows), encoding='utf-8')
print('  [OK] fabricated recording rec-e2e (3 events)')
PY

step "动作缝 automation.create_draft（事件选择/注释 → 草稿文本 + source）"
cat > "$WORK/draftreq.json" <<JSON
{"action": "automation.create_draft", "values": {"recording_id": "rec-e2e",
  "event_ids": ["ev-1", "ev-3", "ev-2"],
  "comments": {"ev-1": "点白块", "ev-2": "需人工补 text"}}}
JSON
CODE=$(curl -s -b "$JAR" -o "$WORK/draft.json" -w '%{http_code}' -H 'Content-Type: application/json' \
  --data @"$WORK/draftreq.json" "$BASE/api/extensions/gamer.yaml/call")
YAML_TEXT=$(getf "$WORK/draft.json" "d['yaml']")
if [ "$CODE" = "200" ] && [ -n "$YAML_TEXT" ]; then ok "draft generated"; else bad "create_draft $CODE"; cat "$WORK/draft.json" 2>/dev/null; fi
python - "$WORK/draft.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
if isinstance(data, dict) and isinstance(data.get('data'), dict): data = data['data']
yaml = data['yaml']
assert 'version: 3' in yaml and yaml.count('- tap:') == 2, yaml
assert chr(35) + ' 点白块' in yaml, yaml
assert any(d['event_id'] == 'ev-2' for d in data['diagnostics']), 'text 必须进诊断'
src = data['source']
assert src['recording_id'] == 'rec-e2e' and len(src['events']) == 3, src
print('  [OK] draft: 2 taps + 注释 + text 诊断 + source 回查（3 事件）')
PY
if [ $? -eq 0 ]; then true; else bad "draft 断言失败"; fi

step "动作缝 automation.save_draft（v3 校验保存）"
python - "$WORK/draft.json" "$WORK/savereq.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
if isinstance(data, dict) and isinstance(data.get('data'), dict): data = data['data']
json.dump({"action": "automation.save_draft",
           "values": {"package_id": "e2e-pkg", "name": "draft-e2e", "yaml": data['yaml']}},
          open(sys.argv[2], 'w'), ensure_ascii=False)
PY
CODE=$(curl -s -b "$JAR" -o "$WORK/saved.json" -w '%{http_code}' -H 'Content-Type: application/json' \
  --data @"$WORK/savereq.json" "$BASE/api/extensions/gamer.yaml/call")
SAVED_ID=$(getf "$WORK/saved.json" "d['id']")
if [ "$CODE" = "200" ] && [ "$SAVED_ID" = "e2e-pkg/draft-e2e.yaml" ]; then ok "saved id=$SAVED_ID"; else bad "save_draft $CODE"; cat "$WORK/saved.json" 2>/dev/null; fi

step "automations 列表出现草稿脚本"
curl -s -b "$JAR" -o "$WORK/autos.json" \
  "$BASE/api/packages/e2e-pkg/plugins/gamer.yaml/resources?prefix=automations%2F"
ACOUNT=$(python -c "import json;print(len(json.load(open('$WORK/autos.json'))['resources']))" 2>/dev/null)
if [ -n "$ACOUNT" ] && [ "$ACOUNT" -ge 1 ]; then ok "automations count=$ACOUNT"; else bad "automations empty"; fi

step "重名拒绝 + gamer.yaml stop → 制作动作禁用（409），vision 离线仍可用"
CODE=$(curl -s -b "$JAR" -o "$WORK/dup.json" -w '%{http_code}' -H 'Content-Type: application/json' \
  --data @"$WORK/savereq.json" "$BASE/api/extensions/gamer.yaml/call")
if grep -q "已存在" "$WORK/dup.json" 2>/dev/null; then ok "重名拒绝（结构化提示 overwrite）"; else bad "dup expected 已存在 got $CODE"; cat "$WORK/dup.json" 2>/dev/null; fi
curl -s -b "$JAR" -o /dev/null -X POST "$BASE/api/extensions/gamer.yaml/stop"
CODE=$(curl -s -b "$JAR" -o "$WORK/gated.json" -w '%{http_code}' -H 'Content-Type: application/json' \
  --data @"$WORK/draftreq.json" "$BASE/api/extensions/gamer.yaml/call")
if [ "$CODE" = "409" ]; then ok "create_draft gated -> 409"; else bad "gate expected 409 got $CODE"; cat "$WORK/gated.json" 2>/dev/null; fi
CODE=$(curl -s -b "$JAR" -o "$WORK/vision2.json" -w '%{http_code}' -H 'Content-Type: application/json' \
  --data @"$WORK/visionreq.json" \
  "$BASE/api/capabilities/vision/test")
if [ "$CODE" = "200" ]; then ok "vision/test（Core REST）不受 yaml stop 影响"; else bad "vision after stop $CODE"; fi

step "stop server & cleanup"
kill $SRV 2>/dev/null
wait $SRV 2>/dev/null
rm -rf "$WORK"
if [ "$FAIL" = "0" ]; then echo; echo "E2E ALL GREEN"; exit 0; else echo; echo "E2E HAS FAILURES"; exit 1; fi
