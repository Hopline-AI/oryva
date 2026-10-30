//! `cargo run -p oryva-emit-typescript --example generate -- <api.ir.json> <out-dir>`
//!
//! Generates the TypeScript SDK straight from an IR file, so the smoke test doesn't depend on
//! the spec parser or the CLI.

use std::path::PathBuf;

use oryva_emit_typescript::{Options, emit_sdk};
use oryva_ir::Api;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(ir), Some(out)) = (args.next(), args.next()) else {
        eprintln!("usage: generate <api.ir.json> <out-dir>");
        std::process::exit(2);
    };
    let api: Api = serde_json::from_str(&std::fs::read_to_string(&ir).expect("read IR file"))
        .expect("parse IR file");
    let files = emit_sdk(&api, &Options::for_api(&api), &PathBuf::from(&out)).expect("emit SDK");
    println!("wrote {} files to {out}", files.len());
}
