use std::fmt;
use std::time::Instant;

use serde::Serialize;
use serde_json::{Value, json};

use crate::{Error, Expect, Model, ModelRequest, Session, Task, TaskFile, ToolDef};

const MAX_RESULT_CHARS: usize = 20_000;
const DEFAULT_SYSTEM: &str = "You are completing a task using the tools provided. Call tools when they help, \
fix and retry when a call fails, and give a short final answer when you are done.";

#[derive(Debug, Clone, Serialize)]
pub struct ToolCallRecord {
    pub name: String,
    pub ok: bool,
    /// The error text (truncated), when the call failed.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskReport {
    pub id: String,
    pub passed: bool,
    /// One entry per failed expectation, or the reason the run itself failed.
    pub failures: Vec<String>,
    pub turns: u32,
    pub tool_calls: Vec<ToolCallRecord>,
    pub tool_errors: usize,
    pub recoveries: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub truncations: usize,
    pub duration_ms: u64,
    pub answer: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub tasks: usize,
    pub passed: usize,
    pub pass_rate: f64,
    /// Mean of input + output tokens (cache reads excluded) per task.
    pub mean_tokens_per_task: f64,
    pub tool_errors: usize,
    pub recoveries: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunReport {
    pub model: String,
    pub tasks: Vec<TaskReport>,
    pub summary: Summary,
}

/// Runs every task against the model, each on a fresh connection to the server.
pub async fn run(file: &TaskFile, model: &dyn Model) -> Result<RunReport, Error> {
    let mut tasks = Vec::new();
    for task in &file.tasks {
        let session = Session::connect(&file.server).await?;
        let defs = session.tools().await;
        let report = match defs {
            Ok(defs) => run_task(file, task, &session, &defs, model).await,
            Err(e) => Err(e),
        };
        session.close().await;
        tasks.push(report?);
    }
    let n = tasks.len().max(1) as f64;
    let passed = tasks.iter().filter(|t| t.passed).count();
    let summary = Summary {
        tasks: tasks.len(),
        passed,
        pass_rate: passed as f64 / n,
        mean_tokens_per_task: tasks
            .iter()
            .map(|t| t.input_tokens + t.output_tokens)
            .sum::<u64>() as f64
            / n,
        tool_errors: tasks.iter().map(|t| t.tool_errors).sum(),
        recoveries: tasks.iter().map(|t| t.recoveries).sum(),
    };
    Ok(RunReport {
        model: file.model.clone(),
        tasks,
        summary,
    })
}

async fn run_task(
    file: &TaskFile,
    task: &Task,
    session: &Session,
    defs: &[ToolDef],
    model: &dyn Model,
) -> Result<TaskReport, Error> {
    let started = Instant::now();
    let tools: Vec<Value> = defs
        .iter()
        .map(|d| json!({"name": d.name, "description": d.description.clone().unwrap_or_default(), "input_schema": d.input_schema}))
        .collect();
    let system = match &file.system {
        Some(extra) => format!("{DEFAULT_SYSTEM}\n\n{extra}"),
        None => DEFAULT_SYSTEM.to_string(),
    };
    let mut messages = vec![json!({"role": "user", "content": task.prompt})];
    let mut r = TaskReport {
        id: task.id.clone(),
        passed: false,
        failures: vec![],
        turns: 0,
        tool_calls: vec![],
        tool_errors: 0,
        recoveries: 0,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        truncations: 0,
        duration_ms: 0,
        answer: String::new(),
    };

    let completed = loop {
        if r.turns >= file.max_turns {
            r.failures
                .push(format!("exceeded max_turns ({})", file.max_turns));
            break false;
        }
        r.turns += 1;
        let req = ModelRequest {
            model: &file.model,
            system: &system,
            effort: file.effort.as_deref(),
            tools: &tools,
            messages: &messages,
        };
        let resp = match model.complete(&req).await {
            Ok(resp) => resp,
            Err(e) => {
                r.failures.push(format!("model call failed: {e}"));
                break false;
            }
        };
        r.input_tokens += resp.usage.input_tokens;
        r.output_tokens += resp.usage.output_tokens;
        r.cache_read_tokens += resp.usage.cache_read_tokens;
        // The assistant turn goes back exactly as received.
        messages.push(json!({"role": "assistant", "content": resp.content}));

        match resp.stop_reason.as_deref() {
            Some("end_turn") => {
                r.answer = text_of(&resp.content);
                break true;
            }
            Some("tool_use") => {
                let mut results = Vec::new();
                if !resp.content.iter().any(|b| b["type"] == "tool_use") {
                    r.failures
                        .push("stop_reason tool_use without a tool_use block".into());
                    break false;
                }
                for block in resp.content.iter().filter(|b| b["type"] == "tool_use") {
                    let name = block["name"].as_str().unwrap_or_default();
                    let out = match &block["input"] {
                        Value::Object(args) => session.call(name, Some(args.clone())).await,
                        Value::Null => session.call(name, None).await,
                        other => crate::ToolOutput {
                            text: format!("tool input must be a JSON object, got {other}"),
                            is_error: true,
                        },
                    };
                    let (text, truncated) = truncate(out.text);
                    r.truncations += truncated as usize;
                    r.tool_calls.push(ToolCallRecord {
                        name: name.to_string(),
                        ok: !out.is_error,
                        error: out.is_error.then(|| text.chars().take(200).collect()),
                    });
                    let text = if text.is_empty() {
                        "(no output)".to_string()
                    } else {
                        text
                    };
                    let mut result =
                        json!({"type": "tool_result", "tool_use_id": block["id"], "content": text});
                    if out.is_error {
                        result["is_error"] = Value::Bool(true);
                    }
                    results.push(result);
                }
                messages.push(json!({"role": "user", "content": results}));
            }
            Some("refusal") => {
                let category = resp
                    .stop_details
                    .as_ref()
                    .and_then(|d| d["category"].as_str())
                    .unwrap_or("unknown");
                r.failures
                    .push(format!("model refused (category: {category})"));
                break false;
            }
            Some("max_tokens") => {
                r.failures.push("stopped at max_tokens".into());
                break false;
            }
            other => {
                r.failures.push(format!(
                    "unexpected stop_reason: {}",
                    other.unwrap_or("none")
                ));
                break false;
            }
        }
    };

    r.tool_errors = r.tool_calls.iter().filter(|c| !c.ok).count();
    if completed {
        r.failures = check(&task.expect, &r);
    }
    r.passed = r.failures.is_empty();
    // A failed call is a recovery if the same tool later succeeded, or the task passed anyway.
    r.recoveries = r
        .tool_calls
        .iter()
        .enumerate()
        .filter(|(i, c)| {
            !c.ok
                && (r.passed
                    || r.tool_calls[i + 1..]
                        .iter()
                        .any(|n| n.ok && n.name == c.name))
        })
        .count();
    r.duration_ms = started.elapsed().as_millis() as u64;
    Ok(r)
}

fn check(e: &Expect, r: &TaskReport) -> Vec<String> {
    let called = |n: &str| r.tool_calls.iter().any(|c| c.name == n);
    let mut f = Vec::new();
    for t in &e.tools_called {
        if !called(t) {
            f.push(format!("tool `{t}` was never called"));
        }
    }
    for t in &e.tools_not_called {
        if called(t) {
            f.push(format!("tool `{t}` was called but should not have been"));
        }
    }
    let answer = r.answer.to_lowercase();
    for s in &e.answer_contains {
        if !answer.contains(&s.to_lowercase()) {
            f.push(format!("answer does not contain \"{s}\""));
        }
    }
    if let Some(max) = e.max_tool_errors
        && r.tool_errors > max
    {
        f.push(format!("{} tool errors, max {max}", r.tool_errors));
    }
    if let Some(max) = e.max_tool_calls
        && r.tool_calls.len() > max
    {
        f.push(format!("{} tool calls, max {max}", r.tool_calls.len()));
    }
    f
}

fn text_of(content: &[Value]) -> String {
    content
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate(text: String) -> (String, bool) {
    let total = text.chars().count();
    if total <= MAX_RESULT_CHARS {
        return (text, false);
    }
    let mut t: String = text.chars().take(MAX_RESULT_CHARS).collect();
    t.push_str(&format!(
        "\n[truncated: {} more characters omitted]",
        total - MAX_RESULT_CHARS
    ));
    (t, true)
}

impl fmt::Display for RunReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Run: model {}", self.model)?;
        writeln!(
            f,
            "{:<28} {:<5} {:>5} {:>5} {:>6} {:>5} {:>8} {:>8} {:>8} {:>6} {:>8}",
            "task",
            "result",
            "turns",
            "calls",
            "errors",
            "recov",
            "in tok",
            "out tok",
            "cache rd",
            "trunc",
            "secs"
        )?;
        for t in &self.tasks {
            writeln!(
                f,
                "{:<28} {:<6} {:>5} {:>5} {:>6} {:>5} {:>8} {:>8} {:>8} {:>6} {:>8.1}",
                t.id,
                if t.passed { "pass" } else { "FAIL" },
                t.turns,
                t.tool_calls.len(),
                t.tool_errors,
                t.recoveries,
                t.input_tokens,
                t.output_tokens,
                t.cache_read_tokens,
                t.truncations,
                t.duration_ms as f64 / 1000.0
            )?;
        }
        for t in &self.tasks {
            writeln!(f, "\n{} [{}]", t.id, if t.passed { "pass" } else { "FAIL" })?;
            for x in &t.failures {
                writeln!(f, "  failed: {x}")?;
            }
            for c in &t.tool_calls {
                match &c.error {
                    None => writeln!(f, "  call {}: ok", c.name)?,
                    Some(e) => writeln!(f, "  call {}: error: {}", c.name, e.replace('\n', " "))?,
                }
            }
        }
        let s = &self.summary;
        writeln!(
            f,
            "\nSummary: {}/{} passed ({:.0}%), mean {:.0} tokens per task, {} tool errors, {} recoveries",
            s.passed,
            s.tasks,
            s.pass_rate * 100.0,
            s.mean_tokens_per_task,
            s.tool_errors,
            s.recoveries
        )
    }
}
