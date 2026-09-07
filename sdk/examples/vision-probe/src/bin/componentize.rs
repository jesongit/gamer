use std::env;
use std::fs;

/// 宿主侧后处理：把 wasm32 core module 包成 WASM Component
/// （与官方 guests 的 componentize 步骤一致）。
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = env::args().nth(1).expect("core wasm input");
    let output = env::args().nth(2).expect("component wasm output");
    let module = fs::read(input)?;
    let component = wit_component::ComponentEncoder::default()
        .module(&module)?
        .validate(true)
        .encode()?;
    fs::write(output, component)?;
    Ok(())
}
