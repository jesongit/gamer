# 设备接入方案

GameBot 实例（宿主二进制 / Docker 容器）如何看到并控制 Android 设备。截至 2026-08-29，
Docker 实例（`docker-compose.local.yml`，宿主 8444）采用**手机无线调试直连**方案
（容器内独立 adb server + 密钥复用 + ICE 候选宣告，投屏出画已实测验证）；此前探索的
「共享宿主 adb server」线因 reverse 隧道死结致容器内无实时会话，完整方案与切换方式
保留在文中与 git 历史（`b2dedc7`），root cause 链见文末存档。

## 方案对比

| 方案 | 设备可见性 | scrcpy 实时会话（投屏/帧缓存截图/控制） | 手机侧操作 | 结论 |
|---|---|---|---|---|
| **共享宿主 adb server**（当前采用） | USB / 无线通吃，零手机操作（容器 adb 客户端复用宿主 server） | **容器内不可用**（reverse 隧道死结，见下节）；adb 层操作（scan/shell/设备管理/screencap 类）全部正常 | 无 | **采用**。adb 可见性即插即用；实时会话暂由宿主实例承担，待源码改造解锁 |
| 手机无线调试直连（容器内独立 server + 密钥挂载） | 需手机开「无线调试」，端口每次重开变化；熄屏深睡时 mDNS 停止广播（见 [PITFALLS](PITFALLS.md)） | 会话可建立（reverse 隧道在容器 server 与容器客户端间闭环）；**投屏出画已验证**（ICE checking→connected ≈10ms，浏览器解出 1920x1080 零丢帧，2026-08-29） | 需开启无线调试 + 配对 | **当前采用模式**（docker-compose.local.yml 现状） |
| usbipd-win USB 直通（容器内独立 server） | 容器独占 USB 设备，宿主 adb 失去设备（attach/detach 切换麻烦） | 可用（容器 server 与容器客户端闭环） | 无 | 不推荐：Windows Docker Desktop 需管理员 + 宿主失去设备，与「宿主实例同时在用手机」互斥 |

## 共享宿主 adb server：原理与操作

### 原理

- 宿主以 `adb -a -P 5037 nodaemon server` 常驻（`-a` = 监听 0.0.0.0；标准拉起的 server 只听
  127.0.0.1，容器经 `host.docker.internal` 也访问不到）。
- 容器内 GameBot 每次 spawn 的 adb 客户端进程读环境变量
  `ADB_SERVER_SOCKET=tcp:host.docker.internal:5037`（Docker Desktop 自带该 DNS），全部打到宿主
  server——USB/无线设备对容器天然全可见，容器内不需要 adbkey（配对/授权在宿主侧完成）。
- GameBot 服务端有 `Adb::probe/reset_server` 自愈（连接超时先 kill-server 重拉），但那是给
  「server 与客户端同机」的常规部署设计的——见下文「运维注意」第 2 条。

### 操作步骤

1. 宿主启动共享 server（kill-server → `-a` 模式拉起 → 轮询就绪 + netstat 验证）：

   ```powershell
   powershell -ExecutionPolicy Bypass -File tools\adb-share-start.ps1
   ```

   预期输出：`adb server listening on 0.0.0.0:5037 (pid=…)` + 设备列表。
   代价：USB 设备断连几秒，运行中的 GameBot 实例自动重连恢复，属预期。

2. compose（docker-compose.local.yml）已含：

   ```yaml
   environment:
     - ADB_SERVER_SOCKET=tcp:host.docker.internal:5037
   ```

   并已移除旧无线调试方案的 `%USERPROFILE%\.android\adbkey` 两条挂载（共享模式下无用且混淆）。

3. 验证：

   ```bash
   docker exec gamer-server-docker adb devices          # 应列出宿主的设备（含 USB serial）
   docker exec gamer-server-docker adb -s <serial> shell getprop ro.product.model
   ```

### 运维注意

1. **server 被 kill 或重启机器后需重跑脚本**：`tools/adb-share-start.ps1` 只是拉起一次，不是服务。
   以下动作都会把 server 拉回标准模式（只听 127.0.0.1）或杀掉它：
   - `gamer.ps1` 的 rebuild/restart（内部 `Reset-AdbServer`）
   - 宿主/容器 GameBot 服务端 adb 超时自愈（`Adb::reset_server`）——共享部署下**任一实例**的
     adb 卡死自愈都会顺带重启这台共享 server
   - 重启机器
2. **安全**：`-a` 使 5037 对局域网可达，任何能连到宿主 5037 端口的主机都能操作已授权设备
   （adb 协议本身无鉴权，设备授权 RSA 握手只对「新 client 密钥」生效）。家庭内网自行评估；
   不需要时用完可 `adb kill-server` 关掉（宿主 GameBot 下次用 adb 会自动拉回标准 server）。
3. Windows 防火墙：本机验证（容器经 host.docker.internal → 宿主 5037）未触发弹窗、连接正常；
   若换网络配置文件（公用网络）后容器连不上 5037，检查防火墙入站规则即可。

## 关键限制：容器内 scrcpy 实时会话不可用（reverse 隧道死结）

**现象**：容器实例（8444）对设备 `POST /api/devices/:id/connect` 报
`连接失败: accept video socket timeout`；设备侧 scrcpy server 实际已启动并创建虚拟屏
（容器日志可见 `New display: 1920x1080/420 (id=…)`），随后 socket 连接失败自行退出。

**根因**（服务端 `scrcpy.rs` 的隧道方向与 adb 协议共同决定，纯配置无解）：

1. GameBot 建 scrcpy 会话时在**容器内** `TcpListener::bind("127.0.0.1:0")` 随机端口 accept，
   然后 `adb -s <serial> reverse localabstract:scrcpy_xxx tcp:<port>` 建隧道。
2. `adb reverse` 的回连方是 **adb server**（不是发起命令的 adb 客户端）。共享模式下 adb server
   跑在**宿主**上，收到设备侧 adbd 转来的 `localabstract` 连接后，去连**宿主自己的
   127.0.0.1:<port>**（adb 源码 `network_loopback_client` 硬编码，reverse 的 local 端不支持指定
   host）。
3. 该端口是容器内随机分配的，宿主上无人监听 → scrcpy server 连不上 → 退出 → 容器内 accept 超时。

即：**adb reverse 隧道只在「adb server 与 GameBot 同一网络命名空间」时闭环**。宿主实例
（server 在 Windows）与「容器内 server + 容器内客户端」的旧模式都满足；唯独「容器内客户端 +
宿主 server」不满足。

**解锁方向**（需改 `server/src/device/scrcpy.rs`，当前未实施）：
- 短改法：bind 地址可配 + 以 `adb forward` 反转隧道方向（server 在设备侧监听 tcp 端口，
  GameBot 经 forward 主动连接）——scrcpy 官方有 `--force-adb-forward` 先例，但 video/audio/control
  三通道的 accept/connect 时序要整体反转，属协议级中等改造；
- 验收替代：容器实例当前只承担 adb 层操作（设备管理/scan/脚本管理的元数据面），实时投屏与
  帧缓存截图由宿主实例（8443）承担。

## 双实例并存注意事项

- **同一手机同一时刻只应被一个 GameBot 实例建立 scrcpy 会话**：scrcpy 会话互斥（第二次 connect
  抢 reverse 隧道/虚拟屏）、控制指令互顶（两套 viewer 触控注入互相干扰）。当前分工即规避：
  宿主实例做实时面，容器实例做 adb/元数据面。
- **定时任务不要在两实例里对同一设备重复配置**：会双跑、抢会话、互相把对方的会话拆掉。
- 两实例数据目录必须隔离（当前：`server/data` vs `docker-data/`），共享会抢 SQLite 与
  scrcpy 会话（见 docker-compose.local.yml 头注释）。

## 无线调试直连方案细节存档（搁置线，恢复时看这里）

- **密钥复用原理**：Android 11+ 无线调试的配对本质是把 client 的 RSA 密钥写入设备授权表；
  USB 授权与无线授权共用同一张表。挂载 `%USERPROFILE%\.android\adbkey`（+ `.pub`）进容器，
  容器内 adb server 持同一密钥，`adb connect <ip:port>` 即免配对码。密钥在宿主共享 server
  模式下不再需要（授权发生在宿主侧）。
- **端口动态变化**：手机「无线调试」的 connect 端口每次重新开启都变（配对端口亦然），
  连接地址需随 `adb devices -l` 的 mDNS 条目或手机屏显更新。
- **熄屏断连坑**：无线 adb 的重连由手机侧 mDNS 广播驱动，熄屏/深睡时不广播，`adb connect`
  对裸设备名无效——服务端保活只对可寻址的经典网络地址（含 `:`）补连。详见
  [PITFALLS](PITFALLS.md)「adb server 重启会掉无线调试连接」条目。
- **WebRTC 出画 root cause 链**（投屏黑屏，**已修复并实测出画**，2026-08-29）：
  1. 容器 bridge 网络的 ICE 候选是容器内网 172.x，宿主浏览器不可达 → 信令（WS）通但视频黑屏；
  2. 加 `rtc_external_ip` / `rtc_udp_port` / `rtc_external_port` 配置宣告宿主可达候选
     （提交 `018f455`；对应 docker-config.toml 三键与 compose `8444:8443/udp` 映射）；
  3. 候选 local_addr 未指定 IP 的修复（`1c6827d`）——0.0.0.0 致 muxed gather 零候选；
  4. 前端 offer 不携带候选的真根因（`b245cfb`）：`createOffer()` 的 SDP 无 a=candidate，
     需等 gathering 完成后发送 `localDescription`。
  修复后实测：checking→connected ≈10ms，SRTP 推流正常，浏览器 getStats 零丢帧、
  解出 1920x1080 真实画面（`docker-config.toml` 三键保持生效）。
