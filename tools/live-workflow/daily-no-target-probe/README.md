# 日常入口与未设置培养目标探针

领奖成功边界可使用同一当前解释器运行 `--reward-boundary --candidate <日常.yaml> --parent <直播.yaml> --report <报告.json>`。该模式覆盖最后金勾检查、返回值传播、直播提前返回和普通收尾，原入口／未培养目标模式保持不变。已发布的 2026-10-16 候选及报告位于 `server/data/live-workflow-20261016/reward-boundary/`。

只运行当前工作区的唯一 YAML 源解析器和 `plugins/gamer-yaml/interpreter`，原生函数使用确定性 mock。没有 Gamer 服务、网络监听、真实设备输入或游戏账号访问。

```powershell
$env:CARGO_TARGET_DIR = 'D:/code/gamer/server/target/live-daily-no-target-20261015/probe-target'
cargo run --locked --offline --manifest-path tools/live-workflow/daily-no-target-probe/Cargo.toml -- --self-check
cargo test --locked --offline --manifest-path tools/live-workflow/daily-no-target-probe/Cargo.toml
cargo run --locked --offline --manifest-path tools/live-workflow/daily-no-target-probe/Cargo.toml -- --candidate 'D:/code/gamer/server/data/live-workflow-20261016/no-target-enter-roi/云游戏日常.candidate.yaml' --report 'D:/code/gamer/server/data/live-workflow-20261016/no-target-enter-roi/interpreter-report.json'
node tools/live-workflow/daily-no-target-probe/negative-controls.mjs --candidate 'D:/code/gamer/server/data/live-workflow-20261016/no-target-enter-roi/云游戏日常.candidate.yaml'
```

`build.rs` 从当前宿主源码复制字节一致的解析器与函数声明快照，仅为编译提供权限元数据枚举和函数库路径分类。执行前验证记录的所有源码哈希，拒绝使用源码更新前的探针可执行文件。解释器直接依赖当前 crate，保留生产 `MAX_STEPS=100000`；mock 调用保险上限 200000 不覆盖生产步数预算。

48 个场景覆盖多次点击进入、月卡优先、迟到月卡、手机稳定等待 3 秒、二次确认/超时、云首次观察 3600 轮与续跑 150 轮上限、旧 150 轮后才出现入口/手机、已配置/未配置/已在挑战页三分支、左右滚动边界、动态目标行进入与其他行诱饵，以及共用挑战等待和原 OCR 边界。

2026-10-16 新增 11 个目标行回归：以真实失败的首次地点中心 861、稳定后地点中心 839、进入中心 816 为夹具，要求停止滚动至少 600ms 后重新匹配地点。另以不同方向的 60px 旧锚点位移验证刷新结果确实参与后续定位。进入匹配按 52×32px 完整模板包含判断，覆盖 ±40px 纵向漂移、左右按钮列位移、中心在范围内但四边裁切的拒绝，以及刷新时地点丢失的有界失败。相邻上下行相距 135px 的相同文字始终作为优先诱饵，防止扩大范围误点。夹具来自日志与素材尺寸；这里的坐标检查不是实际 NCC。

探针执行候选的原函数及本地 helper，只在下一业务阶段使用明确哨兵：恢复流程抵达 `daily_tasks` 即停止，导航抵达原首个体力 OCR 即停止。报告中的哨兵不是业务失败。不会模拟或宣称后续战斗/完整日常成功。`wait_find` 的宿主轮询、真实 NCC 匹配、实际滚动效果、设备屏障和游戏效果需要各自验证。

负对照只在 `server/data/live-workflow-20261016/no-target-enter-roi/negative-controls/` 创建候选副本（可用 `--out` 改路径）。11 项刻意破坏障碍、月卡顺序、等待时长、目标行范围、滚动上限和云加载上限，并分别恢复旧窄范围、移除地点刷新和丢弃刷新结果，要求探针识别回归；原候选哈希前后必须一致。旧候选的窄范围和未刷新锚点会被新增场景拒绝，不能继续按旧版 37 项结果视为通过。
