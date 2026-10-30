//! `cargo run -p oryva-emit-python --example generate -- <ir.json> <outdir>`
//!
//! Writes the SDK to `<outdir>/sdk` and the MCP server to `<outdir>/mcp`.

use std::path::Path;

use oryva_emit_python::{Options, emit_mcp, emit_sdk};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [ir, out] = args.as_slice() else {
        return Err("usage: generate <ir.json> <outdir>".into());
    };
    let api: oryva_ir::Api = serde_json::from_str(&std::fs::read_to_string(ir)?)?;
    let opts = Options::for_api(&api);
    let out = Path::new(out);
    let sdk = emit_sdk(&api, &opts, &out.join("sdk"))?;
    let mcp = emit_mcp(&api, &opts, &out.join("mcp"))?;
    println!(
        "wrote {} SDK files and {} MCP files under {}",
        sdk.len(),
        mcp.len(),
        out.display()
    );
    Ok(())
}
