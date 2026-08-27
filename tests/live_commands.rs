use std::path::{Path, PathBuf};
use std::process::Command;

fn run_with_root(args: &[&str], history_root: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_codex-history"))
        .args(args)
        .env("CODEX_HISTORY_HOME", history_root)
        .output()
        .expect("binary should run")
}

fn sample_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/local_history/sample_root")
}

fn response_item_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/local_history/response_item_root")
}

fn cross_shard_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/local_history/cross_shard_root")
}

#[test]
fn live_outputs_conversation_once_for_simple_thread() {
    let output = run_with_root(&["live", "thr_simple", "--once"], &sample_root());
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[User] (turn 1)"));
    assert!(stdout.contains("Please inspect the parser regression."));
    assert!(stdout.contains("[Assistant] (turn 1)"));
    assert!(stdout.contains("I found the leftover argv issue."));
    // Default does not show internal steps
    assert!(!stdout.contains("[Step]"));
}

#[test]
fn live_includes_steps_when_flag_provided() {
    let output = run_with_root(
        &["live", "thr_simple", "--once", "--include-steps"],
        &sample_root(),
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[User] (turn 1)"));
    assert!(stdout.contains("[Step] (turn 2)"));
    assert!(stdout.contains("command: cargo test cli::tests"));
    assert!(stdout.contains("progress: I found the leftover argv issue."));
}

#[test]
fn live_outputs_completion_message_from_cross_shard_session() {
    let output = run_with_root(&["live", "thr_cross_shard", "--once"], &cross_shard_root());
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Run the parser tests."));
    assert!(stdout.contains("[Assistant] (turn 1)"));
    assert!(stdout.contains("Parser tests passed."));
}

#[test]
fn live_honors_tail_parameter() {
    let output = run_with_root(
        &["live", "thr_simple", "--once", "--tail", "1"],
        &sample_root(),
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Turn 1 should be omitted
    assert!(!stdout.contains("Please inspect the parser regression."));
    // Last turn should be included
    assert!(stdout.contains("Run the shell tool against the help output."));
}

#[test]
fn live_exit_on_complete_exits_for_completed_session() {
    let output = run_with_root(
        &["live", "thr_simple", "--exit-on-complete"],
        &sample_root(),
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Please inspect the parser regression."));
}

#[test]
fn live_returns_error_for_nonexistent_thread() {
    let output = run_with_root(&["live", "nonexistent_thread_id", "--once"], &sample_root());
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("thread not found: nonexistent_thread_id"));
}

#[test]
fn live_rejects_json_output_mode() {
    for flag in ["--json", "--ndjson"] {
        let output = run_with_root(&[flag, "live", "thr_simple", "--once"], &sample_root());
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(&format!("cannot combine {flag} with live")));
    }
}

#[test]
fn live_filters_developer_preambles_in_response_item_session() {
    let output = run_with_root(
        &["live", "thr_response_item", "--once"],
        &response_item_root(),
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[User] (turn 1)"));
    assert!(stdout.contains("Summarize the parser edge cases."));
    assert!(stdout.contains("[Assistant] (turn 1)"));
    assert!(stdout.contains("I found two parser regressions."));
    // Developer instruction preamble must be filtered out
    assert!(!stdout.contains("Repository policy goes here."));
}
