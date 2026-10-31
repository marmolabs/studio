//! The studio-tools MCP server: newline-delimited JSON-RPC 2.0 on stdio. It
//! holds no state but the ticket id; every tool is a request to studiod.

use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::Path;
use studiod::api::{Request, Response, call};

const DEFAULT_PROTOCOL: &str = "2025-06-18";

/// Sends a request to studiod and returns the tool's text, or why it failed.
type Forward<'a> = &'a dyn Fn(&Request) -> Result<Response, String>;

pub fn serve(socket: &Path, ticket: &str) -> Result<(), String> {
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
        if let Some(reply) = handle_line(&line, ticket, &forward) {
            writeln!(stdout, "{reply}")
                .and_then(|()| stdout.flush())
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// The reply to one request line; none for notifications.
fn handle_line(line: &str, ticket: &str, forward: Forward) -> Option<Value> {
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
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            match call_tool(name, &params["arguments"], ticket, forward) {
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

fn tools() -> Value {
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

fn call_tool(name: &str, args: &Value, ticket: &str, forward: Forward) -> Result<String, String> {
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
    let response = forward(&request)?;
    if !response.ok {
        return Err(response.error.unwrap_or_else(|| "request failed".into()));
    }
    Ok(match response.data {
        Value::String(text) => text,
        other => other.to_string(),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use std::cell::RefCell;

    fn send(line: &str) -> Option<Value> {
        handle_line(line, "GAME-1", &|_| Ok(Response::ok("rules text")))
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
            "GAME-1",
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
        let call = |line: &str, forward: Forward| handle_line(line, "GAME-1", forward).unwrap();
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
}
