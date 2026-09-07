//! Checked-in WIT contract and Wasmtime-generated bindings.

// 通用插件 world 全 async 绑定（imports + exports）：宿主函数以 async
// lower 进 linker（guest 调用它们必须运行在 fiber 上），因此 store 在链接
// async import 的实例时被一次性置 `async_required`——此后该 store 的**一切**
// 入口（instantiate 之外的 run/call）都必须走 `*_async` 变体，任何同步入口
// 一律 trap `store configuration requires that *_async functions are used
// instead`（2026-09 缺陷修复：exports 未声明 async 时 call_run/call_call 是
// 同步包装 → 带 import 的插件 start 即 trap + fiber 收尾 abort）。
// exports 声明 async 后 call_run/call_call 生成为 async 包装（TypedFunc::
// call_async，fiber/concurrent 路径由 wasmtime 运行时择定）。guest 侧不受
// 影响：async 只是宿主实现/调用侧约定，组件类型与 guest 契约不变。
//
// 注意 wasmtime 48 起 `Config::async_support` 已废弃为 no-op：async 能力由
// cargo feature `async`（已启用）保证，引擎无需额外配置。
#[cfg(feature = "wasm-runtime")]
wasmtime::component::bindgen!({
    path: "wit/gamer",
    world: "extension-host",
    imports: { default: async },
    exports: { default: async },
});

/// Keymap has a deliberately separate world. The generic extension world
/// remains the Phase 6 lifecycle contract; the keymap world adds the typed
/// event/result entrypoint without changing other extensions.
#[cfg(feature = "wasm-runtime")]
pub(crate) mod keymap {
    wasmtime::component::bindgen!({
        path: "wit/keymap",
        world: "keymap-host",
    });
}

/// YAML v3 uses a request/response export so the app package can provide a
/// lowered program for each run. It shares the generic capability.invoke host
/// import; no YAML type enters the Core capability layer.
#[cfg(feature = "wasm-runtime")]
pub(crate) mod yaml {
    wasmtime::component::bindgen!({
        path: "wit/gamer",
        world: "yaml-extension-host",
    });
}

pub(crate) const WIT_PACKAGE: &str = include_str!("../../wit/gamer/host.wit");
pub(crate) const WIT_PACKAGE_VERSION: &str = "1.0.0";
