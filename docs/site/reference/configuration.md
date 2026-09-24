# 服务端配置

源码开发默认读取 `server/config.toml`。可从仓库的 [config.example.toml](https://github.com/jesongit/gamer/blob/main/server/config.example.toml) 创建本机配置；该文件不提交版本控制。

## 常用选项

| 配置 | 用途 |
| --- | --- |
| `port` | HTTP 监听端口，默认 8443 |
| `local_only` | 默认 false；true 时 HTTP 与 WebRTC 都只使用 IPv4 回环地址 |
| `data_dir` | 数据根目录，源码开发通常为 `./data` |
| `adb_path` | ADB 命令名或绝对路径 |
| `ffmpeg_path` | FFmpeg 命令名或绝对路径；视频功能还需要 ffprobe |
| `scrcpy_server` | 与当前宿主匹配的 scrcpy server 文件 |
| `fps` | 默认帧率上限 |
| `bitrate_mbps` | 视频编码码率，过高可能增大链路负担 |
| `idle_power_secs` | 空闲低功耗等待时长，0 关闭 |
| `log_retain_days` | 文件日志保留天数，0 不自动清理 |
| `compute_max_concurrency` | 视觉等计算任务并发上限，0 自动选择 |

`threshold` 仅作为视觉测试接口未指定阈值时的默认值，不覆盖自动化函数的参数默认值。自动化的匹配阈值、轮询间隔和等待超时在函数参数中设置。

未使用的 `interval`、`log_level`、`encoder_name`、`judge_delay_ms`、`update.check_url` 已从服务端配置定义和发行配置中清理；旧 `[op_templates]` 也没有运行时作用。服务日志过滤使用环境变量 `RUST_LOG`。

设置页支持空闲省电时间、日志保留天数、计算并发上限、普通登录时限和登录失败限制，保存到原配置文件并保留其他字段。页面保存省电/失败限制后在线应用，日志策略在下次清理时应用；普通登录时限仅对新会话生效，已有会话沿用登录时的期限。计算并发上限重启服务后生效，页面显示待重启状态。直接手工编辑配置文件仍需重启才能加载。

已启用的自动化插件会在设置页提供“默认模板等待超时”（初始 10 秒，范围 1～3600 秒）。设置保存在 `data_dir/extension-data/gamer-yaml/settings.json`，下次自动化或函数运行生效，不修改正在运行的任务。步骤明确填写的 `timeout` 优先；自定义函数自己声明的参数默认值不受影响。可视化编辑器返回时刷新函数目录，未填写的参数显示当前默认值，恢复默认会删除显式实参。

更新策略通过独立接口即时生效，并保存在数据目录的 `state/update-policy.json`，优先于配置文件的 `[update]` 基线。投屏参数继续在设备设置中管理，不在系统设置重复提供。

带目录的相对工具路径和 `data_dir` 相对配置文件位置解析；裸命令名通过 PATH 查找。scrcpy 等应用资产受 `GAMER_APP_DIR` 控制，未指定时遵循服务端工作目录。手动运行服务时先进入 `server/`。

## 认证

管理员账号为 `admin`，没有预设密码。默认开发模式下，可在本机首次设置密码，或使用 `GAMER_ADMIN_PASSWORD`。

`[auth].password_hash` 保存 Argon2id PHC，不填写明文。生产模式不使用开发明文环境变量，必须提供有效配置；会话通过 Cookie 认证。`session_abs_secs`、`session_idle_secs` 控制普通会话时限，`login_max_fails`、`login_window_secs` 控制失败限流。

登录页默认勾选「保持登录（30 天）」，浏览器关闭后仍保留登录，有效期从登录时算起，不受普通会话空闲时限影响。取消勾选时使用浏览器会话 Cookie，并遵守普通会话时限。两种会话均持久化到 `data_dir/auth-sessions.json`，服务重启或 rebuild 不会主动清空；退出登录或更换密码会使相应会话失效。存储只含令牌摘要和密码哈希，不保存明文密码。开发环境同一密码每次启动生成不同 Argon2 盐，不影响会话恢复。

`GAMER_ADMIN_TOKEN` 是本机管理通道使用的令牌，与浏览器登录密码不同；由启动工具管理时无需把它填入网页。

## WebRTC 网络

仅本机使用时可设置 `local_only = true`，或用 `GAMER_LOCAL_ONLY=1` 临时覆盖；本机浏览器和 USB ADB 仍可用，其他电脑无法访问。此模式与 `rtc_external_ip`、`rtc_udp_port`、`rtc_external_port` 的非默认值互斥，配置修改后重启生效。

默认同机 / 局域网直接使用 host candidate，无内置 STUN/TURN。固定外部可达地址和端口时可配置：

```toml
rtc_external_ip = "192.168.1.10"
rtc_udp_port = 3478
rtc_external_port = 50000
```

示例表示浏览器访问外部 UDP 50000，映射到服务端绑定的 UDP 3478。地址与端口应改为实际网络；外部端口与绑定端口相同时，`rtc_external_port` 可为 0。

配置固定 `rtc_udp_port` 时需同时提供有效 `rtc_external_ip`。端口变更需要重启服务，网络设备和防火墙也必须允许对应流量。

## 环境变量

| 变量 | 用途 |
| --- | --- |
| `GB_CONFIG` | 指定配置文件位置 |
| `GB_LOG` | 文件日志基准路径或日志输出方式 |
| `RUST_LOG` | 服务日志等级与模块过滤，例如 `info` |
| `GAMER_PROFILE` | 开发或生产配置策略 |
| `GAMER_APP_DIR` | 应用资产目录 |
| `GAMER_DATA_DIR` | 覆盖数据目录 |
| `GAMER_LOCAL_ONLY` | 覆盖本机模式，接受 `1`/`true` 或 `0`/`false`；启动器会透传给升级和回滚进程 |
| `GAMER_ADB_PATH`、`GAMER_FFMPEG_PATH` | 覆盖工具路径 |
| `GAMER_SCRCPY_SERVER` | 覆盖 scrcpy server 路径 |

Launcher 会按安装布局注入所需路径。配置内容错误时应修复后重启，不要通过删除数据目录来规避启动错误。


自动化设置还提供点击前延迟、点击后延迟，默认均为 300ms，范围 0～60000ms（0 关闭）。仅影响自动化的坐标点击、模板自动点击与障碍模板点击；不提供步骤级覆盖。保存后下一次运行生效，当前运行保持开始时的快照，无需重启服务。
