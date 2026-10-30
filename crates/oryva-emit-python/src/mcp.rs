//! Builds the template contexts for the Python MCP server.

use oryva_ir::{Api, Method, ResponseBody, TypeRef};
use serde::Serialize;

use crate::Options;
use crate::py::{
    Target, Uses, annotation, description, docstring, import_lines, method_args, pascal, py_str,
};
use crate::sdk::{Common, common, humanize};

#[derive(Serialize)]
pub struct McpCtx {
    #[serde(flatten)]
    pub common: Common,
    pub mcp_pkg: String,
    pub mcp_dist: String,
    pub env_output_dir: String,
    pub instructions: Option<String>,
    pub stdlib: Vec<String>,
    pub third_party: Vec<String>,
    pub has_binary: bool,
    pub tools: Vec<ToolItem>,
}

#[derive(Serialize)]
pub struct ToolItem {
    pub name: String,
    pub title: String,
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub doc: String,
    pub params: Vec<String>,
    /// The SDK call, e.g. `_sdk().text.translate(input=input)`, split over lines.
    pub call: Vec<String>,
    pub returns: String,
}

pub fn ctx(api: &Api, opts: &Options) -> McpCtx {
    let mut uses = Uses::default();
    uses.typing
        .extend(["Annotated", "Any", "Iterator", "Optional"]);
    let mut tools = Vec::new();
    for r in &api.resources {
        for m in &r.methods {
            tools.push(tool(api, &r.name, m, &mut uses));
        }
    }
    let has_binary = api
        .resources
        .iter()
        .flat_map(|r| &r.methods)
        .any(crate::py::is_binary);
    let mut stdlib = Vec::new();
    if has_binary {
        stdlib.extend(["import mimetypes", "import os", "import tempfile"].map(String::from));
    }
    stdlib.insert(0, "import json".into());
    stdlib.push("from contextlib import contextmanager".into());
    if has_binary {
        stdlib.push("from pathlib import Path".into());
    }
    stdlib.extend(import_lines(&uses));

    // Third-party block in isort order (by module name).
    let class = pascal(&api.name);
    let mut third: Vec<(String, String)> = vec![
        (
            "mcp.server.mcpserver".into(),
            "from mcp.server.mcpserver import MCPServer".into(),
        ),
        (
            "mcp.server.mcpserver.exceptions".into(),
            "from mcp.server.mcpserver.exceptions import ToolError".into(),
        ),
        (
            "mcp_types".into(),
            "from mcp_types import ToolAnnotations".into(),
        ),
        (
            "pydantic".into(),
            "from pydantic import BaseModel, Field".into(),
        ),
        (
            opts.package.clone(),
            format!("from {} import {class}, {class}Error", opts.package),
        ),
    ];
    let named: Vec<String> = std::mem::take(&mut uses.named).into_iter().collect();
    if !named.is_empty() {
        third.push((
            format!("{}.types", opts.package),
            format!("from {}.types import {}", opts.package, named.join(", ")),
        ));
    }
    third.sort();
    McpCtx {
        common: common(api, opts),
        mcp_pkg: format!("{}_mcp", opts.package),
        mcp_dist: format!("{}-mcp", opts.dist_name),
        env_output_dir: format!("{}_MCP_OUTPUT_DIR", api.name.to_uppercase()),
        instructions: api.description.clone(),
        stdlib,
        third_party: third.into_iter().map(|(_, line)| line).collect(),
        has_binary,
        tools,
    }
}

fn title(m: &Method, resource: &str) -> String {
    match m.summary.as_deref().map(|s| s.trim().trim_end_matches('.')) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => humanize(&format!("{resource} {}", m.name)),
    }
}

fn tool(api: &Api, resource: &str, m: &Method, uses: &mut Uses) -> ToolItem {
    let args = method_args(api, m);
    let params = args
        .iter()
        .map(|a| {
            let mut ann = annotation(a.ty, a.required, uses, Target::Mcp);
            let mut desc = a
                .description
                .map(|d| d.split_whitespace().collect::<Vec<_>>().join(" "));
            if matches!(a.ty, TypeRef::File) {
                let note = "Path of a file on the machine running this server.";
                desc = Some(desc.map_or(note.to_string(), |d| format!("{d} {note}")));
            }
            if let Some(d) = desc {
                ann = format!("Annotated[{ann}, Field(description={})]", py_str(&d));
            }
            if a.required {
                format!("{}: {ann}", a.ident)
            } else {
                format!("{}: {ann} = None", a.ident)
            }
        })
        .collect();

    let kwargs: Vec<String> = args.iter().map(|a| format!("{0}={0}", a.ident)).collect();
    let path = format!("_sdk().{resource}.{}", m.name);
    let call = if kwargs.len() <= 3 {
        vec![format!("{path}({})", kwargs.join(", "))]
    } else {
        let mut v = vec![format!("{path}(")];
        v.extend(kwargs.iter().map(|k| format!("    {k},")));
        v.push(")".into());
        v
    };

    let mut call = call;
    let mut text = description(m);
    let (has_result, returns) = match &m.response {
        ResponseBody::Empty => (
            false,
            "return \"Done. The API returned no content.\"".into(),
        ),
        ResponseBody::Text => (true, "return result".into()),
        ResponseBody::Json { .. } => (true, "return _to_json(result)".into()),
        ResponseBody::Binary { content_type } => {
            text.push_str(&format!(
                "\n\nThe response is binary ({content_type}). It is saved to a file; the result gives its path and size."
            ));
            (
                true,
                format!(
                    "return _save_binary(result, {}, {})",
                    py_str(&format!("{resource}_{}", m.name)),
                    py_str(content_type)
                ),
            )
        }
    };

    if has_result {
        call[0] = format!("result = {}", call[0]);
    }

    ToolItem {
        name: format!("{resource}_{}", m.name),
        title: title(m, resource),
        read_only: m.agent.read_only,
        destructive: m.agent.destructive,
        idempotent: m.agent.idempotent,
        doc: docstring(&text, 4),
        params,
        call,
        returns,
    }
}
