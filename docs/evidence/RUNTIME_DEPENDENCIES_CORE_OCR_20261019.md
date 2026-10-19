# 统一运行依赖与 Core OCR 验证（2026-10-19）

## 实现与保留边界

- ADB、FFmpeg/FFprobe、浏览器、Tesseract、语言模型统一在“设置 → 运行依赖”管理。有效指定路径优先，自动模式复用兼容工具；无工具时按固定摘要下载到独立版本目录。
- 下载和安装串行，流式写盘，限制大小和超时，校验 SHA256、安全解包、进程探测后原子发布；不覆盖系统软件或已有版本。下载不阻塞 HTTP 监听，不在 OCR 调用中下载。
- OCR 移到 Core，保留队列上限、进程取消/回收、超时、图片限制、坐标与来源授权。YAML 不再依赖 gamer-ocr 的启用状态。
- 新 MCP scope 为 `ocr.read` / `ocr.recognize`。旧插件权限不自动提升；管理安装/路径配置不开放 MCP。
- 旧 OCR 安装档案、配置、加密字节和数据保留。迁移复制到 `state/ocr`，源目录不改写；损坏或无法解密的配置明确报错。
- Windows x64 支持全部主要工具自动准备；Linux x64 支持独立浏览器及语言模型准备，Docker 依赖随镜像构建。其它平台缺少受验证二进制源时要求已有工具，不能声称所有原生平台均已自动安装验收。

## 已完成的本地检查

| 检查 | 结果与证据（本地忽略目录 `server/target`） |
| --- | --- |
| 完整 Rust 测试 | 1135 passed、0 failed、51 ignored；`runtime-full-verified.log`，437.75 秒，工作目录为 server |
| 缺失依赖真实下载/安装 | 1 passed；`runtime-network-final.log`。空临时目录，无系统 7-Zip，下载固定版本提取器、完整 7-Zip、Tesseract/模型并探测；再次安装拒绝覆盖 |
| 实际 OCR，无插件安装 | 1 passed；`runtime-ocr-real.log`。真实 Tesseract 识别本地合成图片，结果 TEST 123；不代表真实游戏效果 |
| 子进程取消/回收 | 1 passed；`runtime-cancel-test.log`。排队取消、运行中取消回收、下一请求继续、全局停止 |
| Clippy | `--locked -j1 --all-targets -- -D warnings` 通过；`runtime-clippy.log` |
| 无 WASM 特性编译 | `--locked -j1 --no-default-features` 通过；`runtime-no-wasm-check.log` |
| 格式与 SDK | cargo fmt --check、check-plugin-sdk.mjs 通过；固定 SDK 不变 |
| 前端完整测试 | 857 passed / 96 files；`runtime-web-full.log` |
| 设置针对性测试 | 9 passed；`runtime-web-tests2.log` |
| 前端发行构建 | 通过；`runtime-web-build.log` |
| 设置界面隔离浏览器 | 1280×900 和 390×844 检查通过，无溢出；`runtime-ui-qa.log`、`runtime-ui-desktop.png`、`runtime-ui-mobile.png` |
| Debug 原地更新 | 构建/就绪通过，六个依赖全部 ready，MCP 为 Core OCR；`runtime-debug-acceptance.json` |
| 用户实际界面确认 | 用户确认 Debug“设置 → 运行依赖”状态及“文字识别已内置”说明显示正常 |

完整测试中的 ignored 项包含需要显式开启的网络/真实程序与原有平台/手工测试；上述额外执行的两项已单列。没有操作真实游戏、设备或远程生产环境。

## Debug 数据保留

更新前冷备份位于 `D:\Gamer\backups\runtime-dependencies-20261019-173908\debug`，包含原可执行文件、config.toml 和完整 data-dev.zip。备份包含敏感资料，目录访问受限，不作为运行目录。

- ZIP：1607 文件，712869229 字节，完整性检查通过。
- SHA256：`915245f6a7d61cc0ae987d84e703b8326719c116675d9d905cc5a466fd75a308`。
- 更新前后 config.toml SHA256 一致：`542d9d8eba2e8b0ca81617afd69e3f4667d5920b95575bf3ac87c3af026e5cee`。
- 数据库 schema 7，任务 0、设备 1、浏览器目标 1、运行记录 160，更新前后一致。当前任务数以本次现场为准，未把历史备份中的旧任务恢复进来。
- 原账号/浏览器目录和历史 OCR 安装保留，调度保持关闭。

## Docker 验证与更新

待候选镜像验证通过后补充。构建单 Cargo job，使用专属 cgroup：memory.max 6 GiB、memory.high 4 GiB、swap.max 0，不与 Windows Rust 编译并行。现有 release 在此阶段保持原镜像和原卷；更新前必须冷备份并保留原镜像/容器以便回滚。
