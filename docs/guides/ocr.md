# 自动化文字识别（OCR）

文字识别已内置于 Core，无需安装 OCR 插件。自动化通过 `ocr` 读取动态文字，通过 `find_text` 查找文字位置。识别在服务电脑本地执行，使用原始截图；多画面预览的缩图和帧率不会改变 OCR 输入。

## 准备依赖

打开“设置 → 运行依赖”，查看 Tesseract 和识别语言包。启动时优先复用已有工具，缺失时自动准备；也可以选择本机程序和 tessdata 目录。Windows 无需自行安装 7-Zip，Docker 镜像已包含引擎和简体中文/英文模型。

旧 OCR 私密配置、安装版本和历史数据保留。路径失效时修正路径或留空保存，恢复自动检测；识别请求中途不会下载模型。详见[运行依赖](runtime-dependencies.md)。

## 在 YAML 中使用

以下片段插入现有 `run`，沿用唯一的无版本字段语法、目标声明和现有完成条件。OCR 读取到文字不代表自动化目标已经完成。

```yaml
- ocr:
    region: [0.25, 0.30, 0.50, 0.20]
    language: chi_sim+eng
    mode: block
  as: queue_info
- log: $queue_info.text

- find_text:
    text: 确定
    region: [0.20, 0.50, 0.60, 0.40]
    match: exact
    min_confidence: 0.7
    mode: sparse
  as: confirm_text
- if: $confirm_text
  then:
    - tap: $confirm_text
```

`region` 是 `[x, y, w, h]` 相对坐标，均在 `0..1`：例如第一处从画面左侧 25%、顶部 30% 开始，覆盖宽度 50%、高度 20%。区域须有正宽高且完全位于原图内；省略时识别全图。优先选实际需要读字的小区域。

| 参数 | 默认值 | 用法 |
| --- | --- | --- |
| `language` | `chi_sim+eng` | 使用已安装语言；仅英文或数字可选 `eng` |
| `mode` | `block` | `line` 单行，`block` 一块多行文字，`sparse` 分散文字 |
| `scale` | `2` | 识别前放大 `1..3` 倍；返回位置仍对应原图 |
| `whitelist` | 无 | 限制可识别字符；数字可用 `"0123456789:"` |
| `timeout` | `10s` | 单次识别上限 `1ms..15s`，包含 OCR 排队和处理 |
| `match` | `contains` | 仅 `find_text`：包含匹配，或 `exact` 完全匹配 |
| `min_confidence` | `0.5` | 仅 `find_text`：最低置信度，范围 `0..1` |

`find_text` 必须提供 `text`，按返回的行、词顺序取首个满足项。它只识别一次、不自动点击；等待文字出现可使用现有有界 `repeat` 与 `sleep`。

## 返回值与点击

动态整数可用 `number` 从 OCR 的 `text` 中提取，默认 `index:0`（`60/300` 得到 `60`，`index:1` 得到 `300`）。缺少数字或数字溢出明确失败，不返回零。读取完整 `当前值/上限` 时加 `format: fraction`，严格要求两侧整数与非零分母；`/300`、`8/` 等缺项明确失败，不会把上限当成当前值。`calculate` 接受 `op/a/b`，支持 `add/sub/mul/div/floor_div/min/max`，不接受代码表达式。比如 `calculate: {op: floor_div, a: $stamina, b: $cost}` 得到可刷次数，再通过 `min` 限制本批上限；零除和非有限结果会失败。先检查 OCR 置信度并约束合理数值范围，按钮和页面继续用模板识别。

`ocr` 返回 `text`、`confidence`、`lines`、`words`。每行和词包含 `text / confidence / x / y / width / height / center`：边界是全图像素，`center` 是全图相对坐标。顶层 `width / height` 为原始完整截图尺寸，`region` 回显识别区域的像素范围。

没有文字时，`ocr` 返回空文字、空列表和置信度 `0`；`find_text` 未命中返回 `null`。未启用插件、缺少引擎或语言包、权限不足、超时等属于错误，不伪装成未命中。

`find_text` 命中可以直接传给 `tap`，也可传 `$confirm_text.center`。结果携带 `_frame`（`FrameStamp`）和 `_observed_at_ms`；点击继续检查原设备、连接代次和坐标版本。重新连接、旋转或观察超过 5 秒后，需重新识别再点击；识别耗时不会延长这 5 秒。

Tesseract 每次按需启动，结束退出；多账号共用一个有界识别队列。字体、描边、尺寸和背景会影响效果，置信度不等于准确率，应先用目标页面的小区域截图检查实际结果。

离线验证只识别已有素材的原始帧，不访问实时设备；OCR 的本地等待不推进素材时钟，也不触发下载或设备输入。完整语法与完成条件见 [YAML v2 参考](../reference/YAML.md)。
