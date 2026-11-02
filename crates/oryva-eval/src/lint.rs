use std::fmt;

use serde::Serialize;

use crate::{Error, ServerConfig, Session, ToolDef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warn,
    Info,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            Severity::Error => "error",
            Severity::Warn => "warn",
            Severity::Info => "info",
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub severity: Severity,
    pub tool: String,
    pub rule: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCost {
    pub tool: String,
    pub chars: usize,
    pub approx_tokens: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct LintReport {
    pub tool_count: usize,
    pub findings: Vec<Finding>,
    pub total_chars: usize,
    /// Characters / 4.
    pub approx_tokens: usize,
    pub heaviest: Vec<ToolCost>,
}

impl LintReport {
    pub fn error_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count()
    }
}

/// Whether `phrase` appears in a sentence about the tool. A sentence that quotes code, as in "The
/// `team` filter is deprecated", is about a parameter and the tool still works.
fn says_retired(lower: &str, phrase: &str) -> bool {
    lower.match_indices(phrase).any(|(i, _)| {
        let start = lower[..i].rfind(['.', '!', '\n']).map_or(0, |s| s + 1);
        !lower[start..i].contains('`')
    })
}

const MIN_DESCRIPTION: usize = 40;
const DESTRUCTIVE_WORDS: [&str; 6] = ["delete", "remove", "cancel", "refund", "revoke", "drop"];
/// Phrases that say the tool itself is gone. A bare "deprecated" is not enough: a validator
/// can legitimately mention "deprecated values", and "no longer exists" is usually about a record
/// ("returns 404 if the sandbox no longer exists").
const RETIRED_PHRASES: [&str; 13] = [
    "is deprecated",
    "is retired",
    "has been deprecated",
    "has been retired",
    "has been removed",
    "was deprecated",
    "was retired",
    "was removed",
    "permanently removed",
    "no longer available",
    "no longer supported",
    "no longer works",
    "no longer maintained",
];
/// A description that opens with one of these is a deprecation notice.
const RETIRED_PREFIXES: [&str; 3] = ["deprecated", "[deprecated", "retired"];

/// Connects to the server, lists its tools and lints them.
pub async fn lint(cfg: &ServerConfig) -> Result<LintReport, Error> {
    let session = Session::connect(cfg).await?;
    let tools = session.tools().await;
    session.close().await;
    Ok(lint_tools(&tools?))
}

pub fn lint_tools(tools: &[ToolDef]) -> LintReport {
    let mut findings = Vec::new();
    for t in tools {
        let mut add = |severity, rule, message: String| {
            findings.push(Finding {
                severity,
                tool: t.name.clone(),
                rule,
                message,
            });
        };
        let desc = t.description.as_deref().map(str::trim).unwrap_or("");
        if desc.is_empty() {
            add(
                Severity::Error,
                "description-missing",
                "no description".into(),
            );
        } else if desc.chars().count() < MIN_DESCRIPTION {
            add(
                Severity::Warn,
                "description-short",
                format!(
                    "description is {} characters, under {MIN_DESCRIPTION}: \"{desc}\"",
                    desc.chars().count()
                ),
            );
        }

        let undocumented: Vec<&str> = t
            .input_schema
            .get("properties")
            .and_then(|p| p.as_object())
            .into_iter()
            .flatten()
            .filter(|(_, v)| {
                v.get("description")
                    .and_then(|d| d.as_str())
                    .is_none_or(|d| d.trim().is_empty())
            })
            .map(|(k, _)| k.as_str())
            .collect();
        if !undocumented.is_empty() {
            add(
                Severity::Warn,
                "param-description-missing",
                format!(
                    "input properties without a description: {}",
                    undocumented.join(", ")
                ),
            );
        }

        let ann = t.annotations.clone().unwrap_or_default();
        let mut missing = Vec::new();
        if ann.read_only_hint.is_none() {
            missing.push("readOnlyHint");
        }
        // destructiveHint only means something for tools that are not read-only.
        if ann.destructive_hint.is_none() && ann.read_only_hint != Some(true) {
            missing.push("destructiveHint");
        }
        if !missing.is_empty() {
            add(
                Severity::Warn,
                "annotation-missing",
                format!("missing annotations: {}", missing.join(", ")),
            );
        }

        if ann.destructive_hint != Some(true)
            && let Some(word) = words(&t.name)
                .into_iter()
                .find(|w| DESTRUCTIVE_WORDS.contains(&w.as_str()))
        {
            add(
                Severity::Error,
                "destructive-unmarked",
                format!("name contains \"{word}\" but destructiveHint is not true"),
            );
        }

        let lower = desc.to_lowercase();
        let retired = RETIRED_PHRASES
            .iter()
            .find(|p| says_retired(&lower, p))
            .or_else(|| RETIRED_PREFIXES.iter().find(|p| lower.starts_with(**p)));
        if let Some(p) = retired {
            add(
                Severity::Error,
                "deprecated-exposed",
                format!(
                    "description says \"{p}\"; a tool like this should not be exposed to agents"
                ),
            );
        }
    }
    findings.sort_by_key(|f| f.severity); // stable: keeps tool order within a severity

    // Cost: what the model is actually sent for each tool.
    let mut costs: Vec<ToolCost> = tools
        .iter()
        .map(|t| {
            let chars = t.name.chars().count()
                + t.description.as_deref().map_or(0, |d| d.chars().count())
                + t.input_schema.to_string().chars().count();
            ToolCost {
                tool: t.name.clone(),
                chars,
                approx_tokens: chars / 4,
            }
        })
        .collect();
    let total_chars = costs.iter().map(|c| c.chars).sum();
    costs.sort_by(|a, b| b.chars.cmp(&a.chars));
    costs.truncate(5);
    LintReport {
        tool_count: tools.len(),
        findings,
        total_chars,
        approx_tokens: total_chars / 4,
        heaviest: costs,
    }
}

/// Splits `deleteThing`, `delete_thing` and `delete-thing` into lowercase words.
fn words(name: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut prev_lower = false;
    for c in name.chars() {
        if !c.is_alphanumeric() || (c.is_uppercase() && prev_lower) {
            out.push(String::new());
        }
        if c.is_alphanumeric() {
            out.last_mut().unwrap().extend(c.to_lowercase());
        }
        prev_lower = c.is_lowercase();
    }
    out.retain(|w| !w.is_empty());
    out
}

impl fmt::Display for LintReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "Lint: {} tools, {} findings ({} errors)",
            self.tool_count,
            self.findings.len(),
            self.error_count()
        )?;
        for x in &self.findings {
            writeln!(
                f,
                "  {:<5}  {:<32}  {:<26}  {}",
                x.severity, x.tool, x.rule, x.message
            )?;
        }
        writeln!(
            f,
            "\nContext cost: {} characters, about {} tokens",
            self.total_chars, self.approx_tokens
        )?;
        writeln!(f, "Heaviest tools:")?;
        for c in &self.heaviest {
            writeln!(
                f,
                "  {:<32}  {:>7} chars  {:>6} tokens",
                c.tool, c.chars, c.approx_tokens
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, description: &str) -> ToolDef {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "description": description,
            "inputSchema": {"type": "object"},
            "annotations": {"readOnlyHint": true},
        }))
        .unwrap()
    }

    #[test]
    fn deprecated_rule_wants_the_tool_itself_retired() {
        // Real descriptions from sarvam-mcp 0.2.13.
        let validator = "Build-time tool — helps write code that uses Sarvam. For runtime actions, use sarvam_tools_* instead.\n\nValidate a draft Sarvam API request body against the canonical schema BEFORE the dev sends it. Catches: invalid model names, speaker/model mismatches, deprecated values, unsupported language codes, missing required fields. Returns a list of issues with fix suggestions.";
        let analytics = "Runtime tool — calls Sarvam API now. For code-writing help, use sarvam_code_* tools.\n\nPERMANENTLY REMOVED: Sarvam retired the old analytics/parse API family (this `/text-analytics` endpoint included) — confirmed via a direct API call outside this tool (404, first observed 2026-08-13). Calling this tool raises immediately.";
        let report = lint_tools(&[
            tool("validate_request", validator),
            tool("text_analytics", analytics),
            tool(
                "get_sandbox",
                "Returns 404 if the named sandbox no longer exists.",
            ),
            tool(
                "list_projects",
                "Lists projects. **The `team` filter is deprecated.** Use memberships.",
            ),
            tool("old", "DEPRECATED: use new_thing instead of this tool."),
            tool("gone", "Fetches a report. No longer available since March."),
        ]);
        let flagged = |t: &str| {
            report
                .findings
                .iter()
                .any(|f| f.tool == t && f.rule == "deprecated-exposed")
        };
        assert!(!flagged("validate_request"));
        assert!(!flagged("get_sandbox"));
        assert!(!flagged("list_projects"));
        assert!(flagged("text_analytics"));
        assert!(flagged("old"));
        assert!(flagged("gone"));
    }

    #[test]
    fn splits_names() {
        assert_eq!(words("deleteThing"), ["delete", "thing"]);
        assert_eq!(words("sarvam_tools-drop"), ["sarvam", "tools", "drop"]);
        assert_eq!(words("dropdown"), ["dropdown"]);
    }
}
