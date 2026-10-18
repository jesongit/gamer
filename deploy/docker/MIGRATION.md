# 简单迁移：纯 docker run

本机备份、上传两个文件、目标机恢复。目标机只需 Linux Docker，无需 Compose 或宿主 Python。
已有安装迁移不执行 init，沿用原管理员密码。以下备份命令需要更新后的本仓库工具。

部署 ZIP 仅含脚本、安全配置和插件归档，不含 Docker 镜像；`gamer-data.tar.gz`
仅为生产数据。镜像直接使用 Docker Hub 的 `posase/gamer:v1.0.2`。

若已显示 `restored_tasks` 成功，但随后发生 OCI/runc 启动错误，数据已在卷中。
更新部署脚本和 `deploy/docker/chromium-seccomp.json` 后执行 `bash restore.sh start`，
只核验现有卷并重新创建容器，不重复解压数据、不重置密钥，也不开启自动调度。
旧的 created/exited 容器会改名保留。启动运行中的容器、外来镜像/数据卷、已启用
调度的卷均拒绝；先排查再继续。原数据包和数据卷不删除。

## 1. 本机 PowerShell

先查看精简范围，不用停止服务：

```powershell
cd D:\code\gamer
docker pull posase/gamer:v1.0.2
python tools/gamer_docker.py backup --volume gamer-production-data-diag-20261018 --image posase/gamer:v1.0.2 --profile production --report
```

上轮测量原文件约 12.01 GB、精简后约 3.25 GB，压缩前减少约 73%；当前实际大小以 report 为准。
生产包保留数据库/4 个任务、配置包、全部插件安装和私密数据、浏览器登录资料、凭据密钥、
工作稿/候选历史、发布事务、媒体、运行日志和 Trace。只省略明确的本机旧备份、Windows
原始浏览器副本、本机排查目录及可重建的 Chromium 缓存；源卷及本机原始备份均不删除。
未知目录默认保留。需要所有本机排查资料一起迁移时，将 `production` 换成 `full`。

等当前任务结束，停止本机生产容器后生成压缩包：

```powershell
docker stop --time 90 gamer-production-gamer-1
python tools/gamer_docker.py backup --volume gamer-production-data-diag-20261018 --image posase/gamer:v1.0.2 --profile production --output D:/Gamer/migration-v1.0.2/gamer-data.tar.gz
if ($LASTEXITCODE -ne 0) { throw '备份失败，请先恢复本机服务' }
$dataHash = (Get-FileHash D:/Gamer/migration-v1.0.2/gamer-data.tar.gz -Algorithm SHA256).Hash.ToLowerInvariant()
"$dataHash  gamer-data.tar.gz" | Set-Content D:/Gamer/migration-v1.0.2/gamer-data.sha256 -Encoding ascii
```

输出 backup 路径和 bytes 才表示成功。gzip 使用低压缩级别以减少打包 CPU 时间，实际大小以
输出为准。工具拒绝备份仍有容器使用的卷、不覆盖已有文件，失败不会留下名为成功备份的半包。
若失败，可执行 `docker start gamer-production-gamer-1` 恢复本机服务。
成功后保持本机生产容器停止，避免两台机器重复执行任务。

## 2. 上传数据包

把 `gamer-data.tar.gz` 和 `gamer-data.sha256` 通过私密文件传输上传到目标机新建的 `~/gamer/`。
压缩包包含账号资料和密钥，不要公开。原卷、完整历史材料及旧容器仍留在本机。

## 3. 目标机 Bash

```bash
cd ~/gamer
curl -fLO https://github.com/jesongit/gamer/releases/download/docker-v1.0.2/gamer-docker-v1.0.2-amd64-deploy.zip
docker pull --platform linux/amd64 posase/gamer:v1.0.2
docker run --rm --memory 512m --memory-swap 512m --user "$(id -u):$(id -g)" -v "$PWD:/work" --entrypoint python3 posase/gamer:v1.0.2 -m zipfile -e /work/gamer-docker-v1.0.2-amd64-deploy.zip /work
sha256sum -c gamer-data.sha256
bash restore.sh
```

发布的恢复脚本固定在 v1.0.2 镜像摘要，只接受空数据卷和未占用的容器名 `gamer`，不自动删卷或容器。
恢复后保留任务 enabled 状态，但关闭自动调度；旧版未压缩的 `gamer-data.tar` 也可恢复。
脚本实际执行以下启动命令（恢复脚本已经启动后，不要重复运行）：

```bash
docker run -d --name gamer --platform linux/amd64 \
  --init --restart unless-stopped --stop-timeout 90 --shm-size 1g \
  --security-opt "seccomp=$PWD/deploy/docker/chromium-seccomp.json" \
  -p 18443:8443 \
  -e GAMER_PROFILE=prod -e GAMER_COOKIE_SECURE=false \
  -e TZ=Asia/Shanghai -e RUST_LOG=info \
  -e TOKIO_WORKER_THREADS=2 -e MALLOC_ARENA_MAX=2 \
  --mount type=volume,src=gamer-production-data,dst=/data \
  --log-driver json-file --log-opt max-size=10m --log-opt max-file=5 \
  posase/gamer:v1.0.2
```

## 4. 检查并启用

本机另开 PowerShell，替换登录信息建立 SSH 隧道：

```powershell
ssh -L 18444:127.0.0.1:18443 用户名@目标机器IP
```

打开 `http://localhost:18444`，沿用原密码，确认 4 个任务、账号登录及截图正常，并检查“任务 → 查看日志 → 下载诊断包”。
Docker v1.0.2 支持 prod 下通过局域网 HTTP 登录；启动时增加
`-e GAMER_PROFILE=prod -e GAMER_COOKIE_SECURE=false`，端口映射使用
`-p 18443:8443` 即可直接访问。若经 HTTPS 入口访问，将 Cookie 开关设为 true。
旧 v1.0.1 不支持这个独立开关。SSH 隧道仍可按上面的步骤使用。
机器或出口 IP 改变后可能需要重新登录；复制成功不等于账号在新机器可直接使用。按需手动试跑。
目标机开启原有自动调度：

```bash
cd ~/gamer
bash restore.sh activate
```

激活时会短暂停止并启动 `gamer`；原任务设置为每天北京时间 04:01。常用命令：

```bash
docker logs --tail 100 gamer
docker stats --no-stream gamer
docker restart --time 90 gamer
```

迁移成功后保持旧生产容器停止。若回退，先在目标机执行 `docker stop --time 90 gamer`，
再在旧机器执行 `docker start gamer-production-gamer-1`，保留两端卷与备份。

## 生产资源设置

- 镜像已使用 Rust Release 与静态前端；容器没有 Node、Vite、Cargo 或源码热更新。
- 新启动命令设置 Tokio 工作线程为 2，并限制 glibc 分配 arena 数；减少高核数机器上的线程/
  堆开销，实际收益依任务负载测量。计算池配置与任务并发保留原设置。
- 继续使用 30 分钟浏览器空闲回收，保留站点登录资料；无浏览器消费者时不会常驻游戏页面。
- 保留 info 日志及错误证据；Docker 控制台日志最多约 50 MB。新运行启用诊断采样及分层保留，
  历史运行、素材、工作稿和原有 Trace 不直接清理，详见[诊断策略](https://github.com/jesongit/gamer/blob/docker-v1.0.2/docs/reference/DIAGNOSTICS.md)。
- 主机备份/恢复辅助容器限制为 512 MiB，文件缓存计入该上限；批处理保持串行。
- `/dev/shm` 的 1 GiB 是共享内存容量上限，并非启动就占用 1 GiB。不先设过低的内存/CPU 硬上限；
  多账号云游戏的活动峰值需要在目标机器实测后再限额。

参考：[Docker 运行参数](https://docs.docker.com/engine/containers/run/)、
[Tokio 工作线程](https://docs.rs/tokio/latest/tokio/runtime/struct.Builder.html#method.worker_threads)、
[glibc 分配参数](https://sourceware.org/glibc/manual/latest/html_node/Memory-Allocation-Tunables.html)。
