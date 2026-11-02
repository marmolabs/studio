//! The studio-tools MCP server: newline-delimited JSON-RPC 2.0 on stdio. It
//! holds no state but what it serves; every tool is a request to studiod.
//!
//! It serves one of three things, chosen when it starts (see `Mode`): the tools
//! of one ticket's agent, those of a lead talking with the owner about a
//! project, or those of a lead setting a project up. Each has a list of tools
//! and a way of calling them of its own (`ticket_tools` and `call_ticket_tool`,
//! `talk_tools` and `call_talk_tool`, `setup_tools` and `call_setup_tool`), so
//! changing one leaves the others alone.

use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::Path;
use studiod::api::{LeadAct, Request, Response, call};

const DEFAULT_PROTOCOL: &str = "2025-06-18";

/// Sends a request to studiod and returns the tool's text, or why it failed.
type Forward<'a> = &'a dyn Fn(&Request) -> Result<Response, String>;

/// Whose tools the server offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The agent of one ticket, by ticket id.
    Ticket(String),
    /// A lead talking with the owner about a project, by project name.
    Talk(String),
    /// A lead setting a project up, by project name.
    Setup(String),
}

pub fn serve(socket: &Path, mode: &Mode) -> Result<(), String> {
    let forward = |request: &Request| {
        call(socket, request)
            .map_err(|e| format!("can't reach studiod at {}: {e}", socket.display()))
    };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle_line(&line, mode, &forward) {
            writeln!(stdout, "{reply}")
                .and_then(|()| stdout.flush())
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// The reply to one request line; none for notifications.
fn handle_line(line: &str, mode: &Mode, forward: Forward) -> Option<Value> {
    let message: Value = match serde_json::from_str(line) {
        Ok(message) => message,
        Err(err) => return Some(failure(Value::Null, -32700, &format!("parse error: {err}"))),
    };
    // No id means a notification, which never gets a reply.
    let id = message.get("id")?.clone();
    let params = &message["params"];
    let result = match message["method"].as_str().unwrap_or_default() {
        "initialize" => json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or(DEFAULT_PROTOCOL),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "studio", "version": env!("CARGO_PKG_VERSION") },
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools(mode) }),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            match call_tool(name, &params["arguments"], mode, forward) {
                Ok(text) => tool_result(&text, false),
                Err(text) => tool_result(&text, true),
            }
        }
        other => return Some(failure(id, -32601, &format!("method not found: {other}"))),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn failure(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tool_result(text: &str, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

/// The tools of the mode the server was started in.
fn tools(mode: &Mode) -> Value {
    match mode {
        Mode::Ticket(_) => ticket_tools(),
        Mode::Talk(_) => talk_tools(),
        Mode::Setup(_) => setup_tools(),
    }
}

/// Runs a tool of the mode the server was started in.
fn call_tool(name: &str, args: &Value, mode: &Mode, forward: Forward) -> Result<String, String> {
    match mode {
        Mode::Ticket(ticket) => call_ticket_tool(name, args, ticket, forward),
        Mode::Talk(project) => call_talk_tool(name, args, project, forward),
        Mode::Setup(project) => call_setup_tool(name, args, project, forward),
    }
}

/// Sends `request` to studiod and turns its answer into the tool's text, or
/// into why it failed.
fn ask_studiod(request: &Request, forward: Forward) -> Result<String, String> {
    let response = forward(request)?;
    if !response.ok {
        return Err(response.error.unwrap_or_else(|| "request failed".into()));
    }
    Ok(match response.data {
        Value::String(text) => text,
        other => other.to_string(),
    })
}

/// The tools of a lead talking with the owner about a project: start a goal,
/// and act on the project's tasks.
fn talk_tools() -> Value {
    let text = |description: &str| json!({ "type": "string", "description": description });
    let task = || text("The task's id, such as SHOP-3.");
    json!([
        {
            "name": "start_goal",
            "description": "Start a goal: you plan it, and the tasks you file reach it. Use it when the owner wants something new done. It answers the goal's id; the plan files its tasks a little later.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": text("A short name for the goal, in the owner's words."),
                    "version": text("The version the goal is a milestone towards, such as 0.4, if the owner named one."),
                    "detail": text("What the goal needs in full: context, what done looks like."),
                },
                "required": ["text"],
            },
        },
        {
            "name": "send_back",
            "description": "Send a task that waits for the owner's review back to its agent, with a note on what to change.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task": task(),
                    "note": text("What to change, in plain words."),
                },
                "required": ["task", "note"],
            },
        },
        {
            "name": "pause",
            "description": "Stop a running task's agent and hold the task until it is resumed.",
            "inputSchema": {
                "type": "object",
                "properties": { "task": task() },
                "required": ["task"],
            },
        },
        {
            "name": "resume",
            "description": "Continue a task that is held.",
            "inputSchema": {
                "type": "object",
                "properties": { "task": task() },
                "required": ["task"],
            },
        },
        {
            "name": "cancel",
            "description": "Cancel a task for good.",
            "inputSchema": {
                "type": "object",
                "properties": { "task": task() },
                "required": ["task"],
            },
        },
        {
            "name": "tell",
            "description": "Tell the agent working on a task something, such as a change of plan.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task": task(),
                    "text": text("What to tell it."),
                },
                "required": ["task", "text"],
            },
        },
    ])
}

/// Runs one of `talk_tools` for `project`. `start_goal` answers the goal's id
/// and title as JSON; the others answer what studiod answers.
fn call_talk_tool(
    name: &str,
    args: &Value,
    project: &str,
    forward: Forward,
) -> Result<String, String> {
    let text = |key: &str| {
        args[key]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let need = |key: &str| text(key).ok_or_else(|| format!("{name} needs a {key}"));
    let request = match name {
        "start_goal" => {
            let request = Request::CreateGoal {
                project: project.to_string(),
                text: need("text")?,
                // The project's lead plans it.
                hire: None,
                detail: text("detail").unwrap_or_default(),
                version: text("version"),
            };
            let started = forward(&request)?;
            if !started.ok {
                return Err(started.error.unwrap_or_else(|| "request failed".into()));
            }
            return Ok(json!({
                "goal": started.data["id"],
                "title": started.data["goal"],
                "note": "The plan files the tasks a little later. Tell the owner the goal is started.",
            })
            .to_string());
        }
        // What a lead does to a task is asked as the lead of this project, so
        // that studiod can refuse a task that is another project's, and can tell
        // the agent who the message is from.
        "send_back" | "pause" | "resume" | "cancel" | "tell" => {
            let task = need("task")?;
            let act = match name {
                "send_back" => LeadAct::SendBack {
                    note: need("note")?,
                },
                "pause" => LeadAct::Pause,
                "resume" => LeadAct::Resume,
                "cancel" => LeadAct::Cancel,
                _ => LeadAct::Tell {
                    text: need("text")?,
                },
            };
            Request::LeadActs {
                project: project.to_string(),
                task,
                act,
            }
        }
        other => return Err(format!("no tool named {other}")),
    };
    ask_studiod(&request, forward)
}

/// The tools of one ticket's agent.
fn ticket_tools() -> Value {
    let text = |description: &str| json!({ "type": "string", "description": description });
    json!([
        {
            "name": "read_rules",
            "description": "The owner's rules that apply to this ticket.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "load_skill",
            "description": "Without a name, lists the available skills. With a name, returns that skill's instructions.",
            "inputSchema": {
                "type": "object",
                "properties": { "name": text("The skill to load.") },
            },
        },
        {
            "name": "ask_owner",
            "description": "Ask the owner a question and wait for the answer.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "question": text("The question."),
                    "options": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Answers to pick from, if there is a short list.",
                    },
                },
                "required": ["question"],
            },
        },
        {
            "name": "file_ticket",
            "description": "File a new ticket in this project, on the same goal. Only for hires allowed to.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": text("A short title."),
                    "detail": text("What to do and the acceptance criteria."),
                    "hire": text("The hire to give it to (default: the router picks)."),
                },
                "required": ["title", "detail"],
            },
        },
    ])
}

/// Runs one of `ticket_tools` for `ticket`.
fn call_ticket_tool(
    name: &str,
    args: &Value,
    ticket: &str,
    forward: Forward,
) -> Result<String, String> {
    let text = |key: &str| args[key].as_str().map(str::to_string);
    let need = |key: &str| text(key).ok_or_else(|| format!("{name} needs a {key}"));
    let ticket = ticket.to_string();
    let request = match name {
        "read_rules" => Request::ReadRules { ticket },
        "load_skill" => Request::LoadSkill {
            ticket,
            name: text("name"),
        },
        "ask_owner" => Request::AskOwner {
            ticket,
            question: need("question")?,
            options: args["options"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|o| o.as_str().map(str::to_string))
                .collect(),
        },
        "file_ticket" => Request::FileTicket {
            ticket,
            title: need("title")?,
            detail: need("detail")?,
            hire: text("hire"),
        },
        other => return Err(format!("no tool named {other}")),
    };
    ask_studiod(&request, forward)
}

/// The tools of a lead setting a project up: the one that hands over what it
/// found.
fn setup_tools() -> Value {
    let text = |description: &str| json!({ "type": "string", "description": description });
    let command = json!({
        "type": "array",
        "items": { "type": "string" },
        "description": "A program with its arguments, as a list, such as [\"make\", \"test\"]. No shell syntax; for a pipe use [\"sh\", \"-c\", \"...\"].",
    });
    let proposed = |what: &str| {
        json!({
            "type": "object",
            "description": what,
            "properties": {
                "command": command.clone(),
                "why": text("Why this one, in a few words."),
            },
            "required": ["command", "why"],
        })
    };
    json!([
        {
            "name": "propose_setup",
            "description": "Hand over how this project runs: its checks, how it is released, the paths to ask the owner about and a daily limit. Call it once, after reading the project. The studio then tries the checks in a fresh checkout.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "summary": text("What you found, in a sentence or two, in plain words."),
                    "verify": {
                        "type": "array",
                        "items": proposed("A check that shows a change works, such as the build, the tests or the linter."),
                        "description": "The checks that show a change works. Prefer what the CI runs; they must work from a clean copy and finish in a few minutes. Leave out anything that publishes or needs a login.",
                    },
                    "capture": proposed("The command that takes the pictures a change is judged by, when the project can take them. {out} in its arguments is the folder to write into."),
                    "release": {
                        "type": "array",
                        "items": command.clone(),
                        "description": "The commands that release the project, in order. Leave out if there are none.",
                    },
                    "release_source": text("Where the release commands were found, such as \"your CI\"."),
                    "protected": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "glob": text("A path glob, such as migrations/**."),
                                "why": text("Why the owner should be asked first."),
                            },
                            "required": ["glob", "why"],
                        },
                        "description": "Paths the owner is asked about before an agent touches them.",
                    },
                    "daily_usd": { "type": "number", "description": "What the project may spend in a day, in dollars." },
                    "first_goal": {
                        "type": "object",
                        "description": "What you would plan first.",
                        "properties": {
                            "text": text("The goal, in a sentence."),
                            "version": text("The version it is a milestone towards, if the project has one in mind."),
                            "why": text("Why it comes first."),
                        },
                        "required": ["text", "why"],
                    },
                },
                "required": ["summary", "verify"],
            },
        },
    ])
}

/// Runs one of `setup_tools` for `project`.
fn call_setup_tool(
    name: &str,
    args: &Value,
    project: &str,
    forward: Forward,
) -> Result<String, String> {
    if name != "propose_setup" {
        return Err(format!("no tool named {name}"));
    }
    let proposal = serde_json::from_value(args.clone()).map_err(|e| {
        format!(
            "propose_setup can't read what you gave: {e}. A command is a list of strings, \
             the program first."
        )
    })?;
    let request = Request::ProposeSetup {
        project: project.to_string(),
        proposal,
    };
    ask_studiod(&request, forward)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use std::cell::RefCell;

    fn ticket() -> Mode {
        Mode::Ticket("GAME-1".into())
    }

    fn send(line: &str) -> Option<Value> {
        handle_line(line, &ticket(), &|_| Ok(Response::ok("rules text")))
    }

    /// The reply to a call of the tool `name` in `mode`, which forwards nothing.
    fn call_in(mode: &Mode, method: &str, name: &str) -> Value {
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"{method}","params":{{"name":"{name}","arguments":{{}}}}}}"#
        );
        let forwards_nothing: Forward = &|_| Err("nothing should be sent".into());
        handle_line(&line, mode, forwards_nothing).unwrap()
    }

    #[test]
    fn initialize_echoes_the_clients_protocol() {
        let reply = send(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
        )
        .unwrap();
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(reply["result"]["serverInfo"]["name"], "studio");
        assert!(reply["result"]["capabilities"]["tools"].is_object());

        let reply = send(r#"{"jsonrpc":"2.0","id":2,"method":"initialize"}"#).unwrap();
        assert_eq!(reply["result"]["protocolVersion"], DEFAULT_PROTOCOL);
    }

    #[test]
    fn notifications_get_no_reply_and_ping_gets_one() {
        assert!(send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        let reply = send(r#"{"jsonrpc":"2.0","id":"x","method":"ping"}"#).unwrap();
        assert_eq!(reply["id"], "x");
        assert_eq!(reply["result"], json!({}));
    }

    #[test]
    fn lists_the_four_tools() {
        let reply = send(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#).unwrap();
        let names: Vec<&str> = reply["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["read_rules", "load_skill", "ask_owner", "file_ticket"]
        );
    }

    /// Calls the talk tool `name` with `args` for the project `shop`, and
    /// answers what it sent studiod and what the tool answered.
    fn talk_call(name: &str, args: Value, reply: Response) -> (Option<Request>, Value) {
        let sent = RefCell::new(None);
        let line = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": name, "arguments": args },
        })
        .to_string();
        let answer = handle_line(&line, &Mode::Talk("shop".into()), &|request| {
            *sent.borrow_mut() = Some(request.clone());
            Ok(reply.clone())
        })
        .unwrap();
        (sent.into_inner(), answer["result"].clone())
    }

    #[test]
    fn talk_mode_lists_the_leads_six_tools() {
        let talk = Mode::Talk("shop".into());
        let listed = call_in(&talk, "tools/list", "");
        let tools = listed["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "start_goal",
                "send_back",
                "pause",
                "resume",
                "cancel",
                "tell"
            ]
        );
        for tool in tools {
            assert!(tool["description"].as_str().unwrap().len() > 10);
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
        let required = |name: &str| -> Vec<String> {
            let tool = tools.iter().find(|t| t["name"] == name).unwrap();
            serde_json::from_value(tool["inputSchema"]["required"].clone()).unwrap()
        };
        assert_eq!(required("start_goal"), ["text"]);
        assert_eq!(required("send_back"), ["task", "note"]);
        assert_eq!(required("tell"), ["task", "text"]);
        assert_eq!(required("pause"), ["task"]);
        // A ticket's tools are not a lead's.
        assert_eq!(
            call_in(&talk, "tools/call", "file_ticket")["result"]["isError"],
            true
        );
    }

    #[test]
    fn start_goal_files_a_goal_the_lead_plans() {
        let started = Response::ok(json!({ "id": "SHOP-9", "goal": "Add a cart drawer" }));
        let (sent, result) = talk_call(
            "start_goal",
            json!({ "text": " Add a cart drawer ", "version": "0.5", "detail": "Slide in." }),
            started.clone(),
        );
        assert_eq!(
            sent,
            Some(Request::CreateGoal {
                project: "shop".into(),
                text: "Add a cart drawer".into(),
                hire: None,
                detail: "Slide in.".into(),
                version: Some("0.5".into()),
            })
        );
        assert_eq!(result["isError"], false);
        let answer: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(answer["goal"], "SHOP-9");
        assert_eq!(answer["title"], "Add a cart drawer");

        // The detail and the version are optional.
        let (sent, _) = talk_call("start_goal", json!({ "text": "Fix it" }), started);
        assert!(matches!(
            sent,
            Some(Request::CreateGoal { detail, version: None, .. }) if detail.is_empty()
        ));
    }

    #[test]
    fn the_other_tools_act_as_the_lead_of_the_project_on_a_task() {
        let done = Response::ok(json!({ "id": "SHOP-3" }));
        let acts = |act| Request::LeadActs {
            project: "shop".into(),
            task: "SHOP-3".into(),
            act,
        };
        let expect = [
            (
                "send_back",
                json!({ "task": "SHOP-3", "note": "Too many colours." }),
                acts(LeadAct::SendBack {
                    note: "Too many colours.".into(),
                }),
            ),
            ("pause", json!({ "task": "SHOP-3" }), acts(LeadAct::Pause)),
            ("resume", json!({ "task": "SHOP-3" }), acts(LeadAct::Resume)),
            ("cancel", json!({ "task": "SHOP-3" }), acts(LeadAct::Cancel)),
            (
                "tell",
                json!({ "task": "SHOP-3", "text": "Use three." }),
                acts(LeadAct::Tell {
                    text: "Use three.".into(),
                }),
            ),
        ];
        for (name, args, request) in expect {
            let (sent, result) = talk_call(name, args, done.clone());
            assert_eq!(sent, Some(request), "{name}");
            assert_eq!(result["isError"], false, "{name}");
        }
    }

    #[test]
    fn a_talk_tool_that_lacks_something_sends_nothing_and_says_what() {
        for (name, args, missing) in [
            ("start_goal", json!({}), "text"),
            ("start_goal", json!({ "text": "  " }), "text"),
            ("send_back", json!({ "task": "SHOP-3" }), "note"),
            ("pause", json!({}), "task"),
            ("tell", json!({ "task": "SHOP-3" }), "text"),
        ] {
            let (sent, result) = talk_call(name, args, Response::ok(json!({})));
            assert_eq!(sent, None, "{name}");
            assert_eq!(result["isError"], true, "{name}");
            let said = result["content"][0]["text"].as_str().unwrap();
            assert!(said.contains(missing), "{name}: {said}");
        }
    }

    #[test]
    fn a_talk_tool_that_studiod_refuses_is_an_error_with_its_words() {
        for name in ["start_goal", "cancel"] {
            let args = json!({ "text": "Go", "task": "SHOP-3" });
            let (_, result) = talk_call(name, args, Response::err("no ticket SHOP-3"));
            assert_eq!(result["isError"], true, "{name}");
            assert_eq!(result["content"][0]["text"], "no ticket SHOP-3");
        }
        let (_, result) = talk_call("nope", json!({}), Response::ok(json!({})));
        assert_eq!(result["isError"], true);
    }

    #[test]
    fn unknown_methods_and_bad_json_are_protocol_errors() {
        let reply = send(r#"{"jsonrpc":"2.0","id":7,"method":"resources/list"}"#).unwrap();
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["error"]["code"], -32601);
        let reply = send("{nope").unwrap();
        assert_eq!(reply["error"]["code"], -32700);
    }

    #[test]
    fn a_tool_call_forwards_the_ticket_and_returns_text() {
        let seen = RefCell::new(None);
        let reply = handle_line(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"file_ticket","arguments":{"title":"T","detail":"D"}}}"#,
            &ticket(),
            &|request| {
                *seen.borrow_mut() = Some(request.clone());
                Ok(Response::ok(json!({ "id": "GAME-2" })))
            },
        )
        .unwrap();
        assert_eq!(reply["result"]["isError"], false);
        assert_eq!(
            seen.into_inner(),
            Some(Request::FileTicket {
                ticket: "GAME-1".into(),
                title: "T".into(),
                detail: "D".into(),
                hire: None,
            })
        );
    }

    #[test]
    fn tool_errors_are_results_with_is_error() {
        let call = |line: &str, forward: Forward| handle_line(line, &ticket(), forward).unwrap();
        let missing = call(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ask_owner","arguments":{}}}"#,
            &|_| Ok(Response::ok("")),
        );
        assert_eq!(missing["result"]["isError"], true);
        assert!(missing.get("error").is_none());
        assert!(
            missing["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("question")
        );

        let refused = call(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"read_rules"}}"#,
            &|_| Ok(Response::err("no such ticket")),
        );
        assert_eq!(refused["result"]["isError"], true);
        assert_eq!(refused["result"]["content"][0]["text"], "no such ticket");

        let unknown = call(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"nope"}}"#,
            &|_| Ok(Response::ok("")),
        );
        assert_eq!(unknown["result"]["isError"], true);
    }

    #[test]
    fn setup_mode_offers_one_tool_to_hand_over_what_the_lead_found() {
        let setup = Mode::Setup("garden".into());
        let listed = call_in(&setup, "tools/list", "");
        let tools = listed["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "propose_setup");
        let schema = &tools[0]["inputSchema"];
        assert_eq!(schema["required"], json!(["summary", "verify"]));
        for field in [
            "summary",
            "verify",
            "capture",
            "release",
            "release_source",
            "protected",
            "daily_usd",
            "first_goal",
        ] {
            assert!(schema["properties"][field].is_object(), "{field}");
        }
        // A ticket's tools are not a lead's.
        for name in ["ask_owner", "file_ticket", "nope"] {
            assert_eq!(
                call_in(&setup, "tools/call", name)["result"]["isError"],
                true,
                "{name}"
            );
        }
    }

    #[test]
    fn proposing_a_setup_forwards_the_lead_s_findings_for_its_project() {
        let seen = RefCell::new(None);
        let args = json!({
            "summary": "A small game.",
            "verify": [{ "command": ["./test.sh"], "why": "It runs the tests." }],
            "capture": { "command": ["./shots.sh", "{out}"], "why": "It takes pictures." },
            "release": [["./publish.sh", "--now"]],
            "release_source": "your CI",
            "protected": [{ "glob": "migrations/**", "why": "Data." }],
            "daily_usd": 4.0,
            "first_goal": { "text": "Add a menu", "version": "0.2", "why": "It is missing." },
        });
        let line = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "propose_setup", "arguments": args },
        })
        .to_string();
        let reply = handle_line(&line, &Mode::Setup("garden".into()), &|request| {
            *seen.borrow_mut() = Some(request.clone());
            Ok(Response::ok("Thanks."))
        })
        .unwrap();
        assert_eq!(reply["result"]["isError"], false);
        assert_eq!(reply["result"]["content"][0]["text"], "Thanks.");
        let Some(Request::ProposeSetup { project, proposal }) = seen.into_inner() else {
            panic!("no proposal was sent");
        };
        assert_eq!(project, "garden");
        assert_eq!(proposal.summary, "A small game.");
        assert_eq!(proposal.verify[0].command, ["./test.sh"]);
        assert_eq!(proposal.verify[0].why, "It runs the tests.");
        assert_eq!(proposal.capture.unwrap().command, ["./shots.sh", "{out}"]);
        assert_eq!(proposal.release, [["./publish.sh", "--now"]]);
        assert_eq!(proposal.release_source, "your CI");
        assert_eq!(proposal.protected[0].glob, "migrations/**");
        assert_eq!(proposal.daily_usd, Some(4.0));
        assert_eq!(proposal.first_goal.unwrap().version.as_deref(), Some("0.2"));
    }

    #[test]
    fn a_proposal_the_studio_cannot_read_or_refuses_comes_back_as_a_tool_error() {
        let setup = Mode::Setup("garden".into());
        let call = |args: Value, forward: Forward| {
            let line = json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "propose_setup", "arguments": args },
            })
            .to_string();
            handle_line(&line, &setup, forward).unwrap()
        };
        // A command given as one string is not a list.
        let unreadable = call(
            json!({ "summary": "x", "verify": [{ "command": "make test", "why": "y" }] }),
            &|_| Err("nothing should be sent".into()),
        );
        assert_eq!(unreadable["result"]["isError"], true);
        let text = unreadable["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("list of strings"), "{text}");
        // What the studio refuses, the lead is told as it is.
        let refused = call(json!({ "summary": "x", "verify": [] }), &|_| {
            Ok(Response::err("a check has no program to run"))
        });
        assert_eq!(refused["result"]["isError"], true);
        assert_eq!(
            refused["result"]["content"][0]["text"],
            "a check has no program to run"
        );
    }
}
