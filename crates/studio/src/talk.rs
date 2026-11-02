//! `studio talk <project> [text]`: the conversation with a project's lead.
//! Without text it shows the last messages; with text it sends it, waits for
//! the lead's reply and shows it.

use std::path::Path;
use std::time::{Duration, Instant};
use studiod::api::{ConversationView, Request, Response, TalkCard, TalkFrom, TalkMessage, call};

/// How many of the last messages are shown when nothing is sent.
const LAST: usize = 10;

/// How often the conversation is looked at while a reply is awaited.
const POLL: Duration = Duration::from_millis(500);

/// How long a reply is awaited: a lead's turn is stopped after ten minutes.
const WAIT: Duration = Duration::from_secs(11 * 60);

/// Shows the conversation of `project`, or sends `text` and shows the reply.
pub fn run(
    socket: &Path,
    project: &str,
    text: &str,
    json: bool,
    now_ms: u64,
) -> Result<(), String> {
    let text = text.trim();
    let shown = if text.is_empty() {
        let view = conversation(socket, project, None)?;
        let from = view.messages.len().saturating_sub(LAST);
        view.messages[from..].to_vec()
    } else {
        if !json {
            eprintln!("waiting for {project}'s lead to answer...");
        }
        send(socket, project, text, POLL, WAIT)?
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&shown).map_err(|e| e.to_string())?
        );
    } else if shown.is_empty() {
        println!(
            "nothing has been said in {project} yet; say something with `studio talk {project} \"...\"`"
        );
    } else {
        for line in lines(&shown, now_ms) {
            println!("{line}");
        }
    }
    Ok(())
}

/// Sends `text` to the lead of `project`, waits until the lead is done with
/// it, and answers what was said after it: the cards of what the lead did and
/// its reply. A lead that is not done after `wait` is an error.
fn send(
    socket: &Path,
    project: &str,
    text: &str,
    poll: Duration,
    wait: Duration,
) -> Result<Vec<TalkMessage>, String> {
    let stored: TalkMessage = serde_json::from_value(fetch(
        socket,
        &Request::Talk {
            project: project.to_string(),
            text: text.to_string(),
        },
    )?)
    .map_err(|e| format!("studiod's reply isn't a message: {e}"))?;
    let start = Instant::now();
    loop {
        let view = conversation(socket, project, Some(stored.id))?;
        if !view.thinking {
            return Ok(view.messages);
        }
        if start.elapsed() > wait {
            return Err(format!(
                "no answer yet; `studio talk {project}` shows it when it comes"
            ));
        }
        std::thread::sleep(poll);
    }
}

/// The messages of `project`'s conversation after `after`, and whether its
/// lead is working on a reply.
fn conversation(
    socket: &Path,
    project: &str,
    after: Option<u64>,
) -> Result<ConversationView, String> {
    let data = fetch(
        socket,
        &Request::Conversation {
            project: project.to_string(),
            after,
        },
    )?;
    serde_json::from_value(data).map_err(|e| format!("studiod's reply isn't a conversation: {e}"))
}

fn fetch(socket: &Path, request: &Request) -> Result<serde_json::Value, String> {
    let Response { ok, data, error } = call(socket, request)
        .map_err(|e| format!("can't reach studiod at {}: {e}", socket.display()))?;
    if ok {
        Ok(data)
    } else {
        Err(error.unwrap_or_else(|| "request failed".into()))
    }
}

/// The lines to print for `messages`, oldest first: who said it and when, its
/// words, and what its card shows.
fn lines(messages: &[TalkMessage], now_ms: u64) -> Vec<String> {
    let mut out = Vec::new();
    for message in messages {
        let who = match &message.from {
            TalkFrom::Owner => "you",
            TalkFrom::Lead { hire } => hire,
            TalkFrom::Studio => "studio",
        };
        out.push(format!(
            "{who} ({}): {}",
            ago(now_ms.saturating_sub(message.at)),
            message.text
        ));
        match &message.card {
            Some(TalkCard::Plan {
                title,
                version,
                tasks,
                ..
            }) => {
                let version = version.as_deref().map(|v| format!(" for {v}"));
                out.push(format!("  plan: {title}{}", version.unwrap_or_default()));
                for task in tasks {
                    match &task.hire {
                        Some(hire) => out.push(format!("    - {} ({hire})", task.title)),
                        None => out.push(format!("    - {}", task.title)),
                    }
                }
            }
            Some(TalkCard::Acted {
                task,
                title,
                action,
                note,
            }) => {
                let note = if note.is_empty() {
                    String::new()
                } else {
                    format!(": {note}")
                };
                out.push(format!("  {action}: {title} ({task}){note}"));
            }
            Some(TalkCard::Setup { project }) => {
                out.push(format!("  how {project} runs: see it in the web UI"));
            }
            None => {}
        }
    }
    out
}

/// How long ago, in a few words.
fn ago(ms: u64) -> String {
    match ms / 1000 {
        0..=59 => "just now".to_string(),
        s @ 60..=3599 => format!("{}m ago", s / 60),
        s @ 3600..=86399 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    use serde_json::{Value, json};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use studiod::api::PlanTask;

    fn message(id: u64, from: TalkFrom, text: &str, at: u64) -> TalkMessage {
        TalkMessage {
            id,
            at,
            from,
            text: text.into(),
            card: None,
        }
    }

    fn lead() -> TalkFrom {
        TalkFrom::Lead {
            hire: "nora".into(),
        }
    }

    /// A studiod on a socket of its own that answers requests with `replies`,
    /// one for each, and hands the requests it got to the test.
    fn fake_studiod(
        label: &str,
        replies: Vec<Response>,
    ) -> (PathBuf, std::thread::JoinHandle<Vec<Value>>) {
        let socket =
            std::env::temp_dir().join(format!("studio-talk-{}-{label}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for reply in replies {
                let (stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                let mut writer = &stream;
                writeln!(writer, "{}", serde_json::to_string(&reply).unwrap()).unwrap();
                seen.push(serde_json::from_str(&line).unwrap());
            }
            seen
        });
        (socket, handle)
    }

    fn view(messages: Vec<TalkMessage>, thinking: bool) -> Response {
        Response::ok(ConversationView { messages, thinking })
    }

    #[test]
    fn the_command_parses_with_and_without_text() {
        use clap::Parser;
        let parse = |args: &[&str]| {
            crate::Cli::try_parse_from(["studio", "talk"].into_iter().chain(args.iter().copied()))
                .map(|cli| cli.command)
        };
        let talk = |args: &[&str]| match parse(args).unwrap() {
            crate::Command::Talk { project, text } => (project, text),
            _ => panic!("not the talk command"),
        };
        assert_eq!(talk(&["shop"]), ("shop".to_string(), vec![]));
        assert_eq!(
            talk(&["shop", "How's it going?"]),
            ("shop".to_string(), vec!["How's it going?".to_string()])
        );
        assert_eq!(
            talk(&["shop", "plan", "a", "goal"]).1.join(" "),
            "plan a goal"
        );
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn without_text_it_asks_for_the_conversation() {
        let said = vec![
            message(1, TalkFrom::Owner, "Hi", 0),
            message(2, lead(), "Hello", 0),
        ];
        let (socket, studiod) = fake_studiod("view", vec![view(said.clone(), false)]);
        let shown = conversation(&socket, "shop", None).unwrap();
        assert_eq!(shown.messages, said);
        assert_eq!(
            studiod.join().unwrap(),
            [json!({"cmd": "conversation", "project": "shop", "after": null})]
        );
    }

    #[test]
    fn with_text_it_sends_it_and_waits_until_the_lead_is_done() {
        let owner = message(4, TalkFrom::Owner, "How's it going?", 0);
        let reply = message(5, lead(), "Well.", 0);
        let (socket, studiod) = fake_studiod(
            "send",
            vec![
                Response::ok(&owner),
                view(vec![], true),
                view(vec![], true),
                view(vec![reply.clone()], false),
            ],
        );
        let got = send(
            &socket,
            "shop",
            "How's it going?",
            Duration::from_millis(5),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(got, vec![reply]);
        let sent = studiod.join().unwrap();
        assert_eq!(
            sent[0],
            json!({"cmd": "talk", "project": "shop", "text": "How's it going?"})
        );
        // It only asks for what came after its own message.
        for request in &sent[1..] {
            assert_eq!(
                *request,
                json!({"cmd": "conversation", "project": "shop", "after": 4})
            );
        }
    }

    #[test]
    fn a_lead_that_never_answers_is_an_error_and_a_refusal_is_shown_as_it_is() {
        let owner = message(1, TalkFrom::Owner, "Hi", 0);
        let (socket, studiod) = fake_studiod(
            "slow",
            std::iter::once(Response::ok(&owner))
                .chain(std::iter::repeat_with(|| view(vec![], true)).take(50))
                .collect(),
        );
        let error = send(
            &socket,
            "shop",
            "Hi",
            Duration::from_millis(5),
            Duration::from_millis(30),
        )
        .unwrap_err();
        assert!(error.contains("no answer yet"), "{error}");
        drop(studiod);

        let (socket, studiod) = fake_studiod(
            "refused",
            vec![Response::err(
                "shop has no lead yet. Give it one in its conversation in the web UI, or with \
                 `studio lead shop <hire>`.",
            )],
        );
        let error = send(&socket, "shop", "Hi", POLL, WAIT).unwrap_err();
        assert_eq!(
            error,
            "shop has no lead yet. Give it one in its conversation in the web UI, or with \
             `studio lead shop <hire>`."
        );
        studiod.join().unwrap();
    }

    #[test]
    fn messages_and_their_cards_read_as_lines() {
        let mut plan = message(3, lead(), "Here is the plan.", 0);
        plan.card = Some(TalkCard::Plan {
            goal: "SHOP-1".into(),
            title: "Add a cart drawer".into(),
            version: Some("0.5".into()),
            tasks: vec![
                PlanTask {
                    title: "Draw it".into(),
                    hire: Some("dev".into()),
                },
                PlanTask {
                    title: "Wire it".into(),
                    hire: None,
                },
            ],
        });
        let mut acted = message(4, TalkFrom::Studio, "Sent it back.", 0);
        acted.card = Some(TalkCard::Acted {
            task: "SHOP-2".into(),
            title: "Menu art".into(),
            action: "sent back".into(),
            note: "Too many colours.".into(),
        });
        let said = [
            message(1, TalkFrom::Owner, "Plan it", 0),
            message(2, lead(), "On it.", 6_600_000),
            plan,
            acted,
        ];
        let text = lines(&said, 3_600_000 * 2);
        assert_eq!(
            text,
            [
                "you (2h ago): Plan it",
                "nora (10m ago): On it.",
                "nora (2h ago): Here is the plan.",
                "  plan: Add a cart drawer for 0.5",
                "    - Draw it (dev)",
                "    - Wire it",
                "studio (2h ago): Sent it back.",
                "  sent back: Menu art (SHOP-2): Too many colours.",
            ]
        );
        assert_eq!(ago(5_000), "just now");
        assert_eq!(ago(86_400_000 * 3), "3d ago");
    }
}
