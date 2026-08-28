//! Runtime model and execution state for the YAML engine.
//!
//! This module owns the mutable state shared by parsing and execution.  In
//! particular, function depth, return propagation, and temporary cross-file
//! environments live in one stack so callers do not have to save and restore
//! pieces of `Ctx` independently.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde_yaml::Value;

use crate::device::DeviceManager;
use crate::scripts::ScriptStore;
use crate::webrtc::ViewerMap;

/// YAML script runner.
pub struct Runner {
    pub devices: Arc<DeviceManager>,
    /// Active viewer registry used for script visualization events.
    pub viewers: ViewerMap,
    /// Script storage used by `call` and cross-file function resolution.
    pub scripts: Arc<ScriptStore>,
}

impl Runner {
    pub fn new(devices: Arc<DeviceManager>, viewers: ViewerMap, scripts: Arc<ScriptStore>) -> Self {
        Self {
            devices,
            viewers,
            scripts,
        }
    }
}

/// Parsed custom-function definition.
#[derive(Clone, Debug)]
pub struct FuncDef {
    /// Optional template conditions. Every template must match before the body
    /// is executed.
    pub cond: Vec<String>,
    /// Function body before `$N` argument substitution.
    pub body: Vec<Value>,
}

/// Temporary script/function namespace used by a cross-file call.
pub(super) struct FunctionEnvironment {
    pub script_id: String,
    pub funcs: HashMap<String, FuncDef>,
}

struct FunctionFrame {
    parent_return_value: Option<bool>,
    previous_environment: Option<FunctionEnvironment>,
}

/// Centralized function call state.
#[derive(Default)]
struct FunctionStack {
    frames: Vec<FunctionFrame>,
    return_value: Option<bool>,
}

impl FunctionStack {
    fn depth(&self) -> usize {
        self.frames.len()
    }

    fn return_value(&self) -> Option<bool> {
        self.return_value
    }

    fn set_return_value(&mut self, value: bool) {
        self.return_value = Some(value);
    }
}

/// Mutable state for one script run.
pub struct Ctx {
    pub device_id: String,
    pub script_id: String,
    pub log: Vec<(String, String)>,
    pub stop: Arc<AtomicBool>,
    /// Shared across nested `call` scripts so `throw` terminates the whole run.
    pub exit: Arc<AtomicBool>,
    pub interval_ms: u64,
    pub threshold: f32,
    pub log_level_rank: u8,
    pub funcs: HashMap<String, FuncDef>,
    /// `^N` bindings; the innermost find/color branch is the active binding.
    pub ref_stack: Vec<Vec<String>>,
    /// Templates already reported as falling back to full-screen matching.
    pub region_warned: HashSet<String>,
    pub log_cb: Option<Arc<dyn Fn(String, String) + Send + Sync>>,
    function_stack: FunctionStack,
}

impl Ctx {
    pub(super) fn new(
        device_id: String,
        script_id: String,
        stop: Arc<AtomicBool>,
        exit: Arc<AtomicBool>,
        interval_ms: u64,
        threshold: f32,
        log_level_rank: u8,
        funcs: HashMap<String, FuncDef>,
        log_cb: Option<Arc<dyn Fn(String, String) + Send + Sync>>,
    ) -> Self {
        Self {
            device_id,
            script_id,
            log: Vec::new(),
            stop,
            exit,
            interval_ms,
            threshold,
            log_level_rank,
            funcs,
            ref_stack: Vec::new(),
            region_warned: HashSet::new(),
            log_cb,
            function_stack: FunctionStack::default(),
        }
    }

    /// Log level to rank. `success` has `info` visibility.
    pub(super) fn level_rank(level: &str) -> u8 {
        match level {
            "debug" => 0,
            "info" | "success" => 1,
            "warn" => 2,
            _ => 3,
        }
    }

    pub(super) fn parse_level(s: &str) -> Option<u8> {
        match s.trim().to_ascii_lowercase().as_str() {
            "debug" => Some(0),
            "info" => Some(1),
            "warn" | "warning" => Some(2),
            "error" => Some(3),
            _ => None,
        }
    }

    pub(super) fn log(&mut self, level: &str, msg: String) {
        if Self::level_rank(level) < self.log_level_rank {
            return;
        }
        if let Some(cb) = &self.log_cb {
            cb(level.to_string(), msg.clone());
        }
        self.log.push((level.to_string(), msg));
    }

    pub(super) fn function_depth(&self) -> usize {
        self.function_stack.depth()
    }

    pub(super) fn function_return(&self) -> Option<bool> {
        self.function_stack.return_value()
    }

    pub(super) fn set_function_return(&mut self, value: bool) {
        self.function_stack.set_return_value(value);
    }

    pub(super) fn ensure_function_depth(&self, max_depth: usize) -> anyhow::Result<()> {
        if self.function_stack.depth() >= max_depth {
            anyhow::bail!("自定义函数嵌套过深（上限 {}）：疑似无限递归", max_depth);
        }
        Ok(())
    }

    /// Enter a local or cross-file function call. A cross-file environment is
    /// swapped atomically and restored by `leave_function`, including on an
    /// execution error.
    pub(super) fn enter_function(
        &mut self,
        environment: Option<FunctionEnvironment>,
        max_depth: usize,
    ) -> anyhow::Result<()> {
        self.ensure_function_depth(max_depth)?;

        let previous_environment = environment.map(|environment| FunctionEnvironment {
            script_id: std::mem::replace(&mut self.script_id, environment.script_id),
            funcs: std::mem::replace(&mut self.funcs, environment.funcs),
        });
        let parent_return_value = self.function_stack.return_value.take();
        self.function_stack.frames.push(FunctionFrame {
            parent_return_value,
            previous_environment,
        });
        Ok(())
    }

    /// Leave the current function and restore its caller's return propagation
    /// and script/function namespace. Falling through returns `true`.
    pub(super) fn leave_function(&mut self) -> bool {
        let value = self.function_stack.return_value.take().unwrap_or(true);
        let frame = self
            .function_stack
            .frames
            .pop()
            .expect("leave_function requires a matching enter_function");
        if let Some(previous) = frame.previous_environment {
            self.script_id = previous.script_id;
            self.funcs = previous.funcs;
        }
        self.function_stack.return_value = frame.parent_return_value;
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn func(label: &str) -> FuncDef {
        FuncDef {
            cond: Vec::new(),
            body: vec![Value::String(label.into())],
        }
    }

    fn context() -> Ctx {
        let mut funcs = HashMap::new();
        funcs.insert("caller".into(), func("caller"));
        Ctx::new(
            "device".into(),
            "pkg/main.yaml".into(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            500,
            0.85,
            1,
            funcs,
            None,
        )
    }

    #[test]
    fn function_stack_restores_nested_return_and_cross_file_environment() {
        let mut ctx = context();
        ctx.enter_function(None, 32).unwrap();
        ctx.set_function_return(false);

        let mut funcs = HashMap::new();
        funcs.insert("callee".into(), func("callee"));
        ctx.enter_function(
            Some(FunctionEnvironment {
                script_id: "pkg/lib.yaml".into(),
                funcs,
            }),
            32,
        )
        .unwrap();
        assert_eq!(ctx.function_depth(), 2);
        assert_eq!(ctx.script_id, "pkg/lib.yaml");
        assert!(ctx.funcs.contains_key("callee"));
        assert!(ctx.function_return().is_none());

        assert!(ctx.leave_function());
        assert_eq!(ctx.script_id, "pkg/main.yaml");
        assert!(ctx.funcs.contains_key("caller"));
        assert_eq!(ctx.function_return(), Some(false));
        assert!(!ctx.leave_function());
        assert_eq!(ctx.function_depth(), 0);
    }

    #[test]
    fn function_stack_rejects_depth_without_mutating_environment() {
        let mut ctx = context();
        ctx.enter_function(None, 1).unwrap();
        let error = ctx
            .enter_function(
                Some(FunctionEnvironment {
                    script_id: "other/lib.yaml".into(),
                    funcs: HashMap::new(),
                }),
                1,
            )
            .unwrap_err();
        assert!(error.to_string().contains("嵌套过深"));
        assert_eq!(ctx.script_id, "pkg/main.yaml");
        assert!(ctx.funcs.contains_key("caller"));
        assert!(ctx.leave_function());
    }
}
