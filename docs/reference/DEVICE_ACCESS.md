# ADB 设备接入

Gamer 统一使用 ADB serial 识别设备，不区分手机、模拟器或其他 Android 运行环境。设备名称仅用于显示，Android 应用是执行目标，配置包是数据上下文，三者不会互相推导。

## 连接

1. 在 Gamer 服务所在电脑安装 ADB，并在设置或 `server/config.toml` 的 `adb_path` 指定路径。
2. USB 连接需在 Android 开启 USB 调试并授权这台电脑；网络连接需开启对应 ADB 服务。Android 无线调试有配对端口和连接端口，先执行 `adb pair <host:配对端口>`，再使用连接端口。
3. 执行 `adb devices -l` 确认目标处于 `device` 状态。`unauthorized` 要在设备上授权；`offline` 要排查线缆、调试服务或网络。
4. 工作台设备区点击扫描，或者新增并填写完整 ADB serial / `host:port`。扫描严格按 serial 去重，不按型号或名称匹配。
5. 点击连接建立投屏会话；连接不会自动启动 Android 应用，应用区单独选择和启动。

`R58...`、`emulator-5554`、ADB mDNS serial 使用已经发现的传输；`127.0.0.1:7555`、`192.168.1.20:5555` 等明确网络 endpoint 先执行 `adb connect`。不同地址代表不同传输，不自动猜测它们是否指向同一物理设备。地址不允许留空或填写设备型号。

## 屏幕与连接独立

设备设置中的 `screen_mode` 支持镜像主屏和虚拟屏。虚拟屏另设 `vd_res`（例如 1920x1080）和 `vd_dpi`，`fps` 为采集帧率上限。设备能通过 ADB 连接，不代表系统一定支持 scrcpy 虚拟显示；不支持时会明确报错，不会静默切换显示模式而改变脚本坐标。

仅改显示名称或 Android 应用不拆投屏会话。改变地址、屏幕模式、虚拟屏尺寸/DPI 或帧率时，下次会话使用新参数；有活动运行时不会为了配置变更强拆会话。空闲低功耗逻辑仍适用。

## 网络

WebRTC 默认使用 host candidate，不内置 STUN/TURN。远端/NAT 环境使用 `rtc_external_ip`、`rtc_udp_port`、`rtc_external_port` 配置候选与 UDP 映射；这与 ADB 地址是两条独立链路。ADB 已连接但画面不出现时，应继续检查 WebRTC 可达性与 ffmpeg/scrcpy。

## 数据升级

数据库 v3→v4 仅删除设备 `kind` 列，保留 ID、名称、地址、应用和屏幕设置。过去依赖空地址或按型号猜测设备的记录需要填写真实 serial；程序不会代选其他设备。插件身份转换另见仓库 README，与 ADB 迁移分开执行。
