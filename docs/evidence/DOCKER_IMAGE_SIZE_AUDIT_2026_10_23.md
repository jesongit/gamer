# Docker v1.0.4 镜像体积审计（2026-10-23）

只读扫描 `posase/gamer:v1.0.4`，摘要 `sha256:91ee6e7e7ce73d6e1279af82c7344770062f907dae7352ee7f318658aeae468e`。本轮没有修改 Dockerfile、程序、公开镜像、安装或数据卷，也未构建/发布精简版。

扫描覆盖全部 390 个已安装系统包、9,676 个去重普通文件、1,230 个 ELF 文件、12 个镜像层，最终文件扫描无访问错误。容器禁网、只读，无用户数据挂载；额外只读权限仅用于读取镜像内默认用户目录。层检查流式读取 `docker image save`，未落地或解压完整镜像。

## 大小口径

- Registry 压缩层合计 548319285 字节，即 **522.9 MiB**；这是完整冷拉取的层下载量，已有缓存会少于此值。
- Docker history 未压缩层合计 1347739648 字节，即 **1.255 GiB**。本机 Docker inspect 记录约 1.896 GB，接近压缩层与未压缩层之和；不能把这个显示值当成每次实际下载量。
- 当前文件系统去重普通文件逻辑长度约 1.308 GB，属于另一口径，不与镜像 tar 的目录/元数据、层内布局等直接等同。
- 最大一层为安装 Chromium、FFmpeg、OCR 等依赖的层，普通文件 payload 约 **1065.7 MiB**。Gamer 二进制层约 **73.2 MiB**，前端约 **1.8 MiB**。

## 第一轮裁剪候选

下表是当前文件/ELF section 的未压缩大小，**属于可评估候选，不是实施后已经证明可回收的空间**。不等于同等大小的下载节省；须在安装它们的同一个构建阶段/层内裁剪，后加删除层不能消除旧层内容。

| 候选 | 当前大小/候选上限 | 判断与代价 |
| --- | ---: | --- |
| Noto Serif CJK 常规、粗体两份字体 | 51.3 MiB | 可以评估保留 Noto Sans CJK 常规/粗体，完整保留中文字体覆盖；页面的衬线外观会回退，需检查浏览器目标登录页、游戏页面和文字截图。不能使用固定汉字小子集来替代完整字库。 |
| Gamer 普通 ELF 符号表 | 19.4 MiB | `.symtab` 3,461,856 字节、`.strtab` 16,859,195 字节，均未映射为运行段。可对发布副本 strip，并把原符号留作诊断；保留动态链接符号和异常展开信息，检查启动、插件、错误与回溯。其余 ELF 未发现同类可裁符号/调试 section。 |
| Chromium Vulkan 验证层 | 22.6 MiB | `/usr/lib/chromium/libVkLayer_khronos_validation.so` 是调试验证组件。Gamer 启动参数没有启用该验证层；可列为候选，但必须验证沙箱、WebGL/软件渲染和浏览器目标，不等同于删 Vulkan/GL 驱动。 |
| Python `.pyc` 缓存 | 11.7 MiB | 共 704 个，均有对应源码。可在安装层清理，并制定运行时 bytecode 写入策略；冷启动和导入可能变慢，需验证 entrypoint、备份恢复、账号密钥和浏览器辅助脚本。 |
| OCR 方向检测模型 `osd.traineddata` | 10.1 MiB | 当前 Core 只将 line/block/sparse 映射至 PSM 7/6/11，没有方向检测模式。保留 eng、chi_sim；OSD 是候选，仍需完整 OCR 及已收集副本截图回归。Debian 的 tesseract-ocr 包依赖 OSD 包，不能直接 apt 卸载，否则引擎也会被卸载。 |

上述候选去重合计 **约 115.0 MiB**；实施前不承诺全部可裁或业务完全无影响。首次验证可按“符号分离、字体、验证层、OCR 模型、Python 缓存”逐项进行，避免故障时无法定位原因。

Vulkan 验证层是可选调试组件，依据 [Khronos 官方说明](https://github.khronos.org/Vulkan-Site/tutorial/latest/03_Drawing_a_triangle/00_Setup/02_Validation_layers.html)；具体候选文件的大小和 Gamer 是否显式启用来自本次镜像及固定源码检查。

## 更大收益需要重新选择构建依赖

### FFmpeg

包管理器模拟移除 FFmpeg 会移除 91 个相关包，Installed-Size 合计约 **182.5 MiB**（每项包计划彼此可能重叠，不能与其他卸载计划相加）。这些包包含必要媒体能力，不能整体作为冗余删除。

当前是 Debian 通用多媒体构建，实际启用了 flite 语音合成、PocketSphinx 语音识别、光盘访问、额外编码器等。仅 libflite1 包约 26.9 MiB，libcodec2 约 16.1 MiB，libx265 约 16.4 MiB，SVT-AV1 与 rav1e 编码器约 10.8 MiB。Gamer 当前固定流程没有调用这些合成/识别滤镜；通用 FFmpeg 功能说明见 [FFmpeg 官方滤镜文档](https://ffmpeg.org/ffmpeg-filters.html#flite)。

更合适的方案是定制 FFmpeg 构建，再核对动态依赖闭包；不能只删被 libavcodec/libavfilter 链接的 `.so`。必须保留并验证：

- Android H.264 输入 → `select` → PPM 单帧解码（`server/src/device/frames.rs`）。
- 素材 ffprobe 探测/PTS、解码、缩放和 PNG 抽帧（`server/src/media/mod.rs`）；媒体导入目前不限于 H.264，不能因精简编码器而无意丢掉现有输入解码能力。
- Matroska 输入、H.264/libx264、AAC、HLS/TS、FLV/RTMP 输出和必要网络协议（`server/src/media/output.rs`）。

没有进行定制编译，故本轮不报“确定还能净省多少”；182.5 MiB 是目前 FFmpeg 独占安装包总量，不是净节省量。

### Chromium / Mesa / LLVM

Chromium 目录约 381 MiB；共享库目录约 531 MiB。其中 LLVM 单文件约 123.7 MiB、Gallium 约 40.6 MiB、Z3 约 26.5 MiB，合计约 **191 MiB**。

这是浏览器图形依赖包链带入的组件。apt 模拟移除 LLVM 会连带移除 Chromium 和 FFmpeg 等大量包。Chromium 当前为普通 headless 启动，未禁用 GPU、也没有把目标限制为纯静态网页；不能根据一次静态页冒烟就删除这条链。

进一步调整浏览器发行包/图形后端需要独立验收实际云游戏、WebGL、CDP、截图、输入坐标及沙箱；本轮不把它列入直接清理项。移除整个 Chromium 的包级计划约 455.0 MiB，但会失去浏览器目标能力，不符合保留完整功能的精简目标。

### GitHub CLI

`gh` 可单独移除，包级约 **33.9 MiB**，无其他包被连带卸载。它由配置包发布插件调用；下载配置、HTTPS 镜像和本地导入不依赖 gh。若改为发布功能按需安装依赖，可省这部分；若要镜像开箱即支持发布，应保留。

## 已经较精简的部分

- 头文件和静态 `.a` 库均为 **0**；未发现 Rust/Node 编译环境或构建 target 进入运行镜像。
- 文档/手册目录仅 **5.3 MiB**，本次扫描全部为版权/许可类文件，保留。
- 普通 locale 约 **0.36 MiB**；Chromium 仅一份语言包，约 **0.56 MiB**，没有可大幅清理的多语言包。
- apt/dpkg 元数据约 **7.8 MiB**，应保留包管理与审计能力；不是数百 MiB 的下载缓存。
- 全部层重复改写的旧文件 payload 约 **3.51 MiB**。其中第二个 apt 安装层约 **0.98 MiB**，合并 RUN 只有很小收益，当前大头来自组件本身。
- 核心常规中英 OCR 模型合计约 **6.3 MiB**；前端约 **1.8 MiB**，优先级低，保留。

原始完整清单、包级模拟卸载方案、ELF section 和层重复记录位于 `server/target/docker-image-audit-v1.0.4/`：`audit.json`、`layers.json`、`summary.json`。本报告没有把“未在源码直接调用”当成“可直接删除动态依赖”的证明。
