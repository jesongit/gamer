# 生产精简迁移与 docker run 验证

用户要求目标机器使用 `docker run`，并减少上传量及生产资源消耗。
本轮仅修改主机部署/迁移工具与说明，保持已发布的 v1.0 镜像摘要不变。

## 只读现状

- `gamer-production-gamer-1` 始终 healthy，未停止、重建或更改生产配置。
- 容器空闲 `docker stats` 约 207–209 MiB、CPU 约 0.03%；服务 RSS 一次采样约 114 MiB。
  这是空闲采样，不是 4 个浏览器并行峰值或优化前后基准测试。
- `docker top` 中仅 init、Gamer、ADB；Chromium 已按 1800 秒空闲策略回收，没有 Node 开发进程。
- 配置确认自动调度开启、4 个任务全部 enabled、浏览器空闲 1800 秒。
- Dockerfile 原本就使用 `cargo build --release`、静态前端与 prod profile，未携带构建阶段的 Node/Cargo。

## 生产迁移包

新增 `deploy/docker/volume-transfer.py`，由更新后的主机 `tools/gamer_docker.py` 挂载进入已发布镜像执行。
生产卷只读挂载；`--report` 只统计，不要求停止服务。真正导出仍必须停止使用源卷的容器。

实测报告（字节统计，不含 tar 元数据，来自运行中卷，最终停机快照可略有差异）：

- 原文件：12,009,950,768 bytes。
- 包含：3,246,988,990 bytes / 3,032 files。
- 省略：8,762,961,778 bytes / 13,436 files，约 73%。
- 本机旧备份：3,427,911,783 bytes。
- 上一平台原始副本：2,061,041,853 bytes。
- 明确的本机排查目录：3,170,511,345 bytes。
- Chromium 缓存/锁：103,496,797 bytes。

默认 `full` 维持完整导出，用户可显式选择 `production` 精简传输副本；未知目录保留。
数据库、WAL、任务/配置包、Cookie/站点存储、安装密钥、HOME、所有插件和退休插件私密数据、
全部工作稿/候选/安装历史、发布事务、媒体、日志和 Trace 均包含。省略的原文件仍在本机源卷与原始备份中。
`.gz` 输出以 gzip level 1 压缩。没有导出当前生产数据，未声称最终压缩大小。
导出失败清理本轮半包，最终备份名不出现；目标已存在时拒绝覆盖。

## 启动设置

`restore.sh` 已完全移除 Compose 与 `.env` 依赖，恢复压缩包后直接 `docker run`：
命名卷、Chromium seccomp 沙箱、1 GiB shm 容量、90 秒停止窗口、自动重启、镜像健康检查、
info stdout 日志并滚动限制 10 MB × 5、时区 Asia/Shanghai。
新增 `TOKIO_WORKER_THREADS=2`、`MALLOC_ARENA_MAX=2`；不更改计算池配置、任务计划或调度并发。
新参数尚未用于真实 4 账号运行，不宣称有已测出的内存降幅；没有设置未经峰值验证的低内存硬上限。
激活前验证容器镜像和数据卷身份，短暂停机修改调度后启动。

部署包重新生成为纯运行文件包，6 个公开文件、12,858 bytes；不含 Compose、`.env` 或用户数据：
`server/target/gamer-docker-v1.0-amd64-deploy.zip`，另存 `D:/Gamer/migration-20261018/gamer-deploy.zip`。

## 实际验证

- `test_gamer_volume_transfer.py` 3 项通过：精简/完整归档逐字节保留应保留文件、只省略清单内容、
  不改变源文件；活动卷导出被拒且不创建半包；失败导出不发布备份文件。
- `test_gamer_restore.py` 2 项、`test_gamer_docker.py` 1 项通过。
- 已发布镜像内 `bash -n` 通过；部署包中的 shell 文件规范为 LF。
- 两个独立合成命名卷：初始化空安装、按正式 `docker run` 参数启动至 healthy，停机后用新工具
  生成 12,218-byte gzip 归档；新卷恢复，SQLite v7 / integrity_check=ok / 调度关闭 / 1800 秒回收，
  两项新环境变量生效。UID 1000 Chromium 使用沙箱输出测试 DOM。
- 合成恢复实例激活调度并重启后再次 healthy；仅删除这两个合成容器和合成卷。

本轮没有运行真实日常任务、操作账号或停止生产；目标机器与真实 4 账号在新参数下的验收仍需迁移后进行。
