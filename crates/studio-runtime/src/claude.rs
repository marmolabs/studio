//! Claude Code, driven headless: `claude -p` with stream-json in both
//! directions. Permission prompts arrive as `can_use_tool` control requests
//! (`--permission-prompt-tool stdio`) and are answered with control responses,
//! the same protocol the official Agent SDKs speak. The user's own Claude Code
//! login or API key is used by the CLI itself; nothing here reads credentials.

use crate::Runtime;
use crate::run::{
    Capabilities, Control, Cost, EnvReport, FailureKind, Outcome, Question, Run, RunEvent, RunSpec,
    RuntimeError,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;

const QUESTION_TOOL: &str = "AskUserQuestion";
const STDERR_LIMIT: usize = 4096;

/// The longest an agent's shell command may run: 30 minutes, as a check.
const BASH_MAX_TIMEOUT_MS: &str = "1800000";

pub struct ClaudeCode {
    program: PathBuf,
    prefix_args: Vec<String>,
}

impl Default for ClaudeCode {
    fn default() -> Self {
        ClaudeCode {
            program: PathBuf::from("claude"),
            prefix_args: Vec::new(),
        }
    }
}

impl ClaudeCode {
    /// A stand-in program (and leading arguments) for tests.
    #[cfg(test)]
    fn with_program(program: impl Into<PathBuf>, prefix_args: Vec<String>) -> ClaudeCode {
        ClaudeCode {
            program: program.into(),
            prefix_args,
        }
    }

    fn args(spec: &RunSpec) -> Vec<String> {
        let mut args: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-prompt-tool",
            "stdio",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut push = |flag: &str, value: String| {
            args.push(flag.to_string());
            args.push(value);
        };
        if let Some(model) = &spec.model {
            push("--model", model.clone());
        }
        if let Some(session) = &spec.resume {
            push("--resume", session.clone());
        }
        if let Some(effort) = &spec.effort {
            push("--effort", effort.clone());
        }
        if let Some(prompt) = &spec.append_system_prompt {
            push("--append-system-prompt", prompt.clone());
        }
        if let Some(turns) = spec.max_turns {
            push("--max-turns", turns.to_string());
        }
        if let Some(budget) = spec.max_budget_usd {
            push("--max-budget-usd", budget.to_string());
        }
        if let Some(config) = &spec.mcp_config {
            push("--mcp-config", config.to_string_lossy().into_owned());
        }
        if !spec.allowed_tools.is_empty() {
            push("--allowedTools", spec.allowed_tools.join(","));
        }
        if !spec.disallowed_tools.is_empty() {
            push("--disallowedTools", spec.disallowed_tools.join(","));
        }
        if spec.strict_mcp {
            args.push("--strict-mcp-config".to_string());
        }
        args
    }
}

impl Runtime for ClaudeCode {
    fn id(&self) -> &str {
        "claude-code"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            skills: true,
            mcp: true,
            resume: true,
            approvals: true,
            reports_cost: true,
        }
    }

    fn check_env(&self) -> EnvReport {
        let out = std::process::Command::new(&self.program)
            .args(&self.prefix_args)
            .arg("--version")
            .output();
        match out {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout);
                EnvReport {
                    installed: true,
                    version: text.split_whitespace().next().map(str::to_string),
                    problem: None,
                }
            }
            Ok(out) => EnvReport {
                installed: true,
                version: None,
                problem: Some(String::from_utf8_lossy(&out.stderr).trim().to_string()),
            },
            Err(err) => EnvReport {
                installed: false,
                version: None,
                problem: Some(err.to_string()),
            },
        }
    }

    fn start(&self, spec: RunSpec) -> Result<Run, RuntimeError> {
        let mut child = Command::new(&self.program)
            .args(&self.prefix_args)
            .args(ClaudeCode::args(&spec))
            .current_dir(&spec.cwd)
            .envs(spec.env.iter().map(|(name, value)| (name, value)))
            // A run ends when the agent stops, so nothing may keep running in
            // the background after it, and a long build may take as long as
            // one of the project's checks.
            .env("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "1")
            .env("BASH_MAX_TIMEOUT_MS", BASH_MAX_TIMEOUT_MS)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(RuntimeError::Spawn(std::io::Error::other(
                "child pipes were not captured",
            )));
        };
        let (event_tx, event_rx) = mpsc::channel(256);
        let (control_tx, control_rx) = mpsc::channel(32);
        tokio::spawn(drive(
            child,
            stdin,
            stdout,
            stderr,
            spec.prompt,
            control_rx,
            event_tx,
        ));
        Ok(Run::new(event_rx, control_tx))
    }
}

/// One parsed line of Claude's stream-json output.
#[derive(Clone, Debug, PartialEq)]
enum Parsed {
    Event(RunEvent),
    /// A control request the studio doesn't handle; it gets an error reply so
    /// the CLI never waits on it.
    Unsupported {
        request_id: String,
    },
}

/// Parses one stdout line. Lines that aren't JSON, or carry nothing the studio
/// uses, give an empty list.
fn parse_line(line: &str) -> Vec<Parsed> {
    let Ok(msg) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    match msg["type"].as_str().unwrap_or_default() {
        "system" if msg["subtype"] == "init" => vec![Parsed::Event(RunEvent::Started {
            session: text(&msg["session_id"]),
            model: msg["model"].as_str().map(str::to_string),
        })],
        "assistant" => usage(&msg)
            .into_iter()
            .chain(
                blocks(&msg)
                    .filter_map(|block| match block["type"].as_str()? {
                        "text" => Some(RunEvent::Text {
                            text: text(&block["text"]),
                        }),
                        "tool_use" => Some(RunEvent::ToolCall {
                            id: text(&block["id"]),
                            name: text(&block["name"]),
                            input: block["input"].clone(),
                        }),
                        _ => None,
                    })
                    .filter(|event| !matches!(event, RunEvent::Text { text } if text.is_empty())),
            )
            .map(Parsed::Event)
            .collect(),
        "user" => blocks(&msg)
            .filter(|block| block["type"] == "tool_result")
            .map(|block| {
                Parsed::Event(RunEvent::ToolResult {
                    id: text(&block["tool_use_id"]),
                    output: result_text(&block["content"]),
                    is_error: block["is_error"].as_bool().unwrap_or(false),
                })
            })
            .collect(),
        "control_request" => {
            let request_id = text(&msg["request_id"]);
            let request = &msg["request"];
            if request["subtype"] != "can_use_tool" {
                return vec![Parsed::Unsupported { request_id }];
            }
            let tool = text(&request["tool_name"]);
            let event = if tool == QUESTION_TOOL {
                RunEvent::Question {
                    request_id,
                    questions: questions(&request["input"]),
                }
            } else {
                RunEvent::Permission {
                    request_id,
                    tool,
                    input: request["input"].clone(),
                }
            };
            vec![Parsed::Event(event)]
        }
        "rate_limit_event" => vec![Parsed::Event(RunEvent::RateLimit {
            status: text(&msg["rate_limit_info"]["status"]),
            resets_at: msg["rate_limit_info"]["resetsAt"].as_u64(),
        })],
        "result" => vec![Parsed::Event(finished(&msg))],
        _ => Vec::new(),
    }
}

fn usage(msg: &Value) -> Option<RunEvent> {
    let message = &msg["message"];
    let usage = message.get("usage")?;
    let count = |key: &str| usage[key].as_u64().unwrap_or(0);
    Some(RunEvent::Usage {
        message_id: message["id"].as_str()?.to_string(),
        model: message["model"].as_str().map(str::to_string),
        input_tokens: count("input_tokens"),
        output_tokens: count("output_tokens"),
        cache_read_tokens: count("cache_read_input_tokens"),
        cache_write_tokens: count("cache_creation_input_tokens"),
    })
}

fn blocks(msg: &Value) -> impl Iterator<Item = &Value> {
    msg["message"]["content"].as_array().into_iter().flatten()
}

fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn questions(input: &Value) -> Vec<Question> {
    input["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|q| Question {
            question: q["question"].as_str().unwrap_or_default().to_string(),
            header: q["header"].as_str().unwrap_or_default().to_string(),
            options: q["options"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|o| o["label"].as_str().map(str::to_string))
                .collect(),
            multi_select: q["multiSelect"].as_bool().unwrap_or(false),
        })
        .collect()
}

fn finished(msg: &Value) -> RunEvent {
    let summary = msg["result"].as_str().map(str::to_string);
    let outcome = match msg["subtype"].as_str().unwrap_or_default() {
        "success" if msg["is_error"] != true => Outcome::Success,
        "error_max_turns" => Outcome::MaxTurns,
        "error_max_budget_usd" => Outcome::Budget,
        other => {
            let message = summary.clone().unwrap_or_else(|| other.to_string());
            Outcome::Failed {
                kind: classify(&message),
                message,
            }
        }
    };
    RunEvent::Finished {
        outcome,
        session: msg["session_id"].as_str().map(str::to_string),
        cost: Cost {
            usd: msg["total_cost_usd"].as_f64().unwrap_or(0.0),
            reported: true,
            input_tokens: msg["usage"]["input_tokens"].as_u64().unwrap_or(0),
            output_tokens: msg["usage"]["output_tokens"].as_u64().unwrap_or(0),
            // Claude reports the whole session's cost, including resumed runs.
            cumulative: true,
        },
        summary,
    }
}

/// Sorts a failure message into a class the scheduler can act on.
pub(crate) fn classify(message: &str) -> FailureKind {
    let m = message.to_lowercase();
    if [
        "usage limit",
        "session limit",
        "weekly limit",
        "rate limit",
        "429",
        "overloaded",
    ]
    .iter()
    .any(|s| m.contains(s))
    {
        FailureKind::Quota
    } else if [
        "log in",
        "login",
        "authenticat",
        "401",
        "api key",
        "unauthorized",
    ]
    .iter()
    .any(|s| m.contains(s))
    {
        FailureKind::Auth
    } else if m.contains("timed out") || m.contains("timeout") {
        FailureKind::Timeout
    } else {
        FailureKind::Crash
    }
}

fn user_message(prompt: &str) -> Value {
    json!({
        "type": "user",
        "message": { "role": "user", "content": prompt },
        "parent_tool_use_id": null,
        "session_id": "",
    })
}

fn control_request(request_id: &str, request: Value) -> Value {
    json!({ "type": "control_request", "request_id": request_id, "request": request })
}

fn control_response(request_id: &str, response: Value) -> Value {
    json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": response },
    })
}

fn control_error(request_id: &str, error: &str) -> Value {
    json!({
        "type": "control_response",
        "response": { "subtype": "error", "request_id": request_id, "error": error },
    })
}

/// Turns a studio decision into the line Claude expects. `pending` holds the
/// original tool input per request, which an `allow` must echo back.
fn reply(control: Control, pending: &mut HashMap<String, Value>, next_id: &mut u64) -> Value {
    match control {
        Control::Allow { request_id } => {
            let input = pending.remove(&request_id).unwrap_or(Value::Null);
            control_response(
                &request_id,
                json!({ "behavior": "allow", "updatedInput": input }),
            )
        }
        Control::Deny {
            request_id,
            message,
        } => {
            pending.remove(&request_id);
            control_response(
                &request_id,
                json!({ "behavior": "deny", "message": message }),
            )
        }
        Control::Answer {
            request_id,
            answers,
        } => {
            let mut input = pending.remove(&request_id).unwrap_or_else(|| json!({}));
            if let Value::Object(map) = &mut input {
                map.insert("answers".to_string(), json!(answers));
            }
            control_response(
                &request_id,
                json!({ "behavior": "allow", "updatedInput": input }),
            )
        }
        Control::Interrupt => {
            *next_id += 1;
            control_request(
                &format!("studio_{next_id}"),
                json!({ "subtype": "interrupt" }),
            )
        }
    }
}

async fn write_line(stdin: &mut Option<ChildStdin>, value: &Value) {
    let Some(pipe) = stdin.as_mut() else {
        return;
    };
    let line = format!("{value}\n");
    if pipe.write_all(line.as_bytes()).await.is_err() || pipe.flush().await.is_err() {
        *stdin = None;
    }
}

async fn drive(
    mut child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr: ChildStderr,
    prompt: String,
    mut controls: mpsc::Receiver<Control>,
    events: mpsc::Sender<RunEvent>,
) {
    let stderr_task = tokio::spawn(read_limited(stderr));
    let mut stdin = Some(stdin);
    let mut next_id = 0u64;
    write_line(
        &mut stdin,
        &control_request("studio_0", json!({ "subtype": "initialize" })),
    )
    .await;
    write_line(&mut stdin, &user_message(&prompt)).await;

    let mut lines = BufReader::new(stdout).lines();
    let mut pending: HashMap<String, Value> = HashMap::new();
    let mut finished = false;
    let mut controls_open = true;
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Ok(Some(line)) = line else { break };
                for parsed in parse_line(&line) {
                    match parsed {
                        Parsed::Unsupported { request_id } => {
                            write_line(&mut stdin, &control_error(&request_id, "not supported")).await;
                        }
                        Parsed::Event(event) => {
                            remember_input(&line, &event, &mut pending);
                            if matches!(event, RunEvent::Finished { .. }) {
                                finished = true;
                                // Closing stdin lets the CLI exit.
                                stdin = None;
                            }
                            let _ = events.send(event).await;
                        }
                    }
                }
            }
            control = controls.recv(), if controls_open => match control {
                Some(control) => {
                    let line = reply(control, &mut pending, &mut next_id);
                    write_line(&mut stdin, &line).await;
                }
                None => {
                    // The studio dropped the run: stop the process.
                    controls_open = false;
                    let _ = child.start_kill();
                }
            },
        }
    }
    drop(stdin);
    let status = child.wait().await;
    let stderr = stderr_task.await.unwrap_or_default();
    if !finished {
        let message = match (&status, stderr.trim()) {
            (_, text) if !text.is_empty() => text.to_string(),
            (Ok(status), _) => format!("claude exited with {status} before finishing"),
            (Err(err), _) => err.to_string(),
        };
        let _ = events
            .send(RunEvent::Finished {
                outcome: Outcome::Failed {
                    kind: classify(&message),
                    message,
                },
                session: None,
                cost: Cost::default(),
                summary: None,
            })
            .await;
    }
}

/// Keeps the raw tool input of a permission or question request, so an allow
/// can echo it back unchanged.
fn remember_input(line: &str, event: &RunEvent, pending: &mut HashMap<String, Value>) {
    let request_id = match event {
        RunEvent::Permission { request_id, .. } | RunEvent::Question { request_id, .. } => {
            request_id
        }
        _ => return,
    };
    if let Ok(msg) = serde_json::from_str::<Value>(line) {
        pending.insert(request_id.clone(), msg["request"]["input"].clone());
    }
}

async fn read_limited(mut stderr: ChildStderr) -> String {
    let mut buf = Vec::new();
    let _ = stderr.read_to_end(&mut buf).await;
    let text = String::from_utf8_lossy(&buf);
    let start = text.len().saturating_sub(STDERR_LIMIT);
    let start = (start..text.len())
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(text.len());
    text[start..].to_string()
}

#[cfg(test)]
mod recorded;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    #[test]
    fn classifies_failures() {
        assert_eq!(
            classify("You've hit your session limit · resets 5pm"),
            FailureKind::Quota
        );
        assert_eq!(
            classify("API Error: 429 rate_limit_error"),
            FailureKind::Quota
        );
        assert_eq!(
            classify("Invalid API key · Please run /login"),
            FailureKind::Auth
        );
        assert_eq!(classify("Request timed out"), FailureKind::Timeout);
        assert_eq!(classify("something broke"), FailureKind::Crash);
    }

    #[test]
    fn builds_arguments() {
        let spec = RunSpec {
            model: Some("haiku".into()),
            resume: Some("abc".into()),
            effort: Some("high".into()),
            max_turns: Some(5),
            max_budget_usd: Some(1.5),
            allowed_tools: vec!["Read".into(), "Bash(git diff *)".into()],
            strict_mcp: true,
            ..RunSpec::default()
        };
        let args = ClaudeCode::args(&spec).join(" ");
        assert!(args.starts_with("-p --input-format stream-json --output-format stream-json"));
        assert!(args.contains("--permission-prompt-tool stdio"));
        assert!(args.contains(
            "--model haiku --resume abc --effort high --max-turns 5 --max-budget-usd 1.5"
        ));
        assert!(args.contains("--allowedTools Read,Bash(git diff *)"));
        assert!(!args.contains("--bare"));
        assert!(args.ends_with("--strict-mcp-config"));
    }

    #[test]
    fn replies_echo_the_original_input() {
        let mut pending = HashMap::new();
        pending.insert("r1".to_string(), json!({ "file_path": "a.txt" }));
        let mut next = 0;
        let allow = reply(
            Control::Allow {
                request_id: "r1".into(),
            },
            &mut pending,
            &mut next,
        );
        assert_eq!(allow["response"]["request_id"], "r1");
        assert_eq!(allow["response"]["response"]["behavior"], "allow");
        assert_eq!(
            allow["response"]["response"]["updatedInput"]["file_path"],
            "a.txt"
        );
        assert!(pending.is_empty());

        let interrupt = reply(Control::Interrupt, &mut pending, &mut next);
        assert_eq!(interrupt["type"], "control_request");
        assert_eq!(interrupt["request"]["subtype"], "interrupt");
    }

    #[test]
    fn unknown_control_requests_are_flagged() {
        let line =
            r#"{"type":"control_request","request_id":"x","request":{"subtype":"hook_callback"}}"#;
        assert_eq!(
            parse_line(line),
            vec![Parsed::Unsupported {
                request_id: "x".into()
            }]
        );
        assert!(parse_line("not json").is_empty());
    }
}
