use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tempfile::tempdir;

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

fn same_line_closure_program_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/same_line_closure_debug.kettu")
}

fn control_program_path() -> PathBuf {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    repo_root.join("examples/control_test.kettu")
}

fn while_program_path() -> PathBuf {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    repo_root.join("examples/while_test.kettu")
}

fn hof_program_path() -> PathBuf {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    repo_root.join("examples/hof_test.kettu")
}

fn variant_program_path() -> PathBuf {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    repo_root.join("examples/variant_test.kettu")
}

fn list_program_path() -> PathBuf {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    repo_root.join("examples/list_test.kettu")
}

fn breakpoint_disambiguation_columns() -> [(i64, &'static str); 2] {
    [(31, "closure#1"), (65, "closure#2")]
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
    assert_eq!(
        _init_resp.pointer("/body/supportsColumnBreakpoints"),
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
                "breakpoints": [{ "line": 22 }, { "line": 22, "column": 22 }, { "line": 23 }]
            }
        }),
    );

    let bp_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("setBreakpoints"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(
        bp_resp.pointer("/body/breakpoints/1/column"),
        Some(&json!(22))
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
    assert_eq!(
        stack_closure.pointer("/body/stackFrames/0/column"),
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
fn dap_step_in_enters_inline_hof_closure() {
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

    let program = hof_program_path();
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
                "breakpoints": [{ "line": 22 }]
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
    let _stop_on_line = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 5, "command": "stepIn", "arguments": {"threadId": 1}}),
    );
    let _stop_in_closure = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(2),
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 6, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let stack = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    let frame_name = stack
        .pointer("/body/stackFrames/0/name")
        .and_then(Value::as_str)
        .expect("closure frame name");
    assert!(frame_name.starts_with("closure#"), "unexpected frame name: {frame_name}");
    assert_eq!(stack.pointer("/body/stackFrames/0/line"), Some(&json!(22)));
    assert_eq!(stack.pointer("/body/stackFrames/0/column"), Some(&json!(31)));
    assert_eq!(
        stack.pointer("/body/stackFrames/1/name"),
        Some(&json!('@'.to_string() + "test test-filter-big"))
    );

    let frame_id = stack
        .pointer("/body/stackFrames/0/id")
        .and_then(Value::as_i64)
        .expect("closure frame id");
    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 7, "command": "scopes", "arguments": {"frameId": frame_id}}),
    );
    let scopes = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("response")) && m.get("command") == Some(&json!("scopes")),
        Duration::from_secs(2),
    );
    let locals_ref = scopes
        .pointer("/body/scopes/0/variablesReference")
        .and_then(Value::as_i64)
        .expect("locals variablesReference");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 8, "command": "variables", "arguments": {"variablesReference": locals_ref}}),
    );
    let vars_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let vars = vars_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("closure locals");
    let x = vars
        .iter()
        .find(|value| value["name"] == json!("x"))
        .expect("closure param x");
    assert_eq!(x["value"], json!("1"));

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
fn dap_repeated_step_in_on_inline_hof_closure_makes_visible_progress() {
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

    let program = hof_program_path();
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
                "breakpoints": [{ "line": 22 }]
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
    let _stop_on_line = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );

    let mut observed_states = Vec::new();
    for seq in 5..=10 {
        write_dap_message(
            &mut stdin,
            &json!({"type": "request", "seq": seq, "command": "stepIn", "arguments": {"threadId": 1}}),
        );
        let _stop = wait_for_message(
            &rx,
            |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
            Duration::from_secs(2),
        );

        let stack_seq = 100 + seq;
        write_dap_message(
            &mut stdin,
            &json!({"type": "request", "seq": stack_seq, "command": "stackTrace", "arguments": {"threadId": 1}}),
        );
        let stack = wait_for_message(
            &rx,
            |m| {
                m.get("type") == Some(&json!("response"))
                    && m.get("command") == Some(&json!("stackTrace"))
            },
            Duration::from_secs(2),
        );

        let frame_name = stack
            .pointer("/body/stackFrames/0/name")
            .and_then(Value::as_str)
            .expect("top frame name")
            .to_string();
        let frame_id = stack
            .pointer("/body/stackFrames/0/id")
            .and_then(Value::as_i64)
            .expect("top frame id");

        let scopes_seq = 200 + seq;
        write_dap_message(
            &mut stdin,
            &json!({"type": "request", "seq": scopes_seq, "command": "scopes", "arguments": {"frameId": frame_id}}),
        );
        let scopes = wait_for_message(
            &rx,
            |m| m.get("type") == Some(&json!("response")) && m.get("command") == Some(&json!("scopes")),
            Duration::from_secs(2),
        );
        let locals_ref = scopes
            .pointer("/body/scopes/0/variablesReference")
            .and_then(Value::as_i64)
            .expect("locals variablesReference");

        let vars_seq = 300 + seq;
        write_dap_message(
            &mut stdin,
            &json!({"type": "request", "seq": vars_seq, "command": "variables", "arguments": {"variablesReference": locals_ref}}),
        );
        let vars_resp = wait_for_message(
            &rx,
            |m| {
                m.get("type") == Some(&json!("response"))
                    && m.get("command") == Some(&json!("variables"))
            },
            Duration::from_secs(2),
        );

        let x_value = vars_resp
            .pointer("/body/variables")
            .and_then(Value::as_array)
            .and_then(|vars| vars.iter().find(|value| value["name"] == json!("x")))
            .and_then(|value| value.get("value"))
            .and_then(Value::as_str)
            .map(str::to_string);

        observed_states.push((frame_name.clone(), x_value.clone()));
        if frame_name != "closure#3" || x_value.as_deref() != Some("1") {
            break;
        }
    }

    assert!(
        observed_states
            .iter()
            .any(|(frame_name, x_value)| frame_name != "closure#3" || x_value.as_deref() != Some("1")),
        "repeated stepIn never left the initial closure/x=1 state: {observed_states:?}"
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 999, "command": "disconnect", "arguments": {}}),
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
fn dap_breakpoint_in_hof_test_shows_callable_summary_for_stored_lambda() {
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

    let program = hof_program_path();
    let cwd = program
        .parent()
        .expect("program parent")
        .to_string_lossy()
        .to_string();

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 1, "command": "initialize", "arguments": {}}),
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
                "breakpoints": [{ "line": 57 }]
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
        &json!({"type": "request", "seq": 4, "command": "configurationDone", "arguments": {}}),
    );
    let stop = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );
    assert_eq!(stop.pointer("/body/reason"), Some(&json!("breakpoint")));

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 5, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let stack = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    let frame_id = stack
        .pointer("/body/stackFrames/0/id")
        .and_then(Value::as_i64)
        .expect("frame id");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 6, "command": "scopes", "arguments": {"frameId": frame_id}}),
    );
    let scopes = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("response")) && m.get("command") == Some(&json!("scopes")),
        Duration::from_secs(2),
    );
    let locals_ref = scopes
        .pointer("/body/scopes/0/variablesReference")
        .and_then(Value::as_i64)
        .expect("locals variablesReference");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 7, "command": "variables", "arguments": {"variablesReference": locals_ref}}),
    );
    let vars_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let vars = vars_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("variables");
    let callable = vars
        .iter()
        .find(|value| value["name"] == json!("f"))
        .expect("stored lambda f");
    assert_eq!(callable["type"], json!("callable"));
    assert_eq!(callable["value"], json!("closure f(x)"));
    assert_eq!(callable["variablesReference"], json!(0));

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 8,
            "command": "evaluate",
            "arguments": {
                "expression": "f",
                "frameId": frame_id,
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
    assert_eq!(evaluate_resp.pointer("/body/result"), Some(&json!("closure f(x)")));
    assert_eq!(evaluate_resp.pointer("/body/type"), Some(&json!("callable")));

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
fn dap_breakpoint_in_variant_test_shows_qualified_variant_summary() {
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

    let program = variant_program_path();
    let cwd = program
        .parent()
        .expect("program parent")
        .to_string_lossy()
        .to_string();

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 1, "command": "initialize", "arguments": {}}),
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
                "breakpoints": [{ "line": 30 }]
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
        &json!({"type": "request", "seq": 4, "command": "configurationDone", "arguments": {}}),
    );
    let stop = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );
    assert_eq!(stop.pointer("/body/reason"), Some(&json!("breakpoint")));

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 5, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let stack = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    let frame_id = stack
        .pointer("/body/stackFrames/0/id")
        .and_then(Value::as_i64)
        .expect("frame id");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 6, "command": "scopes", "arguments": {"frameId": frame_id}}),
    );
    let scopes = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("response")) && m.get("command") == Some(&json!("scopes")),
        Duration::from_secs(2),
    );
    let locals_ref = scopes
        .pointer("/body/scopes/0/variablesReference")
        .and_then(Value::as_i64)
        .expect("locals variablesReference");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 7, "command": "variables", "arguments": {"variablesReference": locals_ref}}),
    );
    let vars_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let vars = vars_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("variables");
    let variant = vars
        .iter()
        .find(|value| value["name"] == json!("y"))
        .expect("variant local y");
    assert_eq!(variant["type"], json!("variant"));
    assert_eq!(variant["value"], json!("my-result#ok(42)"));
    let variant_ref = variant["variablesReference"]
        .as_i64()
        .expect("variant variablesReference");
    assert!(variant_ref > 0);

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 8, "command": "variables", "arguments": {"variablesReference": variant_ref}}),
    );
    let child_vars_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let child_vars = child_vars_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("variant child variables");
    assert!(child_vars
        .iter()
        .any(|v| v["name"] == json!("case") && v["value"] == json!("\"my-result#ok\"")));
    assert!(child_vars
        .iter()
        .any(|v| v["name"] == json!("payload") && v["value"] == json!("42")));

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 9,
            "command": "evaluate",
            "arguments": {
                "expression": "y",
                "frameId": frame_id,
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
    assert_eq!(evaluate_resp.pointer("/body/result"), Some(&json!("my-result#ok(42)")));
    assert_eq!(evaluate_resp.pointer("/body/type"), Some(&json!("variant")));

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
fn dap_column_breakpoints_disambiguate_same_line_inline_closures() {
    for (column, expected_name) in breakpoint_disambiguation_columns() {
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

        let program = same_line_closure_program_path();
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
                    "breakpoints": [{ "line": 6, "column": column }]
                }
            }),
        );
        let bp_resp = wait_for_message(
            &rx,
            |m| {
                m.get("type") == Some(&json!("response"))
                    && m.get("command") == Some(&json!("setBreakpoints"))
            },
            Duration::from_secs(2),
        );
        assert_eq!(bp_resp.pointer("/body/breakpoints/0/column"), Some(&json!(column)));

        write_dap_message(
            &mut stdin,
            &json!({
                "type": "request",
                "seq": 4,
                "command": "configurationDone",
                "arguments": {}
            }),
        );
        let stop = wait_for_message(
            &rx,
            |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
            Duration::from_secs(4),
        );
        assert_eq!(stop.pointer("/body/reason"), Some(&json!("breakpoint")));

        write_dap_message(
            &mut stdin,
            &json!({"type": "request", "seq": 5, "command": "stackTrace", "arguments": {"threadId": 1}}),
        );
        let stopped_stack = wait_for_message(
            &rx,
            |m| {
                m.get("type") == Some(&json!("response"))
                    && m.get("command") == Some(&json!("stackTrace"))
            },
            Duration::from_secs(2),
        );
        assert_eq!(stopped_stack.pointer("/body/stackFrames/0/line"), Some(&json!(6)));
        assert_eq!(stopped_stack.pointer("/body/stackFrames/0/column"), Some(&json!(column)));

        write_dap_message(
            &mut stdin,
            &json!({"type": "request", "seq": 6, "command": "stepIn", "arguments": {"threadId": 1}}),
        );
        let step_stop = wait_for_message(
            &rx,
            |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
            Duration::from_secs(2),
        );
        assert_eq!(step_stop.pointer("/body/reason"), Some(&json!("step")));

        write_dap_message(
            &mut stdin,
            &json!({"type": "request", "seq": 7, "command": "stackTrace", "arguments": {"threadId": 1}}),
        );
        let closure_stack = wait_for_message(
            &rx,
            |m| {
                m.get("type") == Some(&json!("response"))
                    && m.get("command") == Some(&json!("stackTrace"))
            },
            Duration::from_secs(2),
        );
        assert_eq!(closure_stack.pointer("/body/stackFrames/0/name"), Some(&json!(expected_name)));
        assert_eq!(closure_stack.pointer("/body/stackFrames/0/line"), Some(&json!(6)));
        assert_eq!(closure_stack.pointer("/body/stackFrames/0/column"), Some(&json!(column)));

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
    assert!(locals
        .iter()
        .any(|v| v["name"] == json!("n") && v["value"] == json!("5")));

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
    assert!(captures
        .iter()
        .any(|v| v["name"] == json!("x") && v["value"] == json!("10")));

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
        &json!({
            "type": "request",
            "seq": 11,
            "command": "evaluate",
            "arguments": {
                "frameId": frame_id,
                "expression": "[n, x]",
                "context": "repl"
            }
        }),
    );
    let list_eval_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("evaluate"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(list_eval_resp.pointer("/body/result"), Some(&json!("[5, 10]")));
    assert_eq!(list_eval_resp.pointer("/body/type"), Some(&json!("list")));
    let list_ref = list_eval_resp
        .pointer("/body/variablesReference")
        .and_then(Value::as_i64)
        .expect("list evaluate variablesReference");
    assert!(list_ref > 0);

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 12, "command": "variables", "arguments": {"variablesReference": list_ref}}),
    );
    let list_vars_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let list_vars = list_vars_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("evaluated list children");
    assert!(list_vars
        .iter()
        .any(|v| v["name"] == json!("0") && v["value"] == json!("5")));
    assert!(list_vars
        .iter()
        .any(|v| v["name"] == json!("1") && v["value"] == json!("10")));

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 13,
            "command": "evaluate",
            "arguments": {
                "frameId": frame_id,
                "expression": "map([n, x], |v| v + 1)",
                "context": "repl"
            }
        }),
    );
    let mapped_eval_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("evaluate"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(mapped_eval_resp.pointer("/body/result"), Some(&json!("[6, 11]")));
    assert_eq!(mapped_eval_resp.pointer("/body/type"), Some(&json!("list")));
    let mapped_ref = mapped_eval_resp
        .pointer("/body/variablesReference")
        .and_then(Value::as_i64)
        .expect("mapped list variablesReference");
    assert!(mapped_ref > 0);

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 14,
            "command": "evaluate",
            "arguments": {
                "frameId": frame_id,
                "expression": "ok(n)? + x",
                "context": "repl"
            }
        }),
    );
    let try_eval_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("evaluate"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(try_eval_resp.pointer("/body/result"), Some(&json!("15")));
    assert_eq!(try_eval_resp.pointer("/body/type"), Some(&json!("i64")));

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 15,
            "command": "evaluate",
            "arguments": {
                "frameId": frame_id,
                "expression": "ok([n, x])",
                "context": "repl"
            }
        }),
    );
    let structured_result_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("evaluate"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(
        structured_result_resp.pointer("/body/result"),
        Some(&json!("ok([5, 10])"))
    );
    assert_eq!(
        structured_result_resp.pointer("/body/type"),
        Some(&json!("result"))
    );
    let structured_result_ref = structured_result_resp
        .pointer("/body/variablesReference")
        .and_then(Value::as_i64)
        .expect("structured result variablesReference");
    assert!(structured_result_ref > 0);

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 16, "command": "variables", "arguments": {"variablesReference": structured_result_ref}}),
    );
    let structured_children_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let structured_children = structured_children_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("structured result children");
    assert!(structured_children
        .iter()
        .any(|v| v["name"] == json!("case") && v["value"] == json!("\"ok\"")));
    assert!(structured_children
        .iter()
        .any(|v| v["name"] == json!("payload") && v["value"] == json!("[5, 10]")));
    let payload_ref = structured_children
        .iter()
        .find(|v| v["name"] == json!("payload"))
        .and_then(|v| v["variablesReference"].as_i64())
        .expect("structured payload reference");
    assert!(payload_ref > 0);

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 17, "command": "variables", "arguments": {"variablesReference": payload_ref}}),
    );
    let payload_vars_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let payload_vars = payload_vars_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("structured payload vars");
    assert!(payload_vars
        .iter()
        .any(|v| v["name"] == json!("0") && v["value"] == json!("5")));
    assert!(payload_vars
        .iter()
        .any(|v| v["name"] == json!("1") && v["value"] == json!("10")));

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 18,
            "command": "evaluate",
            "arguments": {
                "frameId": frame_id,
                "expression": "some({ total: n + x })?.total",
                "context": "repl"
            }
        }),
    );
    let optional_eval_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("evaluate"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(optional_eval_resp.pointer("/body/result"), Some(&json!("some(15)")));
    assert_eq!(optional_eval_resp.pointer("/body/type"), Some(&json!("result")));

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
fn dap_uses_runtime_local_values_and_hides_future_locals() {
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

    let program = while_program_path();
    let cwd = program
        .parent()
        .expect("program parent")
        .to_string_lossy()
        .to_string();

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 1, "command": "initialize", "arguments": {}}),
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
                "breakpoints": [{ "line": 37 }, { "line": 42 }]
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
        &json!({"type": "request", "seq": 4, "command": "configurationDone", "arguments": {}}),
    );
    let first_stop = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );
    assert_eq!(
        first_stop.pointer("/body/reason"),
        Some(&json!("breakpoint"))
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 5, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let first_stack = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(
        first_stack.pointer("/body/stackFrames/0/line"),
        Some(&json!(37))
    );
    let first_frame_id = first_stack
        .pointer("/body/stackFrames/0/id")
        .and_then(Value::as_i64)
        .expect("first frame id");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 6, "command": "scopes", "arguments": {"frameId": first_frame_id}}),
    );
    let first_scopes = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("response")) && m.get("command") == Some(&json!("scopes")),
        Duration::from_secs(2),
    );
    let first_locals_ref = first_scopes
        .pointer("/body/scopes/0/variablesReference")
        .and_then(Value::as_i64)
        .expect("locals variablesReference");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 7, "command": "variables", "arguments": {"variablesReference": first_locals_ref}}),
    );
    let first_locals = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let first_locals = first_locals
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("first locals array");
    assert!(first_locals
        .iter()
        .any(|v| v["name"] == json!("total") && v["value"] == json!("0")));
    assert!(
        first_locals.iter().all(|v| v["name"] != json!("i")),
        "line 37 should not expose i before its declaration"
    );
    assert!(
        first_locals.iter().all(|v| v["name"] != json!("j")),
        "line 37 should not expose j before its declaration"
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 8, "command": "continue", "arguments": {"threadId": 1}}),
    );
    let _first_loop_hit = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 9, "command": "continue", "arguments": {"threadId": 1}}),
    );
    let second_loop_hit = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );
    assert_eq!(
        second_loop_hit.pointer("/body/reason"),
        Some(&json!("breakpoint"))
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 10, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let second_stack = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(
        second_stack.pointer("/body/stackFrames/0/line"),
        Some(&json!(42))
    );
    let second_frame_id = second_stack
        .pointer("/body/stackFrames/0/id")
        .and_then(Value::as_i64)
        .expect("second frame id");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 11, "command": "scopes", "arguments": {"frameId": second_frame_id}}),
    );
    let second_scopes = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("response")) && m.get("command") == Some(&json!("scopes")),
        Duration::from_secs(2),
    );
    let second_locals_ref = second_scopes
        .pointer("/body/scopes/0/variablesReference")
        .and_then(Value::as_i64)
        .expect("second locals variablesReference");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 12, "command": "variables", "arguments": {"variablesReference": second_locals_ref}}),
    );
    let second_locals = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let second_locals = second_locals
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("second locals array");
    assert!(
        second_locals
            .iter()
            .any(|v| v["name"] == json!("total") && v["value"] == json!("1")),
        "expected runtime-updated total on the second loop hit"
    );
    assert!(second_locals
        .iter()
        .any(|v| v["name"] == json!("i") && v["value"] == json!("0")));
    assert!(
        second_locals
            .iter()
            .any(|v| v["name"] == json!("j") && v["value"] == json!("1")),
        "expected runtime-updated j on the second loop hit"
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

fn function_call_program_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/function_call_debug.kettu")
}

fn record_program_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/record_debug.kettu")
}

fn resource_program_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/resource_debug.kettu")
}

fn result_program_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/result_debug.kettu")
}

fn capture_record_program_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/capture_record_debug.kettu")
}

#[test]
fn dap_breakpoint_in_called_function_shows_stack_frame_and_locals() {
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

    let program = function_call_program_path();
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

    // Set breakpoint on line 5 (inside make-pos: result#ok(true))
    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 3,
            "command": "setBreakpoints",
            "arguments": {
                "source": { "path": program.to_string_lossy() },
                "breakpoints": [{ "line": 5 }]
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
    let stop = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );
    assert_eq!(stop.pointer("/body/reason"), Some(&json!("breakpoint")));

    // Check stack frames — should show helper + @test test-call-stack
    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 5, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let stack = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    let frames = stack
        .pointer("/body/stackFrames")
        .and_then(Value::as_array)
        .expect("stack frames");
    assert!(
        frames.len() >= 2,
        "expected at least 2 stack frames (function + test), got {:?}",
        frames
    );
    assert!(
        frames[0]["name"].as_str().unwrap().ends_with("make-pos"),
        "expected frame name ending with 'make-pos', got {:?}",
        frames[0]["name"]
    );
    assert_eq!(frames[0]["line"], json!(5));
    assert_eq!(frames[1]["name"], json!("@test test-result-display"));

    // Check locals in the make-pos frame (frame id from stack[0])
    let func_frame_id = frames[0]["id"].as_i64().unwrap();
    let locals_ref = func_frame_id * 10 + 1;
    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 6,
            "command": "variables",
            "arguments": { "variablesReference": locals_ref }
        }),
    );
    let vars_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let vars = vars_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("variables");
    let var_names: Vec<&str> = vars.iter().filter_map(|v| v["name"].as_str()).collect();
    assert!(
        var_names.contains(&"n"),
        "expected 'n' in make-pos locals, got {:?}",
        var_names
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 7, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let repeated_stack = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    let repeated_frames = repeated_stack
        .pointer("/body/stackFrames")
        .and_then(Value::as_array)
        .expect("repeated stack frames");
    assert_eq!(repeated_frames[0]["id"], json!(func_frame_id));
    assert_eq!(repeated_frames[0]["name"], frames[0]["name"]);

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 8, "command": "scopes", "arguments": {"frameId": func_frame_id}}),
    );
    let first_scopes = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("response")) && m.get("command") == Some(&json!("scopes")),
        Duration::from_secs(2),
    );
    let first_locals_ref = first_scopes
        .pointer("/body/scopes/0/variablesReference")
        .and_then(Value::as_i64)
        .expect("first locals variablesReference");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 9, "command": "scopes", "arguments": {"frameId": func_frame_id}}),
    );
    let second_scopes = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("response")) && m.get("command") == Some(&json!("scopes")),
        Duration::from_secs(2),
    );
    let second_locals_ref = second_scopes
        .pointer("/body/scopes/0/variablesReference")
        .and_then(Value::as_i64)
        .expect("second locals variablesReference");
    assert_eq!(first_locals_ref, locals_ref);
    assert_eq!(second_locals_ref, locals_ref);

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
fn dap_breakpoint_in_list_test_shows_expandable_list_local() {
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

    let program = list_program_path();
    let cwd = program
        .parent()
        .expect("program parent")
        .to_string_lossy()
        .to_string();

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 1, "command": "initialize", "arguments": {}}),
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
        &json!({"type": "request", "seq": 4, "command": "configurationDone", "arguments": {}}),
    );
    let stop = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );
    assert_eq!(stop.pointer("/body/reason"), Some(&json!("breakpoint")));

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 5, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let stack = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(stack.pointer("/body/stackFrames/0/line"), Some(&json!(9)));
    let frame_id = stack
        .pointer("/body/stackFrames/0/id")
        .and_then(Value::as_i64)
        .expect("frame id");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 6, "command": "scopes", "arguments": {"frameId": frame_id}}),
    );
    let scopes = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("response")) && m.get("command") == Some(&json!("scopes")),
        Duration::from_secs(2),
    );
    let locals_ref = scopes
        .pointer("/body/scopes/0/variablesReference")
        .and_then(Value::as_i64)
        .expect("locals variablesReference");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 7, "command": "variables", "arguments": {"variablesReference": locals_ref}}),
    );
    let vars_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let vars = vars_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("variables");
    let arr = vars
        .iter()
        .find(|v| v["name"] == json!("arr"))
        .expect("list local arr");
    assert_eq!(arr["type"], json!("list"));
    assert_eq!(arr["value"], json!("[10, 20, 30]"));
    let arr_ref = arr["variablesReference"]
        .as_i64()
        .expect("arr variablesReference");
    assert!(arr_ref > 0, "list local should be expandable");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 8, "command": "variables", "arguments": {"variablesReference": arr_ref}}),
    );
    let list_vars_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let list_vars = list_vars_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("list child variables");
    assert!(list_vars
        .iter()
        .any(|v| v["name"] == json!("0") && v["value"] == json!("10")));
    assert!(list_vars
        .iter()
        .any(|v| v["name"] == json!("1") && v["value"] == json!("20")));
    assert!(list_vars
        .iter()
        .any(|v| v["name"] == json!("2") && v["value"] == json!("30")));

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
fn dap_breakpoint_in_structured_capture_shows_expandable_record_capture() {
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

    let program = capture_record_program_path();
    let cwd = program
        .parent()
        .expect("program parent")
        .to_string_lossy()
        .to_string();

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 1, "command": "initialize", "arguments": {}}),
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
                "breakpoints": [{ "line": 7 }]
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
        &json!({"type": "request", "seq": 4, "command": "configurationDone", "arguments": {}}),
    );
    let stop = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );
    assert_eq!(stop.pointer("/body/reason"), Some(&json!("breakpoint")));

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
    let stack = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(stack.pointer("/body/stackFrames/0/name"), Some(&json!("add-rec")));
    let frame_id = stack
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
    assert!(locals
        .iter()
        .any(|v| v["name"] == json!("n")));

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
    let rec = captures
        .iter()
        .find(|v| v["name"] == json!("rec"))
        .expect("record capture rec");
    assert_eq!(rec["type"], json!("record"));
    assert_eq!(rec["value"], json!("{ a: 1, b: 2 }"));
    let rec_ref = rec["variablesReference"]
        .as_i64()
        .expect("rec variablesReference");
    assert!(rec_ref > 0, "record capture should be expandable");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 10, "command": "variables", "arguments": {"variablesReference": rec_ref}}),
    );
    let rec_vars_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let rec_vars = rec_vars_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("record capture child variables");
    assert!(rec_vars
        .iter()
        .any(|v| v["name"] == json!("a") && v["value"] == json!("1")));
    assert!(rec_vars
        .iter()
        .any(|v| v["name"] == json!("b") && v["value"] == json!("2")));

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
fn dap_breakpoint_in_record_test_shows_record_summary() {
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

    let program = record_program_path();
    let cwd = program
        .parent()
        .expect("program parent")
        .to_string_lossy()
        .to_string();

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 1, "command": "initialize", "arguments": {}}),
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
                "breakpoints": [{ "line": 6 }]
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
        &json!({"type": "request", "seq": 4, "command": "configurationDone", "arguments": {}}),
    );
    let stop = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );
    assert_eq!(stop.pointer("/body/reason"), Some(&json!("breakpoint")));

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 5, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let stack = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(stack.pointer("/body/stackFrames/0/line"), Some(&json!(6)));
    let frame_id = stack
        .pointer("/body/stackFrames/0/id")
        .and_then(Value::as_i64)
        .expect("frame id");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 6, "command": "scopes", "arguments": {"frameId": frame_id}}),
    );
    let scopes = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("response")) && m.get("command") == Some(&json!("scopes")),
        Duration::from_secs(2),
    );
    let locals_ref = scopes
        .pointer("/body/scopes/0/variablesReference")
        .and_then(Value::as_i64)
        .expect("locals variablesReference");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 7, "command": "variables", "arguments": {"variablesReference": locals_ref}}),
    );
    let vars_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let vars = vars_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("variables");
    let record = vars
        .iter()
        .find(|v| v["name"] == json!("r"))
        .expect("record local r");
    assert_eq!(record["type"], json!("record"));
    let record_ref = record["variablesReference"]
        .as_i64()
        .expect("record variablesReference");
    assert!(record_ref > 0, "record local should be expandable");
    let rendered = record["value"].as_str().expect("record summary");
    assert!(rendered.contains("a: 1"), "record summary should include field a: {rendered}");
    assert!(rendered.contains("b: 2"), "record summary should include field b: {rendered}");
    assert!(rendered.contains("c: 3"), "record summary should include field c: {rendered}");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 8, "command": "variables", "arguments": {"variablesReference": record_ref}}),
    );
    let child_vars_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let child_vars = child_vars_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("record child variables");
    assert!(child_vars
        .iter()
        .any(|v| v["name"] == json!("a") && v["value"] == json!("1")));
    assert!(child_vars
        .iter()
        .any(|v| v["name"] == json!("b") && v["value"] == json!("2")));
    assert!(child_vars
        .iter()
        .any(|v| v["name"] == json!("c") && v["value"] == json!("3")));

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
fn dap_breakpoint_in_resource_test_shows_opaque_resource_summary() {
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

    let program = resource_program_path();
    let cwd = program
        .parent()
        .expect("program parent")
        .to_string_lossy()
        .to_string();

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 1, "command": "initialize", "arguments": {}}),
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
                "breakpoints": [{ "line": 17 }]
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
        &json!({"type": "request", "seq": 4, "command": "configurationDone", "arguments": {}}),
    );
    let stop = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );
    assert_eq!(stop.pointer("/body/reason"), Some(&json!("breakpoint")));

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 5, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let stack = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    let frame_id = stack
        .pointer("/body/stackFrames/0/id")
        .and_then(Value::as_i64)
        .expect("frame id");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 6, "command": "scopes", "arguments": {"frameId": frame_id}}),
    );
    let scopes = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("response")) && m.get("command") == Some(&json!("scopes")),
        Duration::from_secs(2),
    );
    let locals_ref = scopes
        .pointer("/body/scopes/0/variablesReference")
        .and_then(Value::as_i64)
        .expect("locals variablesReference");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 7, "command": "variables", "arguments": {"variablesReference": locals_ref}}),
    );
    let vars_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let vars = vars_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("variables");
    let resource = vars
        .iter()
        .find(|value| value["name"] == json!("c"))
        .expect("resource local c");
    assert_eq!(resource["type"], json!("resource"));
    assert_eq!(resource["value"], json!("resource counter"));
    assert_eq!(resource["variablesReference"], json!(0));

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 8,
            "command": "evaluate",
            "arguments": {
                "expression": "c",
                "frameId": frame_id,
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
    assert_eq!(evaluate_resp.pointer("/body/result"), Some(&json!("resource counter")));
    assert_eq!(evaluate_resp.pointer("/body/type"), Some(&json!("resource")));

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
fn dap_can_launch_prebuilt_wasm_with_embedded_debug_source() {
    let temp_dir = tempdir().expect("temp dir");
    let source_path = temp_dir.path().join("artifact_resource_debug.kettu");
    let wasm_path = temp_dir.path().join("artifact_resource_debug.wasm");

    fs::write(
        &source_path,
        r#"package local:artifact-resource-debug;

interface tests {
    resource counter {
        constructor(initial: s32) {
            initial;
        }

        get: func() -> s32 {
            self;
        }
    }

    @test
    test-resource-display: func() -> bool {
        let c = counter(10);
        return true;
    }
}
"#,
    )
    .expect("write resource source");

    let build = Command::new(env!("CARGO_BIN_EXE_kettu"))
        .args([
            "build",
            "--core",
            "--debug",
            source_path.to_str().expect("source path"),
            "-o",
            wasm_path.to_str().expect("wasm path"),
        ])
        .output()
        .expect("build debug wasm");
    assert!(
        build.status.success(),
        "build debug wasm should succeed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    fs::remove_file(&source_path).expect("remove source file after build");

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

    let cwd = temp_dir.path().to_string_lossy().to_string();

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 1, "command": "initialize", "arguments": {}}),
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
                "program": wasm_path.to_string_lossy(),
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
                "source": { "path": source_path.to_string_lossy() },
                "breakpoints": [{ "line": 17 }]
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
        &json!({"type": "request", "seq": 4, "command": "configurationDone", "arguments": {}}),
    );
    let stop = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );
    assert_eq!(stop.pointer("/body/reason"), Some(&json!("breakpoint")));

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 5, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let stack = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(
        stack.pointer("/body/stackFrames/0/source/path"),
        Some(&json!(source_path.to_string_lossy().to_string()))
    );
    let frame_id = stack
        .pointer("/body/stackFrames/0/id")
        .and_then(Value::as_i64)
        .expect("frame id");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 6, "command": "scopes", "arguments": {"frameId": frame_id}}),
    );
    let scopes = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("response")) && m.get("command") == Some(&json!("scopes")),
        Duration::from_secs(2),
    );
    let locals_ref = scopes
        .pointer("/body/scopes/0/variablesReference")
        .and_then(Value::as_i64)
        .expect("locals variablesReference");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 7, "command": "variables", "arguments": {"variablesReference": locals_ref}}),
    );
    let vars_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let vars = vars_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("variables");
    let resource = vars
        .iter()
        .find(|value| value["name"] == json!("c"))
        .expect("resource local c");
    assert_eq!(resource["type"], json!("resource"));
    assert_eq!(resource["value"], json!("resource counter"));

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 8,
            "command": "evaluate",
            "arguments": {
                "expression": "c",
                "frameId": frame_id,
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
    assert_eq!(evaluate_resp.pointer("/body/result"), Some(&json!("resource counter")));
    assert_eq!(evaluate_resp.pointer("/body/type"), Some(&json!("resource")));

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
fn dap_breakpoint_in_result_test_shows_expandable_payload() {
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

    let program = result_program_path();
    let cwd = program
        .parent()
        .expect("program parent")
        .to_string_lossy()
        .to_string();

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 1, "command": "initialize", "arguments": {}}),
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
                "breakpoints": [{ "line": 7 }]
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
        &json!({"type": "request", "seq": 4, "command": "configurationDone", "arguments": {}}),
    );
    let stop = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );
    assert_eq!(stop.pointer("/body/reason"), Some(&json!("breakpoint")));

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 5, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let stack = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    let frame_id = stack
        .pointer("/body/stackFrames/0/id")
        .and_then(Value::as_i64)
        .expect("frame id");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 6, "command": "scopes", "arguments": {"frameId": frame_id}}),
    );
    let scopes = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("response")) && m.get("command") == Some(&json!("scopes")),
        Duration::from_secs(2),
    );
    let locals_ref = scopes
        .pointer("/body/scopes/0/variablesReference")
        .and_then(Value::as_i64)
        .expect("locals variablesReference");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 7, "command": "variables", "arguments": {"variablesReference": locals_ref}}),
    );
    let vars_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let vars = vars_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("variables");
    let result = vars
        .iter()
        .find(|v| v["name"] == json!("r"))
        .expect("result local r");
    assert_eq!(result["type"], json!("result"));
    assert_eq!(result["value"], json!("ok(10)"));
    let result_ref = result["variablesReference"]
        .as_i64()
        .expect("result variablesReference");
    assert!(result_ref > 0, "result local should be expandable");

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 8, "command": "variables", "arguments": {"variablesReference": result_ref}}),
    );
    let child_vars_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("variables"))
        },
        Duration::from_secs(2),
    );
    let child_vars = child_vars_resp
        .pointer("/body/variables")
        .and_then(Value::as_array)
        .expect("result child variables");
    assert!(child_vars
        .iter()
        .any(|v| v["name"] == json!("case") && v["value"] == json!("\"ok\"")));
    assert!(child_vars
        .iter()
        .any(|v| v["name"] == json!("payload") && v["value"] == json!("10")));

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
fn dap_breakpoint_in_nested_closure_shows_runtime_stack_frames() {
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
                "breakpoints": [{ "line": 8 }]
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
    let stop_on_inner = wait_for_message(
        &rx,
        |m| m.get("type") == Some(&json!("event")) && m.get("event") == Some(&json!("stopped")),
        Duration::from_secs(4),
    );
    assert_eq!(
        stop_on_inner.pointer("/body/reason"),
        Some(&json!("breakpoint"))
    );

    write_dap_message(
        &mut stdin,
        &json!({"type": "request", "seq": 5, "command": "stackTrace", "arguments": {"threadId": 1}}),
    );
    let stack = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );
    let frames = stack
        .pointer("/body/stackFrames")
        .and_then(Value::as_array)
        .expect("stack frames");
    assert_eq!(frames[0]["name"], json!("closure#2"));
    assert_eq!(frames[1]["name"], json!("outer"));
    assert_eq!(
        frames[2]["name"],
        json!('@'.to_string() + "test nested-closure-flow")
    );
    assert_eq!(frames[0]["line"], json!(8));

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
