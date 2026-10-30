use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::Error;

pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
pub const DEFAULT_MAX_TURNS: u32 = 12;

/// How to start an MCP server over stdio.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    #[serde(default)]
    pub tools_called: Vec<String>,
    #[serde(default)]
    pub tools_not_called: Vec<String>,
    #[serde(default)]
    pub answer_contains: Vec<String>,
    pub max_tool_errors: Option<usize>,
    pub max_tool_calls: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub prompt: String,
    #[serde(default)]
    pub expect: Expect,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskFile {
    pub server: ServerConfig,
    #[serde(default = "default_model")]
    pub model: String,
    pub effort: Option<String>,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    pub system: Option<String>,
    pub tasks: Vec<Task>,
}

fn default_model() -> String {
    DEFAULT_MODEL.into()
}

fn default_max_turns() -> u32 {
    DEFAULT_MAX_TURNS
}

impl TaskFile {
    /// Parses YAML and expands `${VAR}` / `${VAR:-default}` in the server section from the
    /// process environment.
    pub fn from_yaml(yaml: &str) -> Result<Self, Error> {
        Self::from_yaml_with_env(yaml, |k| std::env::var(k).ok())
    }

    pub fn from_yaml_with_env(
        yaml: &str,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, Error> {
        let mut file: TaskFile =
            serde_saphyr::from_str(yaml).map_err(|e| Error::Config(e.to_string()))?;
        let s = &mut file.server;
        s.command = expand(&s.command, &env)?;
        for a in &mut s.args {
            *a = expand(a, &env)?;
        }
        for v in s.env.values_mut() {
            *v = expand(v, &env)?;
        }
        Ok(file)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let yaml = std::fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
        Self::from_yaml(&yaml)
    }
}

/// Replaces `${VAR}` and `${VAR:-default}`. An unset variable without a default is an error
/// naming it. A variable set to the empty string counts as set.
fn expand(s: &str, env: &impl Fn(&str) -> Option<String>) -> Result<String, Error> {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find('}')
            .ok_or_else(|| Error::Config(format!("unterminated `${{` in `{s}`")))?;
        let (name, default) = match after[..end].split_once(":-") {
            Some((n, d)) => (n, Some(d)),
            None => (&after[..end], None),
        };
        match (env(name), default) {
            (Some(v), _) => out.push_str(&v),
            (None, Some(d)) => out.push_str(d),
            (None, None) => {
                return Err(Error::Config(format!(
                    "environment variable {name} is not set"
                )));
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = "
server:
  command: uvx
  args: [\"x==1\"]
  env:
    A: ${A}
    B: ${B:-fallback}
    C: pre-${A}-post
tasks:
  - id: t
    prompt: hi
";

    #[test]
    fn expands_vars_and_defaults() {
        let f =
            TaskFile::from_yaml_with_env(YAML, |k| (k == "A").then(|| "a".to_string())).unwrap();
        assert_eq!(f.server.env["A"], "a");
        assert_eq!(f.server.env["B"], "fallback");
        assert_eq!(f.server.env["C"], "pre-a-post");
        assert_eq!(f.model, DEFAULT_MODEL);
        assert_eq!(f.max_turns, DEFAULT_MAX_TURNS);
    }

    #[test]
    fn missing_var_names_it() {
        let err = TaskFile::from_yaml_with_env(YAML, |_| None).unwrap_err();
        assert!(err.to_string().contains("environment variable A"), "{err}");
    }
}
