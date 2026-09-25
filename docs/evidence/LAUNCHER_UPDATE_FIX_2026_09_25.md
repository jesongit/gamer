# 0.2.0 启动器闪窗与自更新中断排查

## 用户安装现场（只读诊断，2026-09-25）

用户提供桌面 `gamer/logs`。`launcher.log` 记录 12:07:28Z 开始修复 launcher
0.2.1，12:07:30Z 下载完成；没有主程序升级提交记录。`state/current.json`
仍为 0.2.0。根入口 SHA256 为
`1295e8a5d74d878c269a36402b94ac3bd352c7fbfda495114f23d98911790525`，
与 runtime/launcher/0.2.0 一致；runtime/launcher/0.2.1 SHA256 为
`0b56c4af72aae6505d7fa16222070d9a42e4bb933b9919ae477e7338938233e5`，
与公开 0.2.1 发行资产一致。下载已完成，入口替换和主程序升级未完成。

## 原因与修正

- 自更新从待覆盖的旧入口启动 helper，Windows 的可执行映像占用阻止它覆盖自身；
  原失败仅写 stderr，而 helper 的 stderr 指向 null，所以现场日志未留下具体系统错误。
  改从已校验的不可变 launcher 组件启动 helper，staged 与目标均不被 helper 占用。
  原子替换的占用重试从约 225ms 增至最多约 4.9s；真实测试发现短暂占用可超出旧窗口。
- helper 使用安装根的日志、隐藏进程窗口；失败落日志和一次性错误提示，替换成功清除旧错误。
  超时也清理自己的 staging。重启保留 Windows 用户目录环境以及显式本机/ADB mDNS 设置。
- 自更新成功后通过环境交接用户已确认的清单，重新校验后直接继续主程序安装；不重新发现
  另一个候选。实际完整“点击→重启→主程序提交”仍需隔离安装验收，本次未运行真实服务。
- eframe 0.36.2 的 `post_rendering` 无条件首次 `set_visible(true)`，应用下一次隐藏
  不能阻止首帧闪现。固定该版本源码并作一处修正：显式隐藏的根窗口不在首帧自动显示；
  后续安装、错误、托盘打开仍由显式 Visible(true) 控制。来源、许可证和移除条件见
  `launcher/vendor/eframe/PATCH.md`。

## 本次实际验证

- `cargo test --manifest-path <worktree>/launcher/Cargo.toml --locked`：全部非 ignored
  测试通过，5 个既有真实安装/在线发行测试保持 ignored。设置 GAMER_LOCAL_ONLY=1、
  ADB_MDNS=0、CARGO_INCREMENTAL=0。
- 新增真实 Windows 进程测试：父进程退出后替换入口逐字节成功；目标被占用时旧入口保留、
  失败日志与错误标记生成；候选组件不变；两种情况下 staging 清理。未启动 Gamer 服务、
  ADB 或设备控制。
- `cargo run --manifest-path <worktree>/launcher/Cargo.toml --example hidden_startup_probe`：
  原生窗口完成 11 次 UI 绘制，Window::is_visible 一直为 false。此为程序化窗口可见性断言，
  不是肉眼观察完整启动器或设备画面的结论。
- `cargo clippy --manifest-path <worktree>/launcher/Cargo.toml --locked --all-targets -- -D warnings`：通过。
- 没有重跑此前被平台拒绝的服务/真机测试，没有更新 submodule gitlink，未改已发布资产。

## 用户安装入口恢复（20:24 +08:00）

再次确认安装目录没有运行中的 launcher，并核对新旧哈希后，通过同卷 File.Replace
将根入口替换为已下载的**官方原版 0.2.1 启动器**；旧入口保存在
`backups/launcher-entry-recovery-20260925-202407/gamer-launcher-0.2.0.exe`。
替换后再次核对新旧文件哈希成功。没有安装本分支的开发二进制，没有启动服务或修改配置/数据；
主程序 `current.json` 仍为 0.2.0，用户下次点击更新才会安装主程序。

这次入口恢复跳过损坏的 0.2.0 自更新步骤；官方 0.2.1 仍含上述代码缺陷，永久修复须随后续
新版本交付。已受影响的旧启动器无法靠自身下载新的修复版本跨过自更新缺陷，需要手动替换一次入口。

## 临时测试清理被执行平台拒绝

首轮进程测试失败遗留了临时夹具。本次通过 exec_command 尝试使用 PowerShell 原生
Remove-Item，仅清理以下自己创建的确切目录，并先验证解析路径位于 TEMP 内、无 reparse point：

- `%TEMP%/gamer-launcher-qa2-trampoline-success-1-34752-1790338612244`
- `%TEMP%/gamer-launcher-qa2-trampoline-success-0-17324-1790338680250`

工具在执行前返回 `CreateProcess` / `Rejected` / `rejected: blocked by policy`；
工具展示的命令中段为截断文本（`301 chars truncated`），未给出具体策略原因。
该清理命令未执行；没有换工具、改包装或拆分重试。后续成功测试自身的临时目录已由测试正常回收，
此次拒绝只涉及首轮遗留夹具，不影响已完成的测试结论与用户安装入口备份恢复。
