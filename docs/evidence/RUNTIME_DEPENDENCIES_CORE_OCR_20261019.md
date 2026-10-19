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

### 退休状态补充验证

恢复旧数据的候选发现：历史 OCR 安装会优先显示通用“宿主未注册，需要升级”错误。已将明确的退休原因置于通用缺失宿主提示之前；其它退休权限和缺失内置插件的原有门禁保持。

- `runtime-retirement-tests.log`：4 项通过，包含新增的历史 OCR Running → Enabled、拒绝执行、安装文件及私密字节不变检查；也覆盖原 AI 退休门禁。
- `runtime-retirement-architecture.log`：7 项架构检查通过。
- Debug 再构建成功，六项依赖 ready，历史 OCR 明确显示“已内置，旧插件与私密数据保留只读”；`runtime-debug-retirement-acceptance.json`。
- 最终再次核对 Debug：schema 7、任务 0、设备 1、浏览器目标 1、运行记录 160、SQLite integrity_check=ok；config.toml SHA256 与上述原值一致。`runtime-debug-final-data.json`。

### Linux amd64 候选验收

本地镜像 `gamer:runtime-dependencies-20261019`，镜像 ID `sha256:a174e9fda0c61fe56f727ec46bb1284fd8d69f92fb5220e0d15b7db3b8d57567`。运行代码提交 `5a2b0cd`，主实现提交 `630d852`，插件源码提交 `3ea875c`。这些是本地构建/提交，未发布新的 Docker Hub 标签或插件归档。

`runtime-docker-candidate-acceptance.json` 记录隔离副本（network=none，1 GiB 内存，无 swap）检查：

- prod HTTP 登录可用，Cookie 为 HttpOnly/SameSite=Strict，显式 cookie_secure=false；携带 Cookie 可访问受保护依赖接口。
- 六项运行依赖全部 ready；Tesseract、eng/chi_sim 随镜像安装，无容器启动 apt/sudo。
- Chromium `155.0.8059.79` 使用独立临时目录启动，沙箱保留，空白页 CDP ready。未访问游戏或账号。
- 旧 OCR 安装仍保留但未运行；Core MCP 在授权 Package PNG 上真实识别 TEST 123。越权 Package 拒绝、撤销令牌后 HTTP 401。另有前述 Windows 无任何 OCR 插件安装的真实识别测试。
- 数据库版本/数量与恢复前一致。候选的已知测试密码仅写入隔离副本；原 release 密码未修改。

### 更新、备份及回滚

本机 `gamer-release` 已切换到上述镜像，地址 `http://127.0.0.1:18443/#/console`，容器 healthy，仍挂载原 `gamer-release-data`。prod、HTTP Cookie 策略、原密码配置、端口、init、restart、stop timeout、1 GiB shm、seccomp 和日志限制均核对保持一致。

最终切换前冷备份目录：`D:\Gamer\backups\runtime-dependencies-20261019-173908\release-final-update`：

- `data.tar.gz`：607339 字节，99 条目，完整读取校验通过；SHA256 `b293199560adc2d792cf765d4700562425671df4bba2ca37c02e3a1a49dd8ce6`。
- `container-inspect.json`、`container.env`、`chromium-seccomp.json`、`baseline.json`、`manifest.json` 和 `rollback.ps1` 保留在受限目录。
- `rollback.ps1` 已做 PowerShell 语法检查；将快照恢复到新空卷，再运行旧镜像，保留升级后的原卷。该人工脚本没有再次在当前服务执行。
- 旧镜像 `posase/gamer:v1.0.2` / `sha256:d15f0b1ac1e06e0d3bb6a9fcf9af09924072bfa5d12417a80db69ca6a4e1bbda` 保留。

实际恢复证据包含：隔离恢复旧镜像成功；一次切换保护因直播队列原始文件摘要变化自动回滚到新卷并恢复旧服务成功。复核确认仅队列并发 `revision` 计数器变化，全部业务字段一致。最终备份基线改为服务停止后采集；部署校验对队列业务字段做独立摘要，并只允许 revision 单调递增。OCR 状态只允许预期的退休变化，其它插件记录严格相等。没有放宽凭据、安装版本或配置校验。

最终 `runtime-release-promotion.json`：54 个受保护文件字节一致，队列业务字段摘要一致，历史 OCR 安装/私密数据保留，数据库任务/设备/浏览器目标/运行记录均为 0，schema 7、integrity_check=ok；调度仍关闭。这是本机原有数据现状，不代表远程机器的四个任务；远程机器未操作。

### 资源与清理

两次 Docker 构建均为单 Cargo job，专属 cgroup memory.max=6 GiB、memory.high=4 GiB、swap.max=0，与 Windows Rust 编译错开。两次合计构建组峰值 4295737344 字节（约 4.0 GiB），max/oom/oom_kill 计数均为 0。结束后仅对该空组做缓存回收，再移除自建空 cgroup；未全局 drop_caches 或重启 WSL/Docker。`runtime-docker-cgroup-final.json`、`runtime-docker-cgroup-removed.json`。

临时验收容器/卷与本次失败切换留下的停止容器均已清理，原生产卷、独立恢复卷 `gamer-release-auto-rollback-20261019`、全部备份和旧镜像保留。当前仅一个 Gamer Debug 后端及一个 Gamer release 容器运行。`runtime-docker-cleanup.json`、`deploy/docker/local/release-runtime.json`（本地忽略文件）。

未将本地合成测试当成真实游戏、Android 实机、远程生产或其它原生平台自动安装验收。
