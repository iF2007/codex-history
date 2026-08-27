use std::collections::HashMap;
use std::io::{self, Write};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::backend::local::LocalBackend;
use crate::model::{Item, MessageItem, ThreadDetail, Turn};
use crate::redact::redact_human_text;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveOptions {
    pub thread_id: String,
    pub interval_secs: u64,
    pub tail: Option<usize>,
    pub exit_on_complete: bool,
    pub include_steps: bool,
    pub once: bool,
}

#[derive(Default)]
struct TurnProgress {
    user_displayed: bool,
    steps_displayed: usize,
    agent_displayed: bool,
}

pub fn run_live(backend: &LocalBackend, options: &LiveOptions) -> Result<(), String> {
    let files = backend.find_thread_files(&options.thread_id)?;
    let mut progress: HashMap<String, TurnProgress> = HashMap::new();
    let mut initialized = false;

    loop {
        let detail = backend
            .parse_thread_files(&files)?
            .ok_or_else(|| format!("thread not found: {}", options.thread_id))?;

        if !initialized {
            initialized = true;
            initialize_tail(&detail, options.tail, &mut progress);
        }

        render_live_updates(&detail, options.include_steps, &mut progress)?;

        if options.once {
            break;
        }

        if options.exit_on_complete && is_session_complete(&detail, &progress) {
            break;
        }

        thread::sleep(Duration::from_secs(options.interval_secs));
    }

    Ok(())
}

fn initialize_tail(
    detail: &ThreadDetail,
    tail: Option<usize>,
    progress: &mut HashMap<String, TurnProgress>,
) {
    let Some(n) = tail else {
        return;
    };
    let completed_turns = detail
        .turns
        .iter()
        .filter(|turn| turn.status == "completed")
        .count();
    let completed_to_skip = completed_turns.saturating_sub(n);
    let mut skipped = 0;

    for turn in &detail.turns {
        if turn.status != "completed" || skipped >= completed_to_skip {
            continue;
        }

        progress.insert(
            turn.turn_id.clone(),
            TurnProgress {
                user_displayed: true,
                steps_displayed: usize::MAX,
                agent_displayed: true,
            },
        );
        skipped += 1;
    }
}

fn render_live_updates(
    detail: &ThreadDetail,
    include_steps: bool,
    progress: &mut HashMap<String, TurnProgress>,
) -> Result<(), String> {
    let mut wrote_anything = false;

    for (index, turn) in detail.turns.iter().enumerate() {
        let turn_num = index + 1;
        let entry = progress.entry(turn.turn_id.clone()).or_default();

        if !entry.user_displayed {
            if let Some(user_msg) = extract_user_message(turn) {
                println!("[User] (turn {turn_num})");
                println!("{}", redact_human_text(&user_msg));
                println!();
                entry.user_displayed = true;
                wrote_anything = true;
            }
        }

        if include_steps {
            let steps = extract_turn_steps(turn);
            if steps.len() > entry.steps_displayed {
                for step in &steps[entry.steps_displayed..] {
                    println!("[Step] (turn {turn_num}) {}", redact_human_text(step));
                    wrote_anything = true;
                }
                println!();
                entry.steps_displayed = steps.len();
            }
        }

        if !entry.agent_displayed {
            if let Some(agent_msg) = extract_final_agent_message(turn) {
                println!("[Assistant] (turn {turn_num})");
                println!("{}", redact_human_text(&agent_msg));
                println!();
                entry.agent_displayed = true;
                wrote_anything = true;
            }
        }
    }

    if wrote_anything {
        let _ = io::stdout().flush();
    }

    Ok(())
}

fn is_session_complete(detail: &ThreadDetail, progress: &HashMap<String, TurnProgress>) -> bool {
    let status = detail.summary.status.as_deref().unwrap_or("in_progress");
    let is_thread_done = matches!(status, "completed" | "failed" | "aborted");
    if !is_thread_done {
        return false;
    }

    for turn in &detail.turns {
        if let Some(p) = progress.get(&turn.turn_id) {
            if !p.user_displayed && extract_user_message(turn).is_some() {
                return false;
            }
            if !p.agent_displayed && extract_final_agent_message(turn).is_some() {
                return false;
            }
        } else {
            return false;
        }
    }

    true
}

fn is_instruction_preamble(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("# AGENTS.md instructions")
        || trimmed.starts_with("<environment_context>")
        || trimmed.starts_with("<planning_context>")
        || trimmed.starts_with("<skill>")
        || trimmed.starts_with("<skills_instructions>")
        || trimmed.starts_with("<subagent_notification>")
        || trimmed.starts_with("<turn_aborted>")
}

fn extract_user_message(turn: &Turn) -> Option<String> {
    for item in &turn.items {
        if let Item::UserMessage(msg) = item {
            if msg.attributes.get("role").and_then(Value::as_str) == Some("developer") {
                continue;
            }
            if let Some(text) = msg.text.as_deref() {
                let trimmed = text.trim();
                if trimmed.is_empty() || is_instruction_preamble(trimmed) {
                    continue;
                }
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn extract_final_agent_message(turn: &Turn) -> Option<String> {
    if let Some(text) = turn.final_agent_message.as_deref() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    // Check for explicit final_answer phase
    for item in &turn.items {
        if let Item::AgentMessage(msg) = item {
            if message_phase(msg) == Some("final_answer") {
                if let Some(text) = msg.text.as_deref() {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    }
                }
            }
        }
    }

    // If the turn is completed, find the last agent message.
    if turn.status == "completed" {
        for item in turn.items.iter().rev() {
            if let Item::AgentMessage(msg) = item {
                if let Some(text) = msg.text.as_deref() {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    }
                }
            }
        }
    }

    None
}

fn extract_turn_steps(turn: &Turn) -> Vec<String> {
    let mut steps = Vec::new();
    for item in &turn.items {
        match item {
            Item::CommandExecution(cmd) => {
                let text = cmd.command.as_deref().unwrap_or("(command)");
                steps.push(format!("command: {text}"));
            }
            Item::McpToolCall(tool) => {
                let text = tool.tool.as_deref().unwrap_or("(tool)");
                steps.push(format!("tool: {text}"));
            }
            Item::WebSearch(search) => {
                let text = search.query.as_deref().unwrap_or("(web search)");
                steps.push(format!("web search: {text}"));
            }
            Item::FileChange(fc) => {
                let path = fc
                    .path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .or_else(|| fc.summary.clone())
                    .unwrap_or_else(|| "(file change)".into());
                steps.push(format!("file: {path}"));
            }
            Item::ReasoningSummary(rs) => {
                if let Some(text) = rs.text.as_deref() {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() && trimmed != "(summary)" {
                        let preview = truncate_chars(trimmed, 100);
                        steps.push(format!("reasoning: {preview}"));
                    }
                }
            }
            Item::AgentMessage(msg) if message_phase(msg) == Some("commentary") => {
                if let Some(text) = msg.text.as_deref() {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        let preview = truncate_chars(trimmed, 100);
                        steps.push(format!("progress: {preview}"));
                    }
                }
            }
            Item::Other(other) if other.kind == "custom_tool_call" => {
                let name = other
                    .data
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("(tool)");
                steps.push(format!("tool: {name}"));
            }
            Item::Other(other) if other.kind == "web_search_call" => {
                steps.push("tool: web_search".into());
            }
            _ => {}
        }
    }
    steps
}

fn message_phase(message: &MessageItem) -> Option<&str> {
    message
        .phase
        .as_deref()
        .or_else(|| message.attributes.get("phase").and_then(Value::as_str))
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count > max_chars {
        let truncated: String = text.chars().take(max_chars).collect();
        format!("{truncated}...")
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{TimeZone, Utc};
    use serde_json::Value;

    use super::*;
    use crate::model::{ThreadSummary, UnknownItem};

    fn thread_detail(turns: Vec<Turn>) -> ThreadDetail {
        ThreadDetail {
            summary: ThreadSummary {
                thread_id: "thread".into(),
                name: None,
                preview: None,
                created_at: Utc.timestamp_opt(0, 0).single().expect("timestamp"),
                updated_at: None,
                cwd: None,
                source_kind: None,
                model_provider: None,
                ephemeral: None,
                status: Some("running".into()),
            },
            turns,
            items_count: 0,
            commands_count: 0,
            files_changed_count: 0,
        }
    }

    fn turn(turn_id: &str, status: &str, items: Vec<Item>) -> Turn {
        Turn {
            turn_id: turn_id.into(),
            status: status.into(),
            started_at: None,
            completed_at: None,
            items,
            final_agent_message: None,
        }
    }

    #[test]
    fn tail_skips_old_completed_turns_but_keeps_active_turn() {
        let detail = thread_detail(vec![
            turn("turn_1", "completed", Vec::new()),
            turn("turn_2", "completed", Vec::new()),
            turn("turn_3", "in_progress", Vec::new()),
        ]);
        let mut progress = HashMap::new();

        initialize_tail(&detail, Some(1), &mut progress);

        assert!(progress.contains_key("turn_1"));
        assert!(!progress.contains_key("turn_2"));
        assert!(!progress.contains_key("turn_3"));
    }

    #[test]
    fn includes_custom_tool_and_web_search_steps() {
        let detail = thread_detail(vec![turn(
            "turn_1",
            "in_progress",
            vec![
                Item::Other(UnknownItem {
                    kind: "custom_tool_call".into(),
                    data: BTreeMap::from([("name".into(), Value::String("exec".into()))]),
                }),
                Item::Other(UnknownItem {
                    kind: "web_search_call".into(),
                    data: BTreeMap::new(),
                }),
            ],
        )]);

        assert_eq!(
            extract_turn_steps(&detail.turns[0]),
            ["tool: exec", "tool: web_search"]
        );
    }
}
