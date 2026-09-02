# GameBot 第三方组件声明（NOTICE）

本页是 GameBot Windows 发行包携带的第三方组件许可一览。逐组件的版本、来源
URL、逐文件 sha256/size 与许可注记以 **release/dependencies.lock.toml** 为唯一
事实源；本页与其不一致时以锁文件为准。

## 组件一览

| 组件 | 随包形态 | 版本 | 来源 | 许可 | 归属 | 许可全文 |
|---|---|---|---|---|---|---|
| Android platform-tools | adb.exe + AdbWinApi.dll + AdbWinUsbApi.dll（原字节裁包） | 37.0.1 | https://dl.google.com/android/repository/platform-tools-latest-windows.zip | Apache-2.0 | Copyright (C) The Android Open Source Project | [android-platform-tools/](./android-platform-tools/) |
| FFmpeg | ffmpeg.exe（BtbN win64-lgpl 静态构建） | N-126335-gb32f8d1c23-20260830 | https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-lgpl.zip | LGPL-3.0-or-later | FFmpeg contributors（构建: BtbN/FFmpeg-Builds） | [ffmpeg/](./ffmpeg/) |
| scrcpy-server | scrcpy-server.jar（Android 端，原字节分发） | 3.3.3 | https://github.com/Genymobile/scrcpy/releases/download/v3.3.3/scrcpy-server-v3.3.3 | Apache-2.0 | Copyright (C) Genymobile | [scrcpy/](./scrcpy/) |
| GameBot 本体 | gamer-server.exe / gamer-launcher.exe / web 前端 | 以 server/Cargo.toml 为权威（当前 0.1.1） | 本仓库 | GameBot 自有许可 | GameBot 项目 | — |

Rust 依赖组件清单（含传递依赖）由 `tools/gen-sbom.ps1` 生成 CycloneDX 1.5
格式清单（release/sbom/），不入库、随发布产物归档。

## 许可义务要点

1. **Android platform-tools（Apache-2.0）**
   - 随附 Apache License 2.0 全文（LICENSE.txt）与官方包内 NOTICE.txt 原文
     （含 Android 归属与第三方清单）。
   - Android SDK 服务条款 §3.5：列表中的开源组件仅受其开源许可管辖，本分发
     只做原字节提取裁包（仅保留 adb 三件套），未修改任何字节。

2. **FFmpeg（LGPL-3.0-or-later）**
   - 随附 LGPL-3.0 全文（COPYING.LESSER）、与分发二进制精确对应的源码 offer
     （SOURCE-OFFER.txt，commit b32f8d1c23 archive 直链）与 -buildconf 归档
     （BUILD-CONFIG.txt）。
   - 只允许分发 LGPL 构建：buildconf 不得含 --enable-gpl / --enable-nonfree
     （红线）；换构建必须重走验收门禁。
   - GameBot 以 subprocess 管道独立调用，未修改/静态链接 FFmpeg 源码，修改
     义务不适用于本应用。

3. **scrcpy-server（Apache-2.0）**
   - 随附 Apache License 2.0 全文（LICENSE.txt）与版权归属（scrcpy v3.3.3,
     Copyright (C) Genymobile）。
   - 原字节分发、禁止重打包；版本与宿主应用强绑定（与
     server/src/device/scrcpy.rs 协议常量、manifest 三方同步，不独立升级）。
