use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn write_dap_message(stdin: &mut impl Write, payload: &Value) {
    let body = payload.to_string();
    write!(stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).expect("write dap message");
    stdin.flush().expect("flush dap message");
}

fn read_dap_message(reader: &mut impl BufRead) -> Option<Value> {
    let mut content_length: Option<usize> = None;

    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).ok()?;
        if bytes == 0 {
            return None;
        }

        if line == "\r\n" {
            break;
        }

        if line.to_ascii_lowercase().starts_with("content-length:") {
            let value = line.split(':').nth(1)?.trim().parse::<usize>().ok()?;
            content_length = Some(value);
        }
    }

    let len = content_length?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn wait_for_message(
    rx: &mpsc::Receiver<Value>,
    mut predicate: impl FnMut(&Value) -> bool,
    timeout: Duration,
) -> Value {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let remaining = timeout.saturating_sub(start.elapsed());
        match rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
            Ok(msg) => {
                if predicate(&msg) {
                    return msg;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    panic!("Timed out waiting for expected DAP message");
}

fn callable_program_path() -> PathBuf {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    repo_root.join("examples/callable_closure_test.kettu")
}

fn nested_program_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nested_closure_debug.kettu")
}

fn control_program_path() -> PathBuf {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    repo_root.join("examples/control_test.kettu")
}

#[test]
fn dap_step_in_enters_callable_closure() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kettu"))
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn kettu dap");

    let mut stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");

    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        while let Some(msg) = read_dap_message(&mut reader) {
            if tx.send(msg).is_err() {
                break;
            }
        }
    });

    let program = callable_program_path();
    let cwd = program
        .parent()
        .expect("program parent")
        .to_string_lossy()
        .to_string();

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 1,
            "command": "initialize",
            "arguments": {}
        }),
    );

    let _init_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("initialize"))
        },
        Duration::from_secs(2),
    );

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 2,
            "command": "launch",
            "arguments": {
                "program": program.to_string_lossy(),
                "cwd": cwd,
                "stopOnEntry": false
            }
        }),
    );

    let _launch_resp = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("response")) && m.get("command") == Some(&json!("launch")),
        Duration::from_secs(3),
    );

    let _initialized_event = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("initialized")),
        Duration::from_secs(2),
    );

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 3,
            "command": "setBreakpoints",
            "arguments": {
                "source": { "path": program.to_string_lossy() },
                "breakpoints": [{ "line": 22 }, { "line": 23 }]
            }
        }),
    );

    let _bp_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("setBreakpoints"))
        },
        Duration::from_secs(2),
    );

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 4,
            "command": "configurationDone",
            "arguments": {}
        }),
    );

    let stop_on_def = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );
    assert_eq!(
        stop_on_def.pointer("/body/reason"),
        Some(&json!("breakpoint"))
    );

    // Stack should show only the test frame at the definition line (not closure executing yet)
    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 5,
            "command": "stackTrace",
            "arguments": { "threadId": 1 }
        }),
    );
    let stack_def = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(
        stack_def.pointer("/body/stackFrames/0/name"),
        Some(&json!('@'.to_string() + "test test-no-captures"))
    );
    assert_eq!(
        stack_def.pointer("/body/stackFrames/0/line"),
        Some(&json!(22))
    );

    // Step once (line 22 -> 23)
    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 6, "command": "stepIn", "arguments": {"threadId": 1}}),
    );
    let _stop_call = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(2),
    );

    // Step into the closure call
    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 7, "command": "stepIn", "arguments": {"threadId": 1}}),
    );
    let _stop_closure = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(2),
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 8, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let stack_closure = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(
        stack_closure.pointer("/body/stackFrames/0/name"),
        Some(&json!("double"))
    );
    assert_eq!(
        stack_closure.pointer("/body/stackFrames/0/line"),
        Some(&json!(22))
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 99, "command": "disconnect", "arguments": {}}),
    );
    let _disc_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("disconnect"))
        },
        Duration::from_secs(2),
    );

    let status = child.wait().expect("wait for dap process");
    assert!(status.success());
}

#[test]
fn dap_exposes_captures_and_evaluate_for_callable_closure() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kettu"))
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn kettu dap");

    let mut stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");

    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        while let Some(msg) = read_dap_message(&mut reader) {
            if tx.send(msg).is_err() {
                break;
            }
        }
    });

    let program = callable_program_path();
    let cwd = program
        .parent()
        .expect("program parent")
        .to_string_lossy()
        .to_string();

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 1,
            "command": "initialize",
            "arguments": {}
        }),
    );
    let init_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("initialize"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(
        init_resp.pointer("/body/supportsEvaluateForHovers"),
        Some(&json!(true))
    );

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 2,
            "command": "launch",
            "arguments": {
                "program": program.to_string_lossy(),
                "cwd": cwd,
                "stopOnEntry": false,
                "enableEvaluate": true
            }
        }),
    );
    let _launch_resp = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("response")) && m.get("command") == Some(&json!("launch")),
        Duration::from_secs(3),
    );
    let _initialized_event = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("initialized")),
        Duration::from_secs(2),
    );

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 3,
            "command": "setBreakpoints",
            "arguments": {
                "source": { "path": program.to_string_lossy() },
                "breakpoints": [{ "line": 9 }]
            }
        }),
    );
    let _bp_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("setBreakpoints"))
        },
        Duration::from_secs(2),
    );

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 4,
            "command": "configurationDone",
            "arguments": {}
        }),
    );
    let stop_on_call = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );
    assert_eq!(
        stop_on_call.pointer("/body/reason"),
        Some(&json!("breakpoint"))
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 5, "command": "stepIn", "arguments": {"threadId": 1}}),
    );
    let _stop_closure = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(2),
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 6, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let stack_closure = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(
        stack_closure.pointer("/body/stackFrames/0/name"),
        Some(&json!("add-x"))
    );
    let frame_id = stack_closure
        .pointer("/body/stackFrames/0/id")
        .and_then(Value::as_i64)
        .expect("closure frame id");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 7, "command": "scopes", "arguments": {"frameId": frame_id}}),
    );
    let scopes_resp = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("response")) && m.get("command") == Some(&json!("scopes")),
        Duration::from_secs(2),
    );
    let scopes = scopes_resp
        .pointer("/body/scopes")
        .and_then(Value::as_array)
        .expect("scopes array");
    assert_eq!(scopes.len(), 2, "expected locals and captures scopes");
    let locals_ref = scopes[0]["variablesReference"]
        .as_i64()
        .expect("locals variablesReference");
    let captures_ref = scopes[1]["variablesReference"]
        .as_i64()
        .expect("captures variablesReference");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 8, "command": "variables", "arguments": {"variablesReference": locals_ref}}),
    );
    let locals_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let locals = locals_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("locals array");
    assert!(
        locals
            .iter()
            .any(|v| v["name"] == json!("n") && v["value"] == json!("5"))
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 9, "command": "variables", "arguments": {"variablesReference": captures_ref}}),
    );
    let captures_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let captures = captures_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("captures array");
    assert!(
        captures
            .iter()
            .any(|v| v["name"] == json!("x") && v["value"] == json!("10"))
    );

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 10,
            "command": "evaluate",
            "arguments": {
                "frameId": frame_id,
                "expression": "n + x",
                "context": "repl"
            }
        }),
    );
    let evaluate_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("evaluate"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(evaluate_resp.pointer("/body/result"), Some(&json!("15")));
    assert_eq!(evaluate_resp.pointer("/body/type"), Some(&json!("i64")));

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 99, "command": "disconnect", "arguments": {}}),
    );
    let _disc_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("disconnect"))
        },
        Duration::from_secs(2),
    );

    let status = child.wait().expect("wait for dap process");
    assert!(status.success());
}

#[test]
fn dap_follows_taken_if_branch_and_skips_unreachable_breakpoints() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kettu"))
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn kettu dap");

    let mut stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");

    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        while let Some(msg) = read_dap_message(&mut reader) {
            if tx.send(msg).is_err() {
                break;
            }
        }
    });

    let program = control_program_path();
    let cwd = program
        .parent()
        .expect("program parent")
        .to_string_lossy()
        .to_string();

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 1,
            "command": "initialize",
            "arguments": {}
        }),
    );
    let _init_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("initialize"))
        },
        Duration::from_secs(2),
    );

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 2,
            "command": "launch",
            "arguments": {
                "program": program.to_string_lossy(),
                "cwd": cwd,
                "stopOnEntry": false
            }
        }),
    );
    let _launch_resp = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("response")) && m.get("command") == Some(&json!("launch")),
        Duration::from_secs(3),
    );
    let _initialized_event = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("initialized")),
        Duration::from_secs(2),
    );

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 3,
            "command": "setBreakpoints",
            "arguments": {
                "source": { "path": program.to_string_lossy() },
                "breakpoints": [{ "line": 28 }, { "line": 33 }]
            }
        }),
    );
    let _bp_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("setBreakpoints"))
        },
        Duration::from_secs(2),
    );

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 4,
            "command": "configurationDone",
            "arguments": {}
        }),
    );
    let stop_on_entry_bp = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );
    assert_eq!(
        stop_on_entry_bp.pointer("/body/reason"),
        Some(&json!("breakpoint"))
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 5, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let stack_at_28 = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(
        stack_at_28.pointer("/body/stackFrames/0/line"),
        Some(&json!(28))
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 6, "command": "stepIn", "arguments": {"threadId": 1}}),
    );
    let _stop_29 = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(2),
    );
    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 7, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let stack_at_29 = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(
        stack_at_29.pointer("/body/stackFrames/0/line"),
        Some(&json!(29))
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 8, "command": "stepIn", "arguments": {"threadId": 1}}),
    );
    let _stop_30 = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(2),
    );
    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 9, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let stack_at_30 = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(
        stack_at_30.pointer("/body/stackFrames/0/line"),
        Some(&json!(30))
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 10, "command": "stepIn", "arguments": {"threadId": 1}}),
    );
    let stop_at_taken_branch = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(2),
    );
    assert_eq!(
        stop_at_taken_branch.pointer("/body/reason"),
        Some(&json!("step"))
    );
    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 11, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let stack_at_31 = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(
        stack_at_31.pointer("/body/stackFrames/0/line"),
        Some(&json!(31))
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 12, "command": "continue", "arguments": {"threadId": 1}}),
    );
    let terminated = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("terminated")),
        Duration::from_secs(4),
    );
    assert_eq!(terminated.get("event"), Some(&json!("terminated")));

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 99, "command": "disconnect", "arguments": {}}),
    );
    let _disc_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("disconnect"))
        },
        Duration::from_secs(2),
    );

    let status = child.wait().expect("wait for dap process");
    assert!(status.success());
}

#[test]
fn dap_tracks_nested_closure_breakpoints_and_frames() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kettu"))
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn kettu dap");

    let mut stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");

    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        while let Some(msg) = read_dap_message(&mut reader) {
            if tx.send(msg).is_err() {
                break;
            }
        }
    });

    let program = nested_program_path();
    let cwd = program
        .parent()
        .expect("program parent")
        .to_string_lossy()
        .to_string();

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 1,
            "command": "initialize",
            "arguments": {}
        }),
    );
    let _init_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("initialize"))
        },
        Duration::from_secs(2),
    );

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 2,
            "command": "launch",
            "arguments": {
                "program": program.to_string_lossy(),
                "cwd": cwd,
                "stopOnEntry": false
            }
        }),
    );
    let _launch_resp = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("response")) && m.get("command") == Some(&json!("launch")),
        Duration::from_secs(3),
    );
    let _initialized_event = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("initialized")),
        Duration::from_secs(2),
    );

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 3,
            "command": "setBreakpoints",
            "arguments": {
                "source": { "path": program.to_string_lossy() },
                "breakpoints": [{ "line": 9 }, { "line": 10 }]
            }
        }),
    );
    let _bp_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("setBreakpoints"))
        },
        Duration::from_secs(2),
    );

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 4,
            "command": "configurationDone",
            "arguments": {}
        }),
    );
    let stop_on_call = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );
    assert_eq!(
        stop_on_call.pointer("/body/reason"),
        Some(&json!("breakpoint"))
    );
    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 50, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let call_stack = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(
        call_stack.pointer("/body/stackFrames/0/line"),
        Some(&json!(9))
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 5, "command": "stepIn", "arguments": {"threadId": 1}}),
    );
    let _stop_outer = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(2),
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 6, "command": "next", "arguments": {"threadId": 1}}),
    );
    let stop_outer_setup = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(2),
    );
    assert_eq!(
        stop_outer_setup.pointer("/body/reason"),
        Some(&json!("step"))
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 7, "command": "next", "arguments": {"threadId": 1}}),
    );
    let stop_outer_reduce = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(2),
    );
    assert_eq!(
        stop_outer_reduce.pointer("/body/reason"),
        Some(&json!("step"))
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 8, "command": "stepIn", "arguments": {"threadId": 1}}),
    );
    let stop_inner = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(2),
    );
    assert_eq!(stop_inner.pointer("/body/reason"), Some(&json!("step")));

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 9, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let nested_stack = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    let nested_frames = nested_stack
        .pointer("/body/stackFrames")
        .and_then(Value::as_array)
        .expect("nested frames");
    assert_eq!(nested_frames[0]["name"], json!("closure#2"));
    assert_eq!(nested_frames[1]["name"], json!("outer"));
    assert_eq!(
        nested_frames[2]["name"],
        json!('@'.to_string() + "test nested-closure-flow")
    );
    assert_eq!(nested_frames[0]["line"], json!(8));

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 10, "command": "continue", "arguments": {"threadId": 1}}),
    );
    let stop_return = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(2),
    );
    assert_eq!(
        stop_return.pointer("/body/reason"),
        Some(&json!("breakpoint"))
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 11, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let return_stack = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    let return_frames = return_stack
        .pointer("/body/stackFrames")
        .and_then(Value::as_array)
        .expect("return frames");
    assert_eq!(
        return_frames[0]["name"],
        json!('@'.to_string() + "test nested-closure-flow")
    );
    assert_eq!(return_frames[0]["line"], json!(10));

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 99, "command": "disconnect", "arguments": {}}),
    );
    let _disc_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("disconnect"))
        },
        Duration::from_secs(2),
    );

    let status = child.wait().expect("wait for dap process");
    assert!(status.success());
}
