# 日常副本 OCR 截图回归

`weekly_only: true` 仅执行候选的 `read_weekly_attempts`，用于真实保存帧上周次数 `0/3` 和 `3/3` 的模板定位、OCR 与双读校验；该模式仍拒绝任何输入或导航调用。

执行候选脚本内的 `read_dungeon_power`、`read_stamina_cost` 和 `read_challenge_counter`，逐项比较实际识别结果与人工标注。使用当前唯一 YAML 源解析器、权威解释器 crate、Core NCC、数值和 TSV 解析源码；OCR 使用 Core 相同的灰度转换与 Triangle 缩放，再调用配置的 Tesseract。截图和诊断包保留在忽略的 `server/data/` 中，不放入源码仓库。

```powershell
$env:CARGO_TARGET_DIR = 'D:/code/gamer/server/target'
cargo run --release --locked --offline --manifest-path tools/live-workflow/ocr-compat-probe/Cargo.toml -- --candidate '<候选.yaml>' --fixtures '<夹具.json>' --report '<报告.json>'
```

夹具格式示例（路径须替换为现有文件）：

```json
{
  "engine": "C:/ocr/tesseract.exe",
  "tessdata": "C:/ocr/tessdata",
  "template_roots": ["C:/review/templates", "C:/package/templates"],
  "fixtures": [{
    "name": "cavern",
    "frame": "C:/review/frame.png",
    "expected": {
      "power": {"text": "251/300", "power": 251, "maximum": 300, "layout": "cavern"},
      "cost": 40, "current": 1, "maximum": 6
    }
  }]
}
```

`power_only: true` 只运行体力读取。`inject` 可选 `empty`、`low_confidence`、`unstable`、`missing_numerator`、`missing_denominator`、`multiple_slashes`、`zero_denominator` 或 `zero`；这些是明确标记的合成保护场景，不能计作真实 OCR 识别。`inject_target` 缺省为 `power`，还支持 `cost`、`counter`；拒绝场景用 `expected_error` 指定错误码片段。真实帧场景不注入识别结果。

可选 `engine_arguments` 在 Tesseract 参数前附加启动器参数。例如用 `docker.exe exec -i <隔离容器ID> /usr/bin/tesseract` 对相同预处理 PNG 执行 Linux OCR。容器须由调用者建立、固定镜像 ID、禁网、只读、无设备或业务目录挂载，并在结束后清理；探针本身不管理容器。此时 `tessdata` 是容器内路径，须先在该固定镜像内核验英文模型 SHA256，再通过 `external_traineddata_sha256` 记录；`engine_metadata` 可记录镜像 ID 和隔离条件。报告区分启动器二进制哈希、实际引擎版本和模型哈希。

`build.rs` 生成字节一致的当前解析器快照，执行前检查依赖源码哈希。报告记录候选、帧、实际模板、引擎/模型和源码哈希，以及每次原生调用的参数和结果。模板短名支持唯一 `#` 后缀匹配，延续 Core 文件名 ROI。

宿主只允许读取匹配、OCR、纯计算、日志、虚拟 sleep 和显式失败；输入、应用、服务和通知调用全部拒绝。`wait_find`/`find_text` 对同一保存帧只观察一次，sleep 不耗真实时间；这里没有 WASM 宿主、OCR 队列、截图更新、帧代次或真实游戏验收。登录/导航流程由相邻 `daily-no-target-probe` 单独验证。
