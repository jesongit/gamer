# P6-E2E-DOC 发布产物证据

> 记录时间：2026-09-09（Asia/Shanghai）  
> 工作区：`E:\\code\\gamer`  
> 源提交：`b242c0d586c19bb0fdb996e05879e3f0459ef9ff`  
> 范围：发布脚本最小路径修复、临时目录构建、产物与清单校验；未修改业务代码、测试代码或用户数据。

## 结论

- 官方插件构建脚本：`PASS`，退出码 `0`。
- 产物：`gamer.keymap@1.0.1`、`gamer.video@1.0.0`、`gamer.yaml@3.1.1`，均生成成功。
- registry：`schema_version=2`，3 个条目，独立结构检查通过；registry 和条目均无 `signature` 字段。
- `.gplugin`：3 个包的独立 `signer verify` 均退出码 `0`；WASM 包含 `manifest.toml + plugin.wasm`，builtin 视频包仅包含 `manifest.toml`，均不含 `signature.sig`。
- SHA：产物哈希与 registry 条目一致；脚本现按清单所在发行根目录计算相对路径，4/4 条目原始清单校验通过。
- 已修复发布脚本路径问题：当 `sha256sums.txt` 位于 `registry.json` 与 `plugins\\` 的共同发行根目录时，插件条目写为 `plugins/<file>`，registry 条目写为 `registry.json`；校验仍逐项使用清单中的严格 SHA-256 值。

## 1. 官方构建命令

命令（工作目录 `E:\\code\\gamer`；输出为临时目录）：

```powershell
$p6ReleaseTemp = Join-Path ([System.IO.Path]::GetTempPath()) ('gamer-p6-release-' + [guid]::NewGuid().ToString('N')); New-Item -ItemType Directory -Path $p6ReleaseTemp -Force | Out-Null; $p6Output = Join-Path $p6ReleaseTemp 'plugins'; $p6Registry = Join-Path $p6ReleaseTemp 'registry.json'; $p6Checksums = Join-Path $p6ReleaseTemp 'sha256sums.txt'; $p6Start = Get-Date; & .\\tools\\build-plugins.ps1 -OutputDir $p6Output -RegistryFile $p6Registry -ChecksumsFile $p6Checksums; $p6Exit = $LASTEXITCODE; $p6End = Get-Date; Write-Output ('P6_TEMP=' + $p6ReleaseTemp); Write-Output ('P6_START=' + $p6Start.ToString('o')); Write-Output ('P6_END=' + $p6End.ToString('o')); Write-Output ('P6_EXIT=' + $p6Exit); if (Test-Path $p6Output) { Get-ChildItem -File $p6Output | Select-Object FullName,Length,LastWriteTime | Format-Table -AutoSize | Out-String | Write-Output }; if (Test-Path $p6Registry) { Write-Output 'P6_REGISTRY='; Get-Content -Raw $p6Registry | Write-Output }; if (Test-Path $p6Checksums) { Write-Output 'P6_CHECKSUMS='; Get-Content -Raw $p6Checksums | Write-Output }; exit $p6Exit
```

结果：退出码 `0`，构建过程完成 1/6 至 6/6。实际临时输出目录：  
`C:\Users\Jeson\AppData\Local\Temp\gamer-p6-release-fixed-1c92700c80264be39ef63ac1ff7741ab`。

关键日志：

```text
gamer.keymap@1.0.1 kind=wasm
gamer.video@1.0.0 kind=builtin
gamer.yaml@3.1.1 kind=wasm
staging 自检全部通过
registry v2 已生成（3 个条目）
完整性清单已生成
OK 官方插件产物构建完成（无签名；registry schema_version=2）。
```

## 2. 实际产物与哈希

| 产物 | 实际路径（临时输出） | 大小 | SHA-256 |
| --- | --- | ---: | --- |
| gamer.keymap-1.0.1.gplugin | `plugins\\gamer.keymap-1.0.1.gplugin` | 99183 | `82c79cb6f6f091f19a8cd187ecbf4cb8e3e0fd39dcc525753ceccb8a097f5fba` |
| gamer.video-1.0.0.gplugin | `plugins\\gamer.video-1.0.0.gplugin` | 1098 | `cfc60ed3b1018096687e6932eade6444f17353a2078651cffe3431bcf4b7e6a0` |
| gamer.yaml-3.1.1.gplugin | `plugins\\gamer.yaml-3.1.1.gplugin` | 140418 | `74553641b32c9695c76edb2b6fd69446dfcde78a062f15cb1978be926d17c92a` |
| registry.json | 临时输出根目录 | — | `3b6f31dfa3b06dfafc085c7c6839c58983c3691c4cfd331758e0decb5853f17f` |

## 3. 独立 registry / ZIP / signer 校验

独立校验读取上述临时产物，不重新写入仓库。结果：

- `REGISTRY_SCHEMA_CHECK=1 schema_version=2 entries=3`。
- 三个条目的 `sha_match=True`、`entry_ok=1`。
- ZIP 条目：
  - keymap：`manifest.toml,plugin.wasm`，`signature.sig=False`。
  - video：`manifest.toml`，`plugin.wasm=False`，`signature.sig=False`。
  - yaml：`manifest.toml,plugin.wasm`，`signature.sig=False`。
- `SIGNER_VERIFY gamer.keymap-1.0.1.gplugin exit=0`。
- `SIGNER_VERIFY gamer.video-1.0.0.gplugin exit=0`。
- `SIGNER_VERIFY gamer.yaml-3.1.1.gplugin exit=0`。
- 根对象/条目无签名字段检查：`REGISTRY_NO_SIGNATURE_CHECK=1`，退出码 `0`。

manifest 独立 inspect：

```text
MANIFEST_INSPECT gamer.keymap exit=0 id=gamer.keymap version=1.0.1 kind=wasm
MANIFEST_INSPECT gamer.video exit=0 id=gamer.video version=1.0.0 kind=builtin
MANIFEST_INSPECT gamer.yaml exit=0 id=gamer.yaml version=3.1.1 kind=wasm
```

## 4. SHA-256 清单校验

修复后的原始清单中的插件路径为发行根目录下的 `plugins/` 子目录：

```text
plugins/gamer.keymap-1.0.1.gplugin
plugins/gamer.video-1.0.0.gplugin
plugins/gamer.yaml-3.1.1.gplugin
registry.json
```

从临时发行根目录直接执行原始清单校验：退出码 `0`，四项均为 `OK`。

```text
plugins/gamer.keymap-1.0.1.gplugin: OK
plugins/gamer.video-1.0.0.gplugin: OK
plugins/gamer.yaml-3.1.1.gplugin: OK
registry.json: OK
SHA256SUM_EXIT=0
```

校验器：`D:\\Scoop\\apps\\git\\current\\usr\\bin\\sha256sum.exe -c sha256sums.txt`，工作目录为上述临时发行根目录。

## 5. 明确未验证项

以下项目本次没有执行，不能由本机临时构建结果推导：

- `NOT_VERIFIED`：真实浏览器中的插件市场安装、启用、更新、卸载和 UI 流程。
- `NOT_VERIFIED`：真实 Android 设备、ADB、输入注入、录制和设备应用目标。
- `NOT_VERIFIED`：真实 WebRTC 视频/数据通道、Stage 播放与跨页面接管。
- `NOT_VERIFIED`：公开发布平台、远端 registry/CDN、远端下载安装和真实安装链路。
- `NOT_VERIFIED`：签名、发布凭据或外网授权相关流程；本项目当前链路为免签名构建。

本证据不将上述项目写成通过；发布清单路径问题已由本次最小修复解决。
