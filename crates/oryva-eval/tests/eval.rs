use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use oryva_eval::{
    Error, Model, ModelRequest, ModelResponse, ServerConfig, Severity, TaskFile, Usage, lint, run,
};
use serde_json::{Value, json};

fn server() -> ServerConfig {
    ServerConfig {
        command: env!("CARGO_BIN_EXE_oryva-test-server").into(),
        args: vec![],
        env: Default::default(),
    }
}

#[tokio::test]
async fn lint_flags_the_test_server() {
    let report = lint(&server()).await.unwrap();
    assert_eq!(report.tool_count, 4);
    let has = |tool: &str, rule: &str, sev: Severity| {
        report
            .findings
            .iter()
            .any(|f| f.tool == tool && f.rule == rule && f.severity == sev)
    };
    assert!(has("delete_thing", "destructive-unmarked", Severity::Error));
    assert!(has("delete_thing", "annotation-missing", Severity::Warn));
    assert!(has("delete_thing", "description-short", Severity::Warn));
    assert!(has(
        "delete_thing",
        "param-description-missing",
        Severity::Warn
    ));
    assert!(has("retired_tool", "deprecated-exposed", Severity::Error));
    // echo is clean: well described, annotated, documented parameter.
    assert!(report.findings.iter().all(|f| f.tool != "echo"), "{report}");
    assert_eq!(report.error_count(), 2);
    assert_eq!(report.heaviest.len(), 4);
    assert_eq!(report.approx_tokens, report.total_chars / 4);
    assert!(report.to_string().contains("Context cost"));
    assert!(serde_json::to_value(&report).unwrap()["findings"].is_array());
}

struct Scripted {
    responses: Mutex<VecDeque<ModelResponse>>,
    /// The `messages` array as sent on each call.
    seen: Mutex<Vec<Vec<Value>>>,
}

#[async_trait]
impl Model for Scripted {
    async fn complete(&self, req: &ModelRequest<'_>) -> Result<ModelResponse, Error> {
        self.seen.lock().unwrap().push(req.messages.to_vec());
        Ok(self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("script ran out"))
    }
}

fn tool_use(calls: &[(&str, &str, Value)], extra: Option<Value>) -> ModelResponse {
    let mut content: Vec<Value> = extra.into_iter().collect();
    content.extend(calls.iter().map(
        |(id, name, input)| json!({"type": "tool_use", "id": id, "name": name, "input": input}),
    ));
    ModelResponse {
        content,
        stop_reason: Some("tool_use".into()),
        stop_details: None,
        usage: Usage {
            input_tokens: 100,
            output_tokens: 10,
            cache_read_tokens: 50,
        },
    }
}

fn end(text: &str) -> ModelResponse {
    ModelResponse {
        content: vec![json!({"type": "text", "text": text})],
        stop_reason: Some("end_turn".into()),
        stop_details: None,
        usage: Usage {
            input_tokens: 200,
            output_tokens: 20,
            cache_read_tokens: 0,
        },
    }
}

#[tokio::test]
async fn run_scores_pass_recovery_and_failure() {
    let yaml = format!(
        r#"
server:
  command: {}
tasks:
  - id: echo-pass
    prompt: Echo hello.
    expect:
      tools_called: [echo]
      answer_contains: [HELLO]
      max_tool_errors: 0
  - id: flaky-recovers
    prompt: Call flaky until it works.
    expect:
      tools_called: [flaky]
      max_tool_errors: 1
      max_tool_calls: 2
  - id: retired-fails
    prompt: Use the retired tool.
    expect:
      tools_not_called: [delete_thing]
      answer_contains: [never printed]
      max_tool_errors: 0
"#,
        server().command
    );
    let file = TaskFile::from_yaml(&yaml).unwrap();
    let thinking = json!({"type": "thinking", "thinking": "hmm", "signature": "sig"});
    let model = Scripted {
        responses: Mutex::new(VecDeque::from([
            // echo-pass: two parallel calls in one turn, the second to a tool that errors on args
            tool_use(
                &[
                    ("a", "echo", json!({"message": "hello"})),
                    ("b", "echo", json!({"message": "again"})),
                ],
                Some(thinking.clone()),
            ),
            end("It said hello."),
            // flaky-recovers
            tool_use(&[("c", "flaky", json!({}))], None),
            tool_use(&[("d", "flaky", json!({}))], None),
            end("done"),
            // retired-fails
            tool_use(&[("e", "retired_tool", json!({}))], None),
            end("gave up"),
        ])),
        seen: Mutex::new(vec![]),
    };

    let report = run(&file, &model).await.unwrap();
    let [echo, flaky, retired] = &report.tasks[..] else {
        panic!("{report}")
    };

    assert!(echo.passed, "{:?}", echo.failures);
    assert_eq!(
        (
            echo.turns,
            echo.tool_calls.len(),
            echo.tool_errors,
            echo.recoveries
        ),
        (2, 2, 0, 0)
    );
    assert_eq!(
        (
            echo.input_tokens,
            echo.output_tokens,
            echo.cache_read_tokens
        ),
        (300, 30, 50)
    );

    assert!(flaky.passed, "{:?}", flaky.failures);
    assert_eq!(
        (flaky.tool_errors, flaky.recoveries, flaky.tool_calls.len()),
        (1, 1, 2)
    );
    assert!(
        !flaky.tool_calls[0].ok
            && flaky.tool_calls[0]
                .error
                .as_deref()
                .unwrap()
                .contains("temporary failure")
    );
    assert!(flaky.tool_calls[1].ok);

    assert!(!retired.passed);
    assert_eq!((retired.tool_errors, retired.recoveries), (1, 0));
    assert_eq!(retired.failures.len(), 2, "{:?}", retired.failures);
    assert!(retired.failures.iter().any(|f| f.contains("never printed")));
    assert!(
        retired
            .failures
            .iter()
            .any(|f| f.contains("1 tool errors, max 0"))
    );

    let s = &report.summary;
    assert_eq!((s.tasks, s.passed), (3, 1 + 1));
    assert!((s.pass_rate - 2.0 / 3.0).abs() < 1e-9);
    assert_eq!((s.tool_errors, s.recoveries), (2, 1));
    let text = report.to_string();
    assert!(text.contains("retired-fails") && text.contains("FAIL") && text.contains("Summary"));
    assert_eq!(
        serde_json::to_value(&report).unwrap()["summary"]["tool_errors"],
        2
    );

    // The wire contract: assistant turn echoed unchanged, all results in one user message, in order.
    let seen = model.seen.lock().unwrap();
    let second_call = &seen[1];
    assert_eq!(second_call[1]["role"], "assistant");
    assert_eq!(second_call[1]["content"][0], thinking);
    let results = second_call[2]["content"].as_array().unwrap();
    assert_eq!(second_call[2]["role"], "user");
    assert_eq!(results.len(), 2);
    assert_eq!(
        (
            results[0]["tool_use_id"].as_str(),
            results[1]["tool_use_id"].as_str()
        ),
        (Some("a"), Some("b"))
    );
    assert_eq!(results[0]["content"], "hello");
    assert!(results[0].get("is_error").is_none());
    // flaky's first result carries is_error.
    assert_eq!(seen[3][2]["content"][0]["is_error"], true);
}

#[tokio::test]
async fn run_fails_task_on_bad_stop_reasons() {
    let file = TaskFile::from_yaml(&format!(
        "server:\n  command: {}\nmax_turns: 2\ntasks:\n  - id: refused\n    prompt: x\n  - id: cut\n    prompt: x\n  - id: loops\n    prompt: x\n",
        server().command
    ))
    .unwrap();
    let mut refusal = end("");
    refusal.stop_reason = Some("refusal".into());
    refusal.stop_details = Some(json!({"category": "cyber"}));
    let mut cut = end("");
    cut.stop_reason = Some("max_tokens".into());
    let model = Scripted {
        responses: Mutex::new(VecDeque::from([
            refusal,
            cut,
            tool_use(&[("a", "echo", json!({"message": "x"}))], None),
            tool_use(&[("b", "echo", json!({"message": "x"}))], None),
        ])),
        seen: Mutex::new(vec![]),
    };
    let report = run(&file, &model).await.unwrap();
    assert!(report.tasks.iter().all(|t| !t.passed));
    assert!(report.tasks[0].failures[0].contains("cyber"));
    assert!(report.tasks[1].failures[0].contains("max_tokens"));
    assert!(report.tasks[2].failures[0].contains("max_turns"));
}

#[tokio::test]
async fn failed_call_in_a_passing_task_counts_as_a_recovery() {
    // The failing tool never succeeds later; the task passes anyway.
    let file = TaskFile::from_yaml(&format!(
        "server:\n  command: {}\ntasks:\n  - id: shrug-off\n    prompt: x\n    expect:\n      tools_called: [echo]\n",
        server().command
    ))
    .unwrap();
    let model = Scripted {
        responses: Mutex::new(VecDeque::from([
            tool_use(&[("a", "retired_tool", json!({}))], None),
            tool_use(&[("b", "echo", json!({"message": "hi"}))], None),
            end("done"),
        ])),
        seen: Mutex::new(vec![]),
    };
    let report = run(&file, &model).await.unwrap();
    let t = &report.tasks[0];
    assert!(t.passed, "{:?}", t.failures);
    assert_eq!((t.tool_errors, t.recoveries), (1, 1));
}
