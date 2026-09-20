# 服务端配置

源码开发默认读取 `server/config.toml`。可从仓库的 [config.example.toml](https://github.com/jesongit/gamer/blob/main/server/config.example.toml) 创建本机配置；该文件不提交版本控制。

## 常用选项

| 配置 | 用途 |
| --- | --- |
| `port` | HTTP 监听端口，默认 8443 |
| `data_dir` | 数据根目录，源码开发通常为 `./data` |
| `adb_path` | ADB 命令名或绝对路径 |
| `ffmpeg_path` | FFmpeg 命令名或绝对路径；视频功能还需要 ffprobe |
| `scrcpy_server` | 与当前宿主匹配的 scrcpy server 文件 |
| `fps` | 默认帧率上限 |
| `bitrate_mbps` | 视频编码码率，过高可能增大链路负担 |
| `idle_power_secs` | 空闲低功耗等待时长，0 关闭 |
| `log_retain_days` | 文件日志保留天数，0 不自动清理 |
| `compute_max_concurrency` | 视觉等计算任务并发上限，0 自动选择 |

带目录的相对工具路径和 `data_dir` 相对配置文件位置解析；裸命令名通过 PATH 查找。scrcpy 等应用资产受 `GAMER_APP_DIR` 控制，未指定时遵循服务端工作目录。手动运行服务时先进入 `server/`。

## 认证

管理员账号为 `admin`，没有预设密码。默认开发模式下，可在本机首次设置密码，或使用 `GAMER_ADMIN_PASSWORD`。

`[auth].password_hash` 保存 Argon2id PHC，不填写明文。生产模式不使用开发明文环境变量，必须提供有效配置；会话通过 Cookie 认证。`session_abs_secs`、`session_idle_secs` 控制会话时限，`login_max_fails`、`login_window_secs` 控制失败限流。

`GAMER_ADMIN_TOKEN` 是本机管理通道使用的令牌，与浏览器登录密码不同；由启动工具管理时无需把它填入网页。

## WebRTC 网络

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
| `GAMER_PROFILE` | 开发或生产配置策略 |
| `GAMER_APP_DIR` | 应用资产目录 |
| `GAMER_DATA_DIR` | 覆盖数据目录 |
| `GAMER_ADB_PATH`、`GAMER_FFMPEG_PATH` | 覆盖工具路径 |
| `GAMER_SCRCPY_SERVER` | 覆盖 scrcpy server 路径 |

Launcher 会按安装布局注入所需路径。配置内容错误时应修复后重启，不要通过删除数据目录来规避启动错误。
