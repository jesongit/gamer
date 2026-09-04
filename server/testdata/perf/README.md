# perf fixtures（OPTIMIZATION_PLAN / PERF-001）

确定性生成的基准夹具，由 `tools/gen-perf-fixtures.ps1` 一键重建（删除本目录后重跑脚本即可，
条目结构稳定；跨 ffmpeg 构建二进制哈希可能变化，`manifest.json` 记录了生成时的工具链全文）。

## 资产

| 文件 | 说明 |
|---|---|
| `stream.h264` | 1080x1920 竖屏 H.264 裸流（Annex-B），90 帧、固定 GOP=30，IDR 落在第 1/31/61 帧；恒定 QP 编码（qp=22、0 B 帧、threads=1）避免码率漂移 |
| `keyframe_001/031/061.png` | 从上述三个 IDR 解出的 1080x1920 RGB 截图 |
| `templates/*.png` | 三个目标矩形的灰度裁片（与服务端模板重编码后的消费形态一致），文件名带 `#后缀` 区域元数据 |
| `tmpl-rgb/*.png` | 同一区域彩色版孪生（用于验证灰度转换一致性） |
| `templates.txt` | 模板名清单，命名风格对齐 `web/src/script-language/fixtures/templates.txt`；两种区域形态各占其一：×1000 相对坐标（`361_365_639_479`）与半区码（`dr`） |

画面内容：深色底 + 固定公式的伪随机纹理（geq，整数运算含帧号 N，逐帧微变但可复现）
+ 3 个低亮度 decoy 块 + 3 个高亮目标矩形（色彩/亮度显著唯一，供断言命中坐标）。

## 三个目标矩形（详见 manifest.json 的 rects）

| 名字 | 场景覆盖 | declared (x,y,w,h) | 裁切片（钳制后） | 区域后缀 |
|---|---|---|---|---|
| perf_btn_primary | 大按钮 ≥200x200 | 390,700,300,220 | 同左，中心 (540,810) | ×1000 坐标 |
| perf_txt_status | 小文本块 ~60x40 | 140,420,60,40 | 同左，中心 (170,440) | ×1000 坐标 |
| perf_corner_menu | 贴右下角且**越界** 80x80 → 只画出 40x40 | 1040,1880,80,80 | 1040,1880,40,40，中心 (1060,1900) | 半区码 dr |

注意：`#数字后缀` 是 ×1000 相对坐标（每段 ≤999），与绝对像素间存在 ≤1px 量化误差；
基准断言以 manifest.json 中的像素值为准。

## 后续基准（实现属后续任务，此处只约定测什么）

以 `testdata/perf` 为固定输入，分别计量：

1. **decode_latest_png 总耗时**——对应 `FrameCache` 截图路径的端到端耗时；
2. **分段耗时**——ffmpeg 进程启动 / stdin 写入 annex-b 片段 / 解码等待 / PNG 编码输出四段拆分；
3. **PNG 解码与灰度化**——用 `keyframe_*.png` 直接走 image 解码 + 灰度转换的成本；
4. **NCC 匹配**——分别在**区域**（模板名 `#后缀` 解析出的搜索区，引擎语义：
   无后缀回退全屏）与**全屏**两种搜索窗下做归一化互相关；期望命中中心即上表"中心"列，
   全屏 vs 区域的倍差即 PERF-002/003 优化的上限空间；
5. **模板读取预处理**——从磁盘读 PNG 到产出匹配器所需灰度矩阵（不含 NCC 计算）；
6. **find 整轮**——主模板 + N 个 block（block 可复用 `templates/perf_btn_primary#...` 反相裁块或
   decoy 区域裁片）按 YAML 引擎 find 语义完整一轮（一次截图 + 1+N 次 NCC）。

统计口径按 OPTIMIZATION_PLAN.md §11.1：p50 / p95 / 最大值，另记 CPU 与峰值内存；
Windows 与 Docker/Linux 至少各跑一轮。禁止把 README 中任何 `<50ms` 式定性描述当验收依据。

## 消费入口约定

Rust 基准代码统一从 CARGO_MANIFEST_DIR 相对路径取夹具：

```rust
let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/perf");
let stream = dir.join("stream.h264");
```

夹具二进制属于正式提交物，请勿被 .gitignore / LFS 规则排除。