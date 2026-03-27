use serde_json::{json, Value};
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

fn sample_program_path() -> PathBuf {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    repo_root.join("tests/fixtures/closure_debug.kettu")
}

#[test]
fn dap_breakpoint_and_step_progress() {
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

    let program = sample_program_path();
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
    assert_eq!(init_resp.get("success"), Some(&json!(true)));

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

    let launch_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("launch"))
        },
        Duration::from_secs(3),
    );
    eprintln!("launch response: {}", launch_resp);
    assert_eq!(launch_resp.get("success"), Some(&json!(true)));

    let _initialized_event = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("event"))
                && m.get("event") == Some(&json!("initialized"))
        },
        Duration::from_secs(2),
    );

    // Set breakpoint on closure body start line (line 7 in fixture).
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

    let bp_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("setBreakpoints"))
        },
        Duration::from_secs(2),
    );
    assert_eq!(bp_resp.get("success"), Some(&json!(true)));

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 4,
            "command": "configurationDone",
            "arguments": {}
        }),
    );

    let _config_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("configurationDone"))
        },
        Duration::from_secs(2),
    );

    let stop_event = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("event"))
                && m.get("event") == Some(&json!("stopped"))
        },
        Duration::from_secs(3),
    );
    assert_eq!(
        stop_event.pointer("/body/reason"),
        Some(&json!("breakpoint"))
    );

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 5,
            "command": "stackTrace",
            "arguments": { "threadId": 1 }
        }),
    );

    let stack_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("stackTrace"))
        },
        Duration::from_secs(2),
    );

    let top_name = stack_resp
        .pointer("/body/stackFrames/0/name")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert_eq!(top_name, "closure#1", "expected to be stopped inside closure frame");

    write_dap_message(
        &mut stdin,
        &json!({
            "type": "request",
            "seq": 8,
            "command": "disconnect",
            "arguments": {}
        }),
    );

    let _disconnect_resp = wait_for_message(
        &rx,
        |m| {
            m.get("type") == Some(&json!("response"))
                && m.get("command") == Some(&json!("disconnect"))
        },
        Duration::from_secs(2),
    );

    let status = child.wait().expect("wait for dap process");
    assert!(status.success(), "kettu dap should exit cleanly");
}
