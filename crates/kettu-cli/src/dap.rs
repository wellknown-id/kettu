use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;

#[derive(Clone, Debug)]
struct ListedTest {
    name: String,
    line: i64,
    end_line: i64,
}

#[derive(Clone, Debug)]
struct Variable {
    name: String,
    value: String,
    var_type: String,
}

#[derive(Clone, Debug)]
struct ClosureRange {
    start_line: i64,
    end_line: i64,
    name: String,
}

#[derive(Clone, Debug)]
struct DebugSession {
    program: Option<PathBuf>,
    cwd: Option<PathBuf>,
    source_lines: Vec<String>,
    tests: Vec<ListedTest>,
    closures: Vec<ClosureRange>,
    stop_on_entry: bool,
    configured: bool,
    terminated: bool,
    current_test: usize,
    current_line: i64,
    breakpoints: HashMap<String, BTreeSet<i64>>,
    locals: Vec<Variable>,
}

impl DebugSession {
    fn new() -> Self {
        Self {
            program: None,
            cwd: None,
            source_lines: Vec::new(),
            tests: Vec::new(),
            closures: Vec::new(),
            stop_on_entry: false,
            configured: false,
            terminated: false,
            current_test: 0,
            current_line: 0,
            breakpoints: HashMap::new(),
            locals: Vec::new(),
        }
    }

    fn has_tests(&self) -> bool {
        !self.tests.is_empty()
    }

    fn current_file_key(&self) -> Option<String> {
        self.program
            .as_ref()
            .map(|p| normalize_path_key(&p.display().to_string()))
    }

    fn breakpoint_hit(&self, line: i64) -> bool {
        let Some(file) = self.current_file_key() else {
            return false;
        };
        self.breakpoints
            .get(&file)
            .map(|set| set.contains(&line))
            .unwrap_or(false)
    }

    fn set_current_locals(&mut self) {
        self.locals = infer_locals(&self.source_lines, self.current_line);
    }

    fn active_test(&self) -> Option<&ListedTest> {
        self.tests
            .iter()
            .find(|t| self.current_line >= t.line && self.current_line <= t.end_line)
            .or_else(|| self.tests.get(self.current_test))
    }

    fn advance_one_line(&mut self) -> bool {
        if self.tests.is_empty() {
            return false;
        }

        loop {
            if self.current_test >= self.tests.len() {
                return false;
            }

            let test = &self.tests[self.current_test];

            if self.current_line < test.line {
                self.current_line = test.line;
                self.set_current_locals();
                return true;
            }

            if self.current_line < test.end_line {
                self.current_line += 1;
                self.set_current_locals();
                return true;
            }

            self.current_test += 1;
            if self.current_test >= self.tests.len() {
                return false;
            }

            let next_start = self.tests[self.current_test].line;
            self.current_line = next_start - 1;
        }
    }

    fn run_until_breakpoint_or_end(&mut self) -> StopOutcome {
        while self.advance_one_line() {
            if self.breakpoint_hit(self.current_line) {
                return StopOutcome::Stopped("breakpoint");
            }
        }
        StopOutcome::Terminated
    }

    fn step_once_or_end(&mut self) -> StopOutcome {
        if self.advance_one_line() {
            if self.breakpoint_hit(self.current_line) {
                StopOutcome::Stopped("breakpoint")
            } else {
                StopOutcome::Stopped("step")
            }
        } else {
            StopOutcome::Terminated
        }
    }
}

enum StopOutcome {
    Stopped(&'static str),
    Terminated,
}

pub fn run_server() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = stdout.lock();

    let mut session = DebugSession::new();

    loop {
        let Some(msg) = read_dap_message(&mut reader)? else {
            break;
        };

        let msg_type = msg.get("type").and_then(Value::as_str).unwrap_or_default();
        if msg_type != "request" {
            continue;
        }

        let seq = msg.get("seq").and_then(Value::as_i64).unwrap_or(0);
        let command = msg
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let arguments = msg.get("arguments").cloned().unwrap_or_else(|| json!({}));

        match command {
            "initialize" => {
                send_response(
                    &mut writer,
                    seq,
                    command,
                    true,
                    Some(json!({
                        "supportsConfigurationDoneRequest": true,
                        "supportsStepInTargetsRequest": false,
                        "supportsEvaluateForHovers": false,
                        "supportsSetVariable": false,
                    })),
                    None,
                )?;
            }
            "launch" => {
                let program = arguments
                    .get("program")
                    .and_then(Value::as_str)
                    .map(PathBuf::from);
                let cwd = arguments
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(PathBuf::from);
                let stop_on_entry = arguments
                    .get("stopOnEntry")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);

                match load_program_state(program.clone()) {
                    Ok((source_lines, tests, closures)) => {
                        session.program = program;
                        session.cwd = cwd;
                        session.source_lines = source_lines;
                        session.tests = tests;
                        session.closures = closures;
                        session.stop_on_entry = stop_on_entry;
                        session.configured = false;
                        session.terminated = false;
                        session.current_test = 0;
                        session.current_line = session
                            .tests
                            .first()
                            .map(|t| t.line - 1)
                            .unwrap_or(0);
                        session.locals.clear();

                        send_response(&mut writer, seq, command, true, Some(json!({})), None)?;
                        send_event(&mut writer, "initialized", Some(json!({})))?;
                    }
                    Err(err) => {
                        send_response(&mut writer, seq, command, false, None, Some(err))?;
                    }
                }
            }
            "setBreakpoints" => {
                let source_path = arguments
                    .get("source")
                    .and_then(|s| s.get("path"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let key = normalize_path_key(source_path);

                let mut lines = BTreeSet::new();
                let mut response_bps = Vec::new();

                if let Some(req_bps) = arguments.get("breakpoints").and_then(Value::as_array) {
                    for bp in req_bps {
                        let line = bp.get("line").and_then(Value::as_i64).unwrap_or(1).max(1);
                        lines.insert(line);
                        response_bps.push(json!({
                            "verified": true,
                            "line": line
                        }));
                    }
                }

                session.breakpoints.insert(key, lines);
                send_response(
                    &mut writer,
                    seq,
                    command,
                    true,
                    Some(json!({ "breakpoints": response_bps })),
                    None,
                )?;
            }
            "configurationDone" => {
                session.configured = true;
                send_response(&mut writer, seq, command, true, Some(json!({})), None)?;

                if !session.has_tests() {
                    session.terminated = true;
                    send_event(&mut writer, "terminated", Some(json!({})))?;
                    continue;
                }

                if session.stop_on_entry {
                    if session.advance_one_line() {
                        send_stopped_event(&mut writer, "entry")?;
                    } else {
                        session.terminated = true;
                        send_event(&mut writer, "terminated", Some(json!({})))?;
                    }
                } else {
                    match session.run_until_breakpoint_or_end() {
                        StopOutcome::Stopped(reason) => {
                            send_stopped_event(&mut writer, reason)?;
                        }
                        StopOutcome::Terminated => {
                            session.terminated = true;
                            send_event(&mut writer, "terminated", Some(json!({})))?;
                        }
                    }
                }
            }
            "threads" => {
                send_response(
                    &mut writer,
                    seq,
                    command,
                    true,
                    Some(json!({
                        "threads": [{ "id": 1, "name": "main" }]
                    })),
                    None,
                )?;
            }
            "stackTrace" => {
                let frames = build_stack_frames(&session);
                send_response(
                    &mut writer,
                    seq,
                    command,
                    true,
                    Some(json!({
                        "stackFrames": frames,
                        "totalFrames": frames.len()
                    })),
                    None,
                )?;
            }
            "scopes" => {
                send_response(
                    &mut writer,
                    seq,
                    command,
                    true,
                    Some(json!({
                        "scopes": [{
                            "name": "Locals",
                            "presentationHint": "locals",
                            "variablesReference": 1,
                            "expensive": false
                        }]
                    })),
                    None,
                )?;
            }
            "variables" => {
                let vars: Vec<Value> = session
                    .locals
                    .iter()
                    .map(|v| {
                        json!({
                            "name": v.name,
                            "value": v.value,
                            "type": v.var_type,
                            "variablesReference": 0
                        })
                    })
                    .collect();

                send_response(
                    &mut writer,
                    seq,
                    command,
                    true,
                    Some(json!({ "variables": vars })),
                    None,
                )?;
            }
            "continue" => {
                send_response(
                    &mut writer,
                    seq,
                    command,
                    true,
                    Some(json!({ "allThreadsContinued": true })),
                    None,
                )?;

                if session.terminated {
                    continue;
                }

                match session.run_until_breakpoint_or_end() {
                    StopOutcome::Stopped(reason) => send_stopped_event(&mut writer, reason)?,
                    StopOutcome::Terminated => {
                        session.terminated = true;
                        send_event(&mut writer, "terminated", Some(json!({})))?;
                    }
                }
            }
            "next" | "stepIn" | "stepOut" => {
                send_response(&mut writer, seq, command, true, Some(json!({})), None)?;

                if session.terminated {
                    continue;
                }

                match session.step_once_or_end() {
                    StopOutcome::Stopped(reason) => send_stopped_event(&mut writer, reason)?,
                    StopOutcome::Terminated => {
                        session.terminated = true;
                        send_event(&mut writer, "terminated", Some(json!({})))?;
                    }
                }
            }
            "pause" => {
                send_response(&mut writer, seq, command, true, Some(json!({})), None)?;
                if !session.terminated {
                    send_stopped_event(&mut writer, "pause")?;
                }
            }
            "disconnect" => {
                session.terminated = true;
                send_response(&mut writer, seq, command, true, Some(json!({})), None)?;
                send_event(&mut writer, "terminated", Some(json!({})))?;
                break;
            }
            _ => {
                send_response(&mut writer, seq, command, true, Some(json!({})), None)?;
            }
        }
    }

    Ok(())
}

fn send_stopped_event(writer: &mut impl Write, reason: &str) -> io::Result<()> {
    send_event(
        writer,
        "stopped",
        Some(json!({
            "reason": reason,
            "threadId": 1,
            "allThreadsStopped": true
        })),
    )
}

fn load_program_state(
    program: Option<PathBuf>,
) -> Result<(Vec<String>, Vec<ListedTest>, Vec<ClosureRange>), String> {
    let Some(program) = program else {
        return Err("Missing launch argument: program".to_string());
    };

    let source = fs::read_to_string(&program)
        .map_err(|e| format!("Failed to read source file '{}': {}", program.display(), e))?;
    let tests = list_tests_single_file(&program)?;
    let source_lines: Vec<String> = source.lines().map(ToString::to_string).collect();
    let closures = parse_closures(&source_lines);
    Ok((source_lines, tests, closures))
}

fn build_stack_frames(session: &DebugSession) -> Vec<Value> {
    let Some(program) = &session.program else {
        return vec![];
    };

    let mut frames = Vec::new();
    let mut frame_id = 1;

    if let Some(closure) = session
        .closures
        .iter()
        .find(|c| session.current_line >= c.start_line && session.current_line <= c.end_line)
    {
        frames.push(json!({
            "id": frame_id,
            "name": closure.name,
            "line": session.current_line.max(1),
            "column": 1,
            "source": {
                "name": program.file_name().and_then(|n| n.to_str()).unwrap_or("program.kettu"),
                "path": program.display().to_string()
            }
        }));
        frame_id += 1;
    }

    let frame_name = session
        .active_test()
        .map(|t| format!("@test {}", t.name))
        .unwrap_or_else(|| "@test <unknown>".to_string());

    frames.push(json!({
        "id": frame_id,
        "name": frame_name,
        "line": session.current_line.max(1),
        "column": 1,
        "source": {
            "name": program.file_name().and_then(|n| n.to_str()).unwrap_or("program.kettu"),
            "path": program.display().to_string()
        }
    }));

    frames
}

fn normalize_path_key(path: &str) -> String {
    if cfg!(windows) {
        path.replace('\\', "/").to_ascii_lowercase()
    } else {
        path.replace('\\', "/")
    }
}

fn offset_to_line(source: &str, offset: usize) -> usize {
    source[..offset.min(source.len())]
        .bytes()
        .filter(|&b| b == b'\n')
        .count()
        + 1
}

fn list_tests_single_file(file: &PathBuf) -> Result<Vec<ListedTest>, String> {
    use kettu_parser::{Gate, InterfaceItem, TopLevelItem};

    let content = fs::read_to_string(file)
        .map_err(|e| format!("Error reading file '{}': {}", file.display(), e))?;

    let (ast, parse_errors) = kettu_parser::parse_file(&content);
    if !parse_errors.is_empty() {
        let all = parse_errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("Parse error(s): {}", all));
    }

    let ast = ast.ok_or_else(|| "Failed to parse file".to_string())?;

    let mut listed = Vec::new();
    for item in &ast.items {
        if let TopLevelItem::Interface(iface) = item {
            for iface_item in &iface.items {
                if let InterfaceItem::Func(func) = iface_item {
                    let is_test = func.gates.iter().any(|g| matches!(g, Gate::Test));
                    if !is_test {
                        continue;
                    }

                    listed.push(ListedTest {
                        name: func.name.name.clone(),
                        line: offset_to_line(&content, func.span.start) as i64,
                        end_line: offset_to_line(&content, func.span.end) as i64,
                    });
                }
            }
        }
    }

    listed.sort_by_key(|t| t.line);
    Ok(listed)
}

fn parse_closures(source_lines: &[String]) -> Vec<ClosureRange> {
    let mut closures = Vec::new();
    let mut active_start: Option<i64> = None;
    let mut depth: i64 = 0;
    let mut counter = 1;
    let mut saw_pipe = false;

    for (index, line) in source_lines.iter().enumerate() {
        let line_no = (index + 1) as i64;
        let trimmed = line.trim();

        // Track whether we're inside a closure body by brace depth once started.
        if active_start.is_some() {
            depth += trimmed.chars().filter(|c| *c == '{').count() as i64;
            depth -= trimmed.chars().filter(|c| *c == '}').count() as i64;

            if depth <= 0 {
                let start_line = active_start.unwrap_or(line_no);
                closures.push(ClosureRange {
                    start_line,
                    end_line: line_no,
                    name: format!("closure#{}", counter),
                });
                counter += 1;
                active_start = None;
                depth = 0;
            }
            continue;
        }

        // Detect the start of a closure header: a pipe indicates parameters.
        if trimmed.contains('|') {
            saw_pipe = true;
        }

        // Start counting when we see an opening brace after a header with a pipe
        // (can be on the same line or the following line). If there is no brace,
        // treat it as a single-line closure body on that line.
        if saw_pipe {
            if trimmed.contains('{') {
                active_start = Some(line_no);
                depth = trimmed.chars().filter(|c| *c == '{').count() as i64;
                depth -= trimmed.chars().filter(|c| *c == '}').count() as i64;
                saw_pipe = false;
                if depth <= 0 {
                    // Single-line closure like `|x| { x + 1 }`.
                    closures.push(ClosureRange {
                        start_line: line_no,
                        end_line: line_no,
                        name: format!("closure#{}", counter),
                    });
                    counter += 1;
                    active_start = None;
                    depth = 0;
                }
            } else {
                // No braces: treat as inline expression closure on this line.
                closures.push(ClosureRange {
                    start_line: line_no,
                    end_line: line_no,
                    name: format!("closure#{}", counter),
                });
                counter += 1;
                saw_pipe = false;
                active_start = None;
                depth = 0;
            }
        }
    }

    closures
}

fn infer_locals(source_lines: &[String], current_line: i64) -> Vec<Variable> {
    if current_line <= 0 {
        return Vec::new();
    }

    let mut values: HashMap<String, String> = HashMap::new();
    let max_index = (current_line as usize).min(source_lines.len());

    for line in source_lines.iter().take(max_index) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("let ") {
            if let Some((name, expr)) = split_assignment(rest) {
                if is_identifier(name) {
                    values.insert(name.to_string(), infer_expr_value(expr));
                }
            }
            continue;
        }

        if let Some((name, expr)) = split_assignment(trimmed) {
            if is_identifier(name) {
                values.insert(name.to_string(), infer_expr_value(expr));
            }
        }
    }

    let mut vars: Vec<Variable> = values
        .into_iter()
        .map(|(name, value)| {
            let var_type = if value == "true" || value == "false" {
                "bool"
            } else if value.starts_with('"') && value.ends_with('"') {
                "string"
            } else if value.parse::<i64>().is_ok() {
                "i64"
            } else if value.parse::<f64>().is_ok() {
                "f64"
            } else {
                "unknown"
            }
            .to_string();

            Variable {
                name,
                value,
                var_type,
            }
        })
        .collect();

    vars.sort_by(|a, b| a.name.cmp(&b.name));
    vars
}

fn split_assignment(input: &str) -> Option<(&str, &str)> {
    let mut parts = input.splitn(2, '=');
    let left = parts.next()?.trim();
    let right = parts.next()?.trim().trim_end_matches(';').trim();
    if left.is_empty() || right.is_empty() {
        return None;
    }
    Some((left, right))
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn infer_expr_value(expr: &str) -> String {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return "<unknown>".to_string();
    }
    if trimmed == "true" || trimmed == "false" {
        return trimmed.to_string();
    }
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('"') && trimmed.contains('"'))
    {
        return trimmed.to_string();
    }
    if trimmed.parse::<i64>().is_ok() || trimmed.parse::<f64>().is_ok() {
        return trimmed.to_string();
    }
    "<expr>".to_string()
}

fn send_response(
    writer: &mut impl Write,
    request_seq: i64,
    command: &str,
    success: bool,
    body: Option<Value>,
    message: Option<String>,
) -> io::Result<()> {
    let mut response = json!({
        "type": "response",
        "request_seq": request_seq,
        "success": success,
        "command": command,
    });

    if let Some(body) = body {
        response["body"] = body;
    }
    if let Some(message) = message {
        response["message"] = json!(message);
    }

    write_dap_message(writer, &response)
}

fn send_event(writer: &mut impl Write, event: &str, body: Option<Value>) -> io::Result<()> {
    let mut payload = json!({
        "type": "event",
        "event": event,
    });
    if let Some(body) = body {
        payload["body"] = body;
    }
    write_dap_message(writer, &payload)
}

fn write_dap_message(writer: &mut impl Write, payload: &Value) -> io::Result<()> {
    let body = payload.to_string();
    write!(writer, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    writer.flush()
}

fn read_dap_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut content_length: Option<usize> = None;

    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Ok(None);
        }

        if line == "\r\n" {
            break;
        }

        let lower = line.to_ascii_lowercase();
        if lower.starts_with("content-length:") {
            let raw = line
                .split(':')
                .nth(1)
                .map(str::trim)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid DAP header"))?;
            let parsed = raw.parse::<usize>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "Invalid Content-Length value")
            })?;
            content_length = Some(parsed);
        }
    }

    let len = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Missing Content-Length"))?;

    let mut buffer = vec![0u8; len];
    reader.read_exact(&mut buffer)?;
    let value: Value = serde_json::from_slice(&buffer)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::{infer_locals, parse_closures, DebugSession, ListedTest};
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn closure_ranges_are_detected() {
        let source = vec![
            "interface main {".to_string(),
            "  @test fn t() -> bool {".to_string(),
            "    let f = |x| {".to_string(),
            "      x + 1".to_string(),
            "    }".to_string(),
            "    true".to_string(),
            "  }".to_string(),
            "}".to_string(),
        ];

        let closures = parse_closures(&source);
        assert_eq!(closures.len(), 1);
        assert_eq!(closures[0].start_line, 3);
        assert_eq!(closures[0].end_line, 5);
    }

    #[test]
    fn stepping_always_progresses_or_ends() {
        let mut session = DebugSession::new();
        session.tests = vec![ListedTest {
            name: "t".to_string(),
            line: 10,
            end_line: 12,
        }];
        session.current_line = 9;

        assert!(session.advance_one_line());
        assert_eq!(session.current_line, 10);
        assert!(session.advance_one_line());
        assert_eq!(session.current_line, 11);
        assert!(session.advance_one_line());
        assert_eq!(session.current_line, 12);
        assert!(!session.advance_one_line());
    }

    #[test]
    fn locals_inference_reads_assignments() {
        let lines = vec![
            "interface main {".to_string(),
            "  @test fn t() -> bool {".to_string(),
            "    let a = 1".to_string(),
            "    let b = \"ok\"".to_string(),
            "    a = 2".to_string(),
            "    true".to_string(),
            "  }".to_string(),
            "}".to_string(),
        ];

        let locals = infer_locals(&lines, 6);
        let a = locals.iter().find(|v| v.name == "a").unwrap();
        let b = locals.iter().find(|v| v.name == "b").unwrap();

        assert_eq!(a.value, "2");
        assert_eq!(b.value, "\"ok\"");
    }

    #[test]
    fn stack_trace_includes_closure_frame() {
        let lines = vec![
            "interface t {".to_string(),
            "  @test fn t() -> bool {".to_string(),
            "    let y = reduce([1,2], 0) |acc, n| {".to_string(),
            "      acc + n".to_string(),
            "    }".to_string(),
            "    true".to_string(),
            "  }".to_string(),
            "}".to_string(),
        ];

        let closures = parse_closures(&lines);
        let mut session = DebugSession::new();
        session.program = Some(PathBuf::from("/tmp/file.kettu"));
        session.source_lines = lines;
        session.tests = vec![ListedTest {
            name: "t".into(),
            line: 2,
            end_line: 7,
        }];
        session.current_line = 4; // inside closure
        session.closures = closures;

        let frames = super::build_stack_frames(&session);
        assert!(frames.len() >= 2);
        assert_eq!(frames[0].get("name"), Some(&json!("closure#1")));
        assert_eq!(frames[1].get("name"), Some(&json!('@'.to_string() + "test t")));
    }
}
