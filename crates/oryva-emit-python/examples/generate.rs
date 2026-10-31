//! Generates the Python SDK and MCP server from an IR file, without the spec parser.
//!
//! Usage: `cargo run -p oryva-emit-python --example generate -- <ir.json> <outdir>`
//! Writes `<outdir>/sdk` and `<outdir>/mcp`.

use std::path::Path;

use oryva_emit_python::{Options, emit_mcp, emit_sdk};
use oryva_ir::Api;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [ir, out] = args.as_slice() else {
        return Err("usage: generate <ir.json> <outdir>".into());
    };
    let api: Api = serde_json::from_str(&std::fs::read_to_string(ir)?)?;
    let opts = Options::for_api(&api);
    emit_sdk(&api, &opts, &Path::new(out).join("sdk"))?;
    emit_mcp(&api, &opts, &Path::new(out).join("mcp"))?;
    Ok(())
}
