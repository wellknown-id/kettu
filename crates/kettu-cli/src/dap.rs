use gimli::{self, constants, DwarfSections, EndianSlice, LittleEndian, Reader};
use serde_json::{json, Value};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use wasmparser::{Parser, Payload};
use wasmtime::{Engine, Linker, Module, Store};

#[derive(Clone, Debug)]
struct SourcePosition {
    line: i64,
    column: i64,
}

impl SourcePosition {
    fn new(line: i64, column: i64) -> Self {
        Self {
            line: line.max(1),
            column: column.max(1),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct BreakpointLocation {
    line: i64,
    column: Option<i64>,
}

impl BreakpointLocation {
    fn new(line: i64, column: Option<i64>) -> Self {
        Self {
            line: line.max(1),
            column: column.map(|value| value.max(1)),
        }
    }

    fn matches(&self, position: &SourcePosition) -> bool {
        self.line == position.line
            && self
                .column
                .map(|column| column == position.column)
                .unwrap_or(true)
    }
}

#[derive(Clone, Debug)]
struct ListedTest {
    name: String,
    line: i64,
    end_line: i64,
    body: Vec<kettu_parser::Statement>,
    trace: Vec<TraceEvent>,
}

#[derive(Clone, Debug)]
struct Variable {
    name: String,
    value: String,
    var_type: String,
    variables_reference: i64,
    raw_value: SimpleValue,
}

#[derive(Clone, Debug)]
struct TraceEvent {
    line: i64,
    column: i64,
    env_before: HashMap<String, SimpleValue>,
    runtime_subprogram_start_line: Option<i64>,
    runtime_locals: HashMap<u32, i64>,
    runtime_pointer_derefs: HashMap<u32, PointerDeref>,
    runtime_closure_keys: Vec<i64>,
}

#[derive(Clone, Debug)]
struct ClosureRange {
    debug_key: i64,
    start_line: i64,
    start_column: i64,
    end_line: i64,
    name: String,
    params: Vec<String>,
    captures: Vec<String>,
    body: kettu_parser::Expr,
    inline_invocation_line: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
enum SimpleValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    List(Vec<SimpleValue>),
    Record(BTreeMap<String, SimpleValue>),
    Result {
        discriminant: i32,
        payload: i32,
        payload_value: Option<Box<SimpleValue>>,
        err_message: Option<String>,
    },
    Unknown(String),
}

impl SimpleValue {
    fn display(&self) -> String {
        match self {
            Self::Bool(value) => value.to_string(),
            Self::Int(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::String(value) => format!("\"{}\"", value),
            Self::List(values) => {
                let rendered = values
                    .iter()
                    .map(SimpleValue::display)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{}]", rendered)
            }
            Self::Record(fields) => {
                let rendered = fields
                    .iter()
                    .map(|(name, value)| format!("{}: {}", name, value.display()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{ {} }}", rendered)
            }
            Self::Result {
                discriminant,
                payload,
                payload_value,
                err_message,
            } => {
                let case_name = variant_case_name(*discriminant);
                match variant_payload_value(
                    *discriminant,
                    *payload,
                    payload_value.as_deref(),
                    err_message.as_ref(),
                ) {
                    Some(value) => format!("{case_name}({})", value.display()),
                    None => case_name,
                }
            }
            Self::Unknown(value) => value.clone(),
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            Self::Bool(_) => "bool",
            Self::Int(_) => "i64",
            Self::Float(_) => "f64",
            Self::String(_) => "string",
            Self::List(_) => "list",
            Self::Record(_) => "record",
            Self::Result { .. } => "result",
            Self::Unknown(_) => "unknown",
        }
    }
}

fn variant_discriminant(case_name: &str) -> i32 {
    case_name
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_add(b as u32)) as i32
}

fn variant_case_name(discriminant: i32) -> String {
    for case_name in ["ok", "err", "some", "none"] {
        if discriminant == variant_discriminant(case_name) {
            return case_name.to_string();
        }
    }
    format!("variant#{discriminant}")
}

fn simple_value_to_i32(value: &SimpleValue) -> Option<i32> {
    match value {
        SimpleValue::Bool(value) => Some(if *value { 1 } else { 0 }),
        SimpleValue::Int(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn variant_payload_value(
    discriminant: i32,
    payload: i32,
    payload_value: Option<&SimpleValue>,
    err_message: Option<&String>,
) -> Option<SimpleValue> {
    if let Some(value) = payload_value {
        return Some(value.clone());
    }

    match variant_case_name(discriminant).as_str() {
        "none" => None,
        "err" => err_message
            .cloned()
            .map(SimpleValue::String)
            .or(Some(SimpleValue::Int(i64::from(payload)))),
        _ => Some(SimpleValue::Int(i64::from(payload))),
    }
}

fn result_child_variables(
    discriminant: i32,
    payload: i32,
    payload_value: Option<&SimpleValue>,
    err_message: Option<&String>,
) -> Vec<Variable> {
    let case_name = variant_case_name(discriminant);
    let mut vars = vec![Variable::from_value(
        "case",
        SimpleValue::String(case_name.clone()),
    )];

    if let Some(value) = variant_payload_value(discriminant, payload, payload_value, err_message) {
        vars.push(Variable::from_value("payload", value));
    }

    vars
}

fn simple_variant_value(case_name: &str, payload: Option<SimpleValue>) -> SimpleValue {
    let discriminant = variant_discriminant(case_name);
    let payload_int = payload
        .as_ref()
        .and_then(simple_value_to_i32)
        .unwrap_or_default();
    let err_message = if case_name == "err" {
        match payload.as_ref() {
            Some(SimpleValue::String(message)) => Some(message.clone()),
            _ => None,
        }
    } else {
        None
    };

    SimpleValue::Result {
        discriminant,
        payload: payload_int,
        payload_value: payload.map(Box::new),
        err_message,
    }
}

fn variant_pattern_binding_value(value: &SimpleValue) -> Option<SimpleValue> {
    match value {
        SimpleValue::Result {
            discriminant,
            payload,
            payload_value,
            err_message,
        } => variant_payload_value(
            *discriminant,
            *payload,
            payload_value.as_deref(),
            err_message.as_ref(),
        ),
        _ => None,
    }
}

impl Variable {
    fn from_value(name: impl Into<String>, value: SimpleValue) -> Self {
        let display = value.display();
        let type_name = value.type_name().to_string();
        Self {
            name: name.into(),
            value: display,
            var_type: type_name,
            variables_reference: 0,
            raw_value: value,
        }
    }
}

#[derive(Clone, Debug)]
struct ActiveClosure {
    closure_index: usize,
    resume_line_after_closure: Option<i64>,
    resume_trace_index_after_closure: Option<usize>,
    param_bindings: HashMap<String, SimpleValue>,
    capture_bindings: HashMap<String, SimpleValue>,
}

#[derive(Clone, Debug)]
struct DebugSymbol {
    start_line: i64,
    end_line: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DwarfBindingKind {
    Parameter,
    Variable,
}

#[derive(Clone, Debug)]
struct DwarfBinding {
    name: String,
    kind: DwarfBindingKind,
    decl_line: i64,
    local_index: Option<u32>,
}

#[derive(Clone, Debug)]
struct DwarfSubprogram {
    name: String,
    start_line: i64,
    end_line: i64,
    bindings: Vec<DwarfBinding>,
}

#[derive(Clone, Debug, Default)]
struct DebugSymbols {
    functions: HashMap<String, DebugSymbol>,
    lambdas: Vec<DebugSymbol>,
    subprograms: Vec<DwarfSubprogram>,
}

#[derive(Clone, Debug)]
struct DwarfLineRow {
    address: u64,
    line: i64,
}

#[derive(Clone, Debug, Default)]
struct PointerDeref {
    discriminant: i32,
    payload: i32,
    err_string: Option<String>,
}

#[derive(Default)]
struct RuntimeTraceState {
    events: Vec<RuntimeTraceEvent>,
    pending_locals: HashMap<u32, i64>,
    pending_derefs: HashMap<u32, PointerDeref>,
    active_closure_keys: Vec<i64>,
    pause_mode: RuntimePauseMode,
    pause_event: Option<RuntimePauseEvent>,
}

#[derive(Clone, Debug)]
struct RuntimeTraceEvent {
    line: i64,
    column: i64,
    subprogram_start_line: Option<i64>,
    locals: HashMap<u32, i64>,
    pointer_derefs: HashMap<u32, PointerDeref>,
    active_closure_keys: Vec<i64>,
}

#[derive(Clone, Debug, Default)]
struct PausedRuntimeSnapshot {
    subprogram_start_line: Option<i64>,
    locals: HashMap<u32, i64>,
    pointer_derefs: HashMap<u32, PointerDeref>,
    closure_keys: Vec<i64>,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug)]
struct RuntimePauseEvent {
    line: i64,
    column: i64,
    reason: &'static str,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug, Default)]
enum RuntimePauseMode {
    #[default]
    ContinueToEnd,
    Breakpoints(BTreeSet<BreakpointLocation>),
}

impl RuntimePauseMode {
    fn should_pause(&self, position: &SourcePosition) -> bool {
        match self {
            Self::ContinueToEnd => false,
            Self::Breakpoints(breakpoints) => breakpoints
                .iter()
                .any(|breakpoint| breakpoint.matches(position)),
        }
    }
}

struct RuntimeDebugArtifacts {
    engine: Engine,
    module: Module,
    #[cfg_attr(not(test), allow(dead_code))]
    debug_resume_frames: Vec<DebugResumeFrameMetadata>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
struct DebugResumeFrameMetadata {
    func_index: u32,
    name: String,
    start_line: i64,
    state_offset: u32,
    byte_size: u32,
    slot_count: usize,
    bindings: Vec<DebugResumeBindingMetadata>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
struct DebugResumeBindingMetadata {
    name: String,
    local_index: u32,
    decl_line: i64,
    kind: String,
}

impl RuntimeDebugArtifacts {
    fn compile(
        program: &PathBuf,
        source: &str,
        ast: &kettu_parser::WitFile,
    ) -> Result<Self, String> {
        let wasm = compile_debug_runtime_module(program, source, ast)?;
        let debug_resume_frames = parse_debug_resume_frames(&wasm)?;
        let engine = Engine::default();
        let module = Module::new(&engine, &wasm)
            .map_err(|err| format!("Failed to load debug runtime wasm: {}", err))?;

        Ok(Self {
            engine,
            module,
            debug_resume_frames,
        })
    }

    fn start_test(&self, test_name: &str) -> Result<RuntimeDebugSession, String> {
        RuntimeDebugSession::start(&self.engine, &self.module, test_name)
    }

    fn collect_trace_for_test(&self, test_name: &str) -> Result<Vec<RuntimeTraceEvent>, String> {
        let mut session = self.start_test(test_name)?;
        session.run_to_completion()?;
        Ok(session.events())
    }
}

fn parse_debug_resume_frames(wasm: &[u8]) -> Result<Vec<DebugResumeFrameMetadata>, String> {
    let mut section_data = None;
    for payload in Parser::new(0).parse_all(wasm) {
        match payload.map_err(|err| format!("Failed to parse runtime debug wasm: {}", err))? {
            Payload::CustomSection(section) if section.name() == "kettu-debug-resume" => {
                section_data = Some(section.data().to_vec());
                break;
            }
            _ => {}
        }
    }

    let Some(section_data) = section_data else {
        return Ok(Vec::new());
    };

    let json: Value = serde_json::from_slice(&section_data)
        .map_err(|err| format!("Failed to parse debug resume metadata: {}", err))?;
    let Some(frames) = json.get("frames").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    frames.iter().map(parse_debug_resume_frame).collect()
}

fn parse_debug_resume_frame(frame: &Value) -> Result<DebugResumeFrameMetadata, String> {
    let bindings = frame
        .get("bindings")
        .and_then(Value::as_array)
        .ok_or_else(|| "Debug resume frame is missing bindings".to_string())?
        .iter()
        .map(parse_debug_resume_binding)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(DebugResumeFrameMetadata {
        func_index: value_as_u32(frame, "funcIndex")?,
        name: value_as_string(frame, "name")?,
        start_line: value_as_i64(frame, "startLine")?,
        state_offset: value_as_u32(frame, "stateOffset")?,
        byte_size: value_as_u32(frame, "byteSize")?,
        slot_count: value_as_usize(frame, "slotCount")?,
        bindings,
    })
}

fn parse_debug_resume_binding(binding: &Value) -> Result<DebugResumeBindingMetadata, String> {
    Ok(DebugResumeBindingMetadata {
        name: value_as_string(binding, "name")?,
        local_index: value_as_u32(binding, "localIndex")?,
        decl_line: value_as_i64(binding, "declLine")?,
        kind: value_as_string(binding, "kind")?,
    })
}

fn value_as_string(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| format!("Debug resume metadata is missing string field '{}'", key))
}

fn value_as_u32(value: &Value, key: &str) -> Result<u32, String> {
    let raw = value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("Debug resume metadata is missing numeric field '{}'", key))?;
    u32::try_from(raw)
        .map_err(|_| format!("Debug resume metadata field '{}' exceeds u32", key))
}

fn value_as_usize(value: &Value, key: &str) -> Result<usize, String> {
    let raw = value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("Debug resume metadata is missing numeric field '{}'", key))?;
    usize::try_from(raw)
        .map_err(|_| format!("Debug resume metadata field '{}' exceeds usize", key))
}

fn value_as_i64(value: &Value, key: &str) -> Result<i64, String> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("Debug resume metadata is missing signed numeric field '{}'", key))
}

struct RuntimeDebugSession {
    store: Store<RuntimeTraceState>,
    instance: wasmtime::Instance,
    export_name: String,
}

const DEBUG_PAUSE_TRAP_MESSAGE: &str = "debug pause";

enum RuntimeExecutionOutcome {
    Completed,
    Paused(RuntimePauseEvent),
    Fault(String),
}

impl RuntimeDebugSession {
    fn start(engine: &Engine, module: &Module, test_name: &str) -> Result<Self, String> {
        let mut linker = Linker::new(engine);
        wire_runtime_debug_imports(&mut linker)?;

        let mut store = Store::new(engine, RuntimeTraceState::default());
        let instance = linker
            .instantiate(&mut store, module)
            .map_err(|err| format!("Failed to instantiate runtime debug module: {}", err))?;
        let export_name = find_test_export_name(&mut store, &instance, test_name)
            .ok_or_else(|| format!("Failed to find test export for '{}'", test_name))?;

        Ok(Self {
            store,
            instance,
            export_name,
        })
    }

    fn run_to_completion(&mut self) -> Result<(), String> {
        self.store.data_mut().pause_mode = RuntimePauseMode::ContinueToEnd;
        self.store.data_mut().pause_event = None;
        match self.execute_test() {
            RuntimeExecutionOutcome::Completed => Ok(()),
            RuntimeExecutionOutcome::Paused(pause) => Err(format!(
                "Runtime unexpectedly paused at {}:{} while collecting trace",
                pause.line, pause.column
            )),
            RuntimeExecutionOutcome::Fault(err) => Err(err),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn run_until_pause(
        &mut self,
        breakpoints: BTreeSet<BreakpointLocation>,
    ) -> Result<Option<RuntimePauseEvent>, String> {
        self.store.data_mut().pause_mode = RuntimePauseMode::Breakpoints(breakpoints);
        self.store.data_mut().pause_event = None;

        match self.execute_test() {
            RuntimeExecutionOutcome::Completed => Ok(None),
            RuntimeExecutionOutcome::Paused(pause) => Ok(Some(pause)),
            RuntimeExecutionOutcome::Fault(err) => Err(err),
        }
    }

    fn execute_test(&mut self) -> RuntimeExecutionOutcome {
        let test_func = self
            .instance
            .get_func(&mut self.store, &self.export_name)
            .ok_or_else(|| format!("Failed to load test export '{}'", self.export_name));

        let test_func = match test_func {
            Ok(test_func) => test_func,
            Err(err) => return RuntimeExecutionOutcome::Fault(err),
        };

        let ty = test_func.ty(&self.store);
        let mut results = vec![wasmtime::Val::I32(0); ty.results().len()];
        match test_func.call(&mut self.store, &[], &mut results) {
            Ok(()) => RuntimeExecutionOutcome::Completed,
            Err(err) => self.classify_runtime_trap(err),
        }
    }

    fn classify_runtime_trap(&self, err: wasmtime::Error) -> RuntimeExecutionOutcome {
        let error_chain = format_runtime_error_chain(&err);
        if self.store.data().pause_event.is_some() && error_chain.contains(DEBUG_PAUSE_TRAP_MESSAGE) {
            return RuntimeExecutionOutcome::Paused(
                self.store
                    .data()
                    .pause_event
                    .clone()
                    .expect("pause event checked above"),
            );
        }

        RuntimeExecutionOutcome::Fault(format!(
            "Failed to execute test export '{}': {}",
            self.export_name, error_chain
        ))
    }

    fn events(&self) -> Vec<RuntimeTraceEvent> {
        self.store.data().events.clone()
    }

    fn latest_event(&self) -> Option<&RuntimeTraceEvent> {
        self.store.data().events.last()
    }
}

fn format_runtime_error_chain(err: &wasmtime::Error) -> String {
    let mut chain = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        chain.push_str(&format!("\n  caused by: {}", cause));
        source = cause.source();
    }
    chain
}

#[derive(Clone, Copy, Debug)]
enum FrameTarget {
    Test,
    Closure(usize),
    Function(usize),
}

#[derive(Clone, Debug)]
struct FrameDescriptor {
    id: i64,
    name: String,
    line: i64,
    column: i64,
    target: FrameTarget,
}

struct ProgramState {
    source_text: String,
    source_lines: Vec<String>,
    tests: Vec<ListedTest>,
    closures: Vec<ClosureRange>,
    debug_symbols: DebugSymbols,
    runtime_debug_artifacts: RuntimeDebugArtifacts,
}

struct DebugSession {
    program: Option<PathBuf>,
    cwd: Option<PathBuf>,
    source_text: String,
    source_lines: Vec<String>,
    tests: Vec<ListedTest>,
    closures: Vec<ClosureRange>,
    debug_symbols: DebugSymbols,
    stop_on_entry: bool,
    enable_evaluate: bool,
    configured: bool,
    terminated: bool,
    current_test: usize,
    current_trace_index: Option<usize>,
    current_line: i64,
    current_column: i64,
    breakpoints: HashMap<String, BTreeSet<BreakpointLocation>>,
    active_closures: Vec<ActiveClosure>,
    child_variables: HashMap<i64, Vec<Variable>>,
    next_child_variables_reference: i64,
    runtime_debug_artifacts: Option<RuntimeDebugArtifacts>,
    active_runtime_session: Option<RuntimeDebugSession>,
    runtime_pause_event: Option<RuntimePauseEvent>,
    paused_frames: Vec<FrameDescriptor>,
}

impl DebugSession {
    fn new() -> Self {
        Self {
            program: None,
            cwd: None,
            source_text: String::new(),
            source_lines: Vec::new(),
            tests: Vec::new(),
            closures: Vec::new(),
            debug_symbols: DebugSymbols::default(),
            stop_on_entry: false,
            enable_evaluate: false,
            configured: false,
            terminated: false,
            current_test: 0,
            current_trace_index: None,
            current_line: 0,
            current_column: 1,
            breakpoints: HashMap::new(),
            active_closures: Vec::new(),
            child_variables: HashMap::new(),
            next_child_variables_reference: 1_000_000,
            runtime_debug_artifacts: None,
            active_runtime_session: None,
            runtime_pause_event: None,
            paused_frames: Vec::new(),
        }
    }

    fn reset_child_variables(&mut self) {
        self.child_variables.clear();
        self.next_child_variables_reference = 1_000_000;
    }

    fn reset_runtime_pause_state(&mut self) {
        self.active_runtime_session = None;
        self.runtime_pause_event = None;
    }

    fn clear_paused_frames(&mut self) {
        self.paused_frames.clear();
    }

    fn capture_paused_frames(&mut self) {
        self.paused_frames = collect_stack_frames(self);
    }

    fn current_frame_descriptors(&self) -> Vec<FrameDescriptor> {
        if self.paused_frames.is_empty() {
            collect_stack_frames(self)
        } else {
            self.paused_frames.clone()
        }
    }

    fn start_runtime_session_for_test(&mut self, test_index: usize) -> Result<bool, String> {
        let Some(runtime_debug_artifacts) = self.runtime_debug_artifacts.as_ref() else {
            return Ok(false);
        };
        let Some(test) = self.tests.get(test_index) else {
            return Ok(false);
        };

        let session = runtime_debug_artifacts.start_test(&test.name)?;
        self.active_runtime_session = Some(session);
        self.runtime_pause_event = None;
        Ok(true)
    }

    fn run_active_runtime_until_pause(&mut self) -> Result<Option<RuntimePauseEvent>, String> {
        let breakpoints = self.current_file_breakpoints();
        let Some(runtime_session) = self.active_runtime_session.as_mut() else {
            return Ok(None);
        };

        let pause = runtime_session.run_until_pause(breakpoints)?;
        self.runtime_pause_event = pause.clone();
        if let Some(pause) = &self.runtime_pause_event {
            self.current_line = pause.line;
            self.current_column = pause.column;
        }
        Ok(pause)
    }

    fn current_file_breakpoints(&self) -> BTreeSet<BreakpointLocation> {
        let Some(file) = self.current_file_key() else {
            return BTreeSet::new();
        };
        self.breakpoints.get(&file).cloned().unwrap_or_default()
    }

    fn current_file_has_column_breakpoints(&self) -> bool {
        self.current_file_breakpoints()
            .iter()
            .any(|breakpoint| breakpoint.column.is_some())
    }

    fn can_continue_with_runtime(&self) -> bool {
        self.tests
            .get(self.current_test)
            .map(|test| {
                self.current_trace_index.is_none()
                    && self.current_line < test.line
                    && !self.current_file_has_column_breakpoints()
            })
            .unwrap_or(false)
    }

    fn sync_trace_cursor_to_current_position(&mut self) {
        let best_match = self.tests.get(self.current_test).and_then(|test| {
            test.trace
                .iter()
                .enumerate()
                .filter(|(_, entry)| entry.line == self.current_line)
                .min_by_key(|(_, entry)| ((entry.column - self.current_column).abs(), entry.column))
                .map(|(index, entry)| (index, entry.column))
        });

        if let Some((index, column)) = best_match {
            self.current_trace_index = Some(index);
            self.current_column = column;
        } else {
            self.current_trace_index = None;
        }

        self.sync_active_closures_with_current_line();
    }

    fn run_live_runtime_until_breakpoint_or_end(&mut self) -> Option<StopOutcome> {
        if !self.can_continue_with_runtime() {
            return None;
        }

        loop {
            if self.current_test >= self.tests.len() {
                return Some(StopOutcome::Terminated);
            }

            if self.active_runtime_session.is_none() {
                match self.start_runtime_session_for_test(self.current_test) {
                    Ok(true) => {}
                    Ok(false) | Err(_) => {
                        self.reset_runtime_pause_state();
                        return None;
                    }
                }
            }

            match self.run_active_runtime_until_pause() {
                Ok(Some(_pause)) => {
                    self.sync_trace_cursor_to_current_position();
                    return Some(StopOutcome::Stopped("breakpoint"));
                }
                Ok(None) => {
                    self.reset_runtime_pause_state();
                    self.current_trace_index = None;
                    self.active_closures.clear();
                    self.current_test += 1;
                    if self.current_test >= self.tests.len() {
                        return Some(StopOutcome::Terminated);
                    }
                    self.current_line = self.tests[self.current_test].line - 1;
                    self.current_column = 1;
                }
                Err(_) => {
                    self.reset_runtime_pause_state();
                    return None;
                }
            }
        }
    }

    fn allocate_child_variables_reference(&mut self, vars: Vec<Variable>) -> i64 {
        let reference = self.next_child_variables_reference;
        self.next_child_variables_reference += 1;
        self.child_variables.insert(reference, vars);
        reference
    }

    fn attach_child_references(&mut self, vars: Vec<Variable>) -> Vec<Variable> {
        vars.into_iter()
            .map(|var| self.attach_child_reference(var))
            .collect()
    }

    fn attach_child_reference(&mut self, mut var: Variable) -> Variable {
        let child_vars = match &var.raw_value {
            SimpleValue::List(values) => values
                .iter()
                .enumerate()
                .map(|(index, value)| Variable::from_value(index.to_string(), value.clone()))
                .collect::<Vec<_>>(),
            SimpleValue::Record(fields) => fields
                .iter()
                .map(|(name, value)| Variable::from_value(name.clone(), value.clone()))
                .collect::<Vec<_>>(),
            SimpleValue::Result {
                discriminant,
                payload,
                payload_value,
                err_message,
            } => result_child_variables(
                *discriminant,
                *payload,
                payload_value.as_deref(),
                err_message.as_ref(),
            ),
            _ => Vec::new(),
        };

        if !child_vars.is_empty() {
            let child_vars = self.attach_child_references(child_vars);
            var.variables_reference = self.allocate_child_variables_reference(child_vars);
        }
        var
    }

    fn has_tests(&self) -> bool {
        !self.tests.is_empty()
    }

    fn current_file_key(&self) -> Option<String> {
        self.program
            .as_ref()
            .map(|p| normalize_path_key(&p.display().to_string()))
    }

    fn snapped_breakpoint_position(&self, breakpoint: &BreakpointLocation) -> Option<SourcePosition> {
        let column = breakpoint.column?;
        self.closures
            .iter()
            .filter(|closure| {
                closure.start_line == breakpoint.line
                    && closure.inline_invocation_line == Some(breakpoint.line)
            })
            .min_by_key(|closure| {
                let distance = (closure.start_column - column).abs();
                (distance, closure.start_column)
            })
            .map(|closure| SourcePosition::new(closure.start_line, closure.start_column))
    }

    fn matched_breakpoint_position(&self, position: &SourcePosition) -> Option<SourcePosition> {
        let Some(file) = self.current_file_key() else {
            return None;
        };
        self.breakpoints
            .get(&file)
            .and_then(|set| {
                set.iter().find_map(|breakpoint| {
                    if breakpoint.matches(position) {
                        Some(position.clone())
                    } else if breakpoint.line == position.line {
                        self.snapped_breakpoint_position(breakpoint)
                    } else {
                        None
                    }
                })
            })
    }

    fn breakpoint_hit(&mut self, position: &SourcePosition) -> bool {
        if let Some(position) = self.matched_breakpoint_position(position) {
            self.current_line = position.line;
            self.current_column = position.column;
            true
        } else {
            false
        }
    }

    fn current_position(&self) -> SourcePosition {
        SourcePosition::new(self.current_line, self.current_column)
    }

    fn active_test(&self) -> Option<&ListedTest> {
        self.tests
            .iter()
            .find(|t| self.current_line >= t.line && self.current_line <= t.end_line)
            .or_else(|| self.tests.get(self.current_test))
    }

    fn active_closure_indices(&self) -> Vec<usize> {
        let mut indices = Vec::new();

        if let Some(snapshot) = paused_runtime_snapshot(self) {
            for closure_key in &snapshot.closure_keys {
                if let Some(index) = self
                    .closures
                    .iter()
                    .position(|closure| closure.debug_key == *closure_key)
                {
                    if !indices.contains(&index) {
                        indices.push(index);
                    }
                }
            }
        }

        for index in self
            .active_closures
            .iter()
            .map(|closure| closure.closure_index)
        {
            if !indices.contains(&index) {
                indices.push(index);
            }
        }

        if !indices.is_empty() {
            return indices;
        }

        let mut derived: Vec<usize> = self
            .closures
            .iter()
            .enumerate()
            .filter(|(_, closure)| {
                self.current_line > closure.start_line && self.current_line <= closure.end_line
            })
            .map(|(index, _)| index)
            .collect();

        derived.sort_by_key(|index| {
            (
                self.closures[*index].start_line,
                Reverse(self.closures[*index].end_line),
            )
        });

        for index in derived {
            if !indices.contains(&index) {
                indices.push(index);
            }
        }

        indices
    }

    fn find_active_closure_state(&self, closure_index: usize) -> Option<&ActiveClosure> {
        self.active_closures
            .iter()
            .find(|closure| closure.closure_index == closure_index)
    }

    fn sync_active_closures_with_current_line(&mut self) {
        while let Some(active) = self.active_closures.last() {
            let closure = &self.closures[active.closure_index];
            if self.current_line < closure.start_line || self.current_line > closure.end_line {
                self.active_closures.pop();
            } else {
                break;
            }
        }
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
            if !test.trace.is_empty() {
                let next_index = self.current_trace_index.map_or(0, |index| index + 1);
                if let Some(entry) = test.trace.get(next_index) {
                    self.current_trace_index = Some(next_index);
                    self.current_line = entry.line;
                    self.current_column = entry.column;
                    self.sync_active_closures_with_current_line();
                    return true;
                }

                self.current_test += 1;
                self.current_trace_index = None;
                if self.current_test >= self.tests.len() {
                    return false;
                }
                self.current_line = self.tests[self.current_test].line - 1;
                self.current_column = 1;
                continue;
            }

            if self.current_line < test.line {
                self.current_line = test.line;
                self.current_column = 1;
                return true;
            }

            if self.current_line < test.end_line {
                self.current_line += 1;
                self.current_column = 1;
                return true;
            }

            self.current_test += 1;
            if self.current_test >= self.tests.len() {
                return false;
            }

            let next_start = self.tests[self.current_test].line;
            self.current_line = next_start - 1;
            self.current_column = 1;
        }
    }

    fn run_until_breakpoint_or_end(&mut self) -> StopOutcome {
        while self.advance_one_line() {
            if self.breakpoint_hit(&self.current_position()) {
                return StopOutcome::Stopped("breakpoint");
            }
        }
        StopOutcome::Terminated
    }

    fn step_once_or_end(&mut self, action: &str) -> StopOutcome {
        if action == "stepOut" && self.step_out_of_closure() {
            if self.breakpoint_hit(&self.current_position()) {
                return StopOutcome::Stopped("breakpoint");
            }
            return StopOutcome::Stopped("step");
        }

        if action == "stepIn" {
            if let Some((closure_index, resume_line, param_bindings, capture_bindings)) =
                find_invoked_closure(self, self.current_line)
            {
                let closure = &self.closures[closure_index];
                let enter_in_place = self.current_line >= closure.start_line
                    && self.current_line <= closure.end_line;
                self.active_closures.push(ActiveClosure {
                    closure_index,
                    resume_line_after_closure: Some(resume_line),
                    resume_trace_index_after_closure: enter_in_place
                        .then(|| self.current_trace_index.map(|index| index + 1))
                        .flatten(),
                    param_bindings,
                    capture_bindings,
                });
                if enter_in_place {
                    self.current_column = closure.start_column;
                    return StopOutcome::Stopped("step");
                }
                if self.advance_one_line() {
                    if self.breakpoint_hit(&self.current_position()) {
                        return StopOutcome::Stopped("breakpoint");
                    }
                    return StopOutcome::Stopped("step");
                }
                return StopOutcome::Terminated;
            }
        }

        if self.advance_one_line() {
            if self.breakpoint_hit(&self.current_position()) {
                StopOutcome::Stopped("breakpoint")
            } else {
                StopOutcome::Stopped("step")
            }
        } else {
            StopOutcome::Terminated
        }
    }

    fn step_out_of_closure(&mut self) -> bool {
        if let Some(active) = self.active_closures.pop() {
            if let Some(resume_index) = active.resume_trace_index_after_closure {
                if let Some(test) = self.tests.get(self.current_test) {
                    if let Some(entry) = test.trace.get(resume_index) {
                        self.current_trace_index = Some(resume_index);
                        self.current_line = entry.line;
                        self.sync_active_closures_with_current_line();
                        return true;
                    }
                }
            }
            let resume = active
                .resume_line_after_closure
                .unwrap_or(self.current_line + 1);
            if let Some(line) = self.first_trace_line_at_or_after(resume) {
                self.current_line = line;
            } else {
                self.current_line = resume;
            }
            self.sync_active_closures_with_current_line();
            return true;
        }

        if let Some(closure_index) = self.active_closure_indices().last().copied() {
            if let Some(line) =
                self.first_trace_line_at_or_after(self.closures[closure_index].end_line + 1)
            {
                self.current_line = line;
            } else {
                self.current_line = self.closures[closure_index].end_line + 1;
            }
            self.sync_active_closures_with_current_line();
            return true;
        }

        false
    }

    fn first_trace_line_at_or_after(&mut self, line: i64) -> Option<i64> {
        let Some(test) = self.tests.get(self.current_test) else {
            self.current_trace_index = None;
            return None;
        };

        let Some((index, entry)) = test
            .trace
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.line >= line)
        else {
            self.current_trace_index = None;
            return None;
        };

        self.current_trace_index = Some(index);
        Some(entry.line)
    }
}

enum StopOutcome {
    Stopped(&'static str),
    Terminated,
}

enum StatementFlow {
    Continue,
    Return(Option<SimpleValue>),
    Break,
    ContinueLoop,
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
                        "supportsColumnBreakpoints": true,
                        "supportsStepInTargetsRequest": false,
                        "supportsEvaluateForHovers": true,
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
                let enable_evaluate = arguments
                    .get("enableEvaluate")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);

                match load_program_state(program.clone()) {
                    Ok(program_state) => {
                        session.program = program;
                        session.cwd = cwd;
                        session.source_text = program_state.source_text;
                        session.source_lines = program_state.source_lines;
                        session.tests = program_state.tests;
                        session.closures = program_state.closures;
                        session.debug_symbols = program_state.debug_symbols;
                        session.runtime_debug_artifacts = Some(program_state.runtime_debug_artifacts);
                        session.stop_on_entry = stop_on_entry;
                        session.enable_evaluate = enable_evaluate;
                        session.configured = false;
                        session.terminated = false;
                        session.current_test = 0;
                        session.current_trace_index = None;
                        session.current_line =
                            session.tests.first().map(|t| t.line - 1).unwrap_or(0);
                        session.current_column = 1;
                        session.active_closures.clear();
                        session.reset_child_variables();
                        session.reset_runtime_pause_state();
                        session.clear_paused_frames();

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

                let mut locations = BTreeSet::new();
                let mut response_bps = Vec::new();

                if let Some(req_bps) = arguments.get("breakpoints").and_then(Value::as_array) {
                    for bp in req_bps {
                        let line = bp.get("line").and_then(Value::as_i64).unwrap_or(1).max(1);
                        let column = bp
                            .get("column")
                            .and_then(Value::as_i64)
                            .map(|value| value.max(1));
                        locations.insert(BreakpointLocation::new(line, column));
                        let mut response = json!({
                            "verified": true,
                            "line": line
                        });
                        if let Some(column) = column {
                            response["column"] = json!(column);
                        }
                        response_bps.push(response);
                    }
                }

                session.breakpoints.insert(key, locations);
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
                    session.clear_paused_frames();
                    if session.advance_one_line() {
                        session.capture_paused_frames();
                        send_stopped_event(&mut writer, "entry")?;
                    } else {
                        session.terminated = true;
                        session.clear_paused_frames();
                        send_event(&mut writer, "terminated", Some(json!({})))?;
                    }
                } else {
                    session.clear_paused_frames();
                    let outcome = if let Some(outcome) = session.run_live_runtime_until_breakpoint_or_end() {
                        outcome
                    } else {
                        session.reset_runtime_pause_state();
                        session.run_until_breakpoint_or_end()
                    };
                    match outcome {
                        StopOutcome::Stopped(reason) => {
                            session.capture_paused_frames();
                            send_stopped_event(&mut writer, reason)?;
                        }
                        StopOutcome::Terminated => {
                            session.terminated = true;
                            session.clear_paused_frames();
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
                let frame_id = arguments
                    .get("frameId")
                    .and_then(Value::as_i64)
                    .unwrap_or(1);
                send_response(
                    &mut writer,
                    seq,
                    command,
                    true,
                    Some(json!({ "scopes": build_scopes(&session, frame_id) })),
                    None,
                )?;
            }
            "variables" => {
                let variables_reference = arguments
                    .get("variablesReference")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let vars: Vec<Value> = variables_for_reference(&mut session, variables_reference)
                    .into_iter()
                    .map(|v| {
                        json!({
                            "name": v.name,
                            "value": v.value,
                            "type": v.var_type,
                            "variablesReference": v.variables_reference
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
            "evaluate" => {
                let expression = arguments
                    .get("expression")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let frame_id = arguments.get("frameId").and_then(Value::as_i64);

                if !session.enable_evaluate {
                    send_response(
                        &mut writer,
                        seq,
                        command,
                        false,
                        None,
                        Some("Evaluate support is disabled for this launch. Set enableEvaluate to true.".to_string()),
                    )?;
                    continue;
                }

                match evaluate_in_frame(&session, frame_id, expression) {
                    Ok(value) => {
                        let evaluated = session.attach_child_reference(Variable::from_value(
                            expression.to_string(),
                            value,
                        ));
                        send_response(
                            &mut writer,
                            seq,
                            command,
                            true,
                            Some(json!({
                                "result": evaluated.value,
                                "type": evaluated.var_type,
                                "variablesReference": evaluated.variables_reference
                            })),
                            None,
                        )?;
                    }
                    Err(err) => {
                        send_response(&mut writer, seq, command, false, None, Some(err))?;
                    }
                }
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

                session.reset_child_variables();
                session.clear_paused_frames();

                let outcome = if let Some(outcome) = session.run_live_runtime_until_breakpoint_or_end() {
                    outcome
                } else {
                    session.reset_runtime_pause_state();
                    session.run_until_breakpoint_or_end()
                };
                match outcome {
                    StopOutcome::Stopped(reason) => {
                        session.capture_paused_frames();
                        send_stopped_event(&mut writer, reason)?
                    }
                    StopOutcome::Terminated => {
                        session.terminated = true;
                        session.clear_paused_frames();
                        send_event(&mut writer, "terminated", Some(json!({})))?;
                    }
                }
            }
            "next" | "stepIn" | "stepOut" => {
                send_response(&mut writer, seq, command, true, Some(json!({})), None)?;

                if session.terminated {
                    continue;
                }

                session.reset_child_variables();
                session.reset_runtime_pause_state();
                session.clear_paused_frames();

                match session.step_once_or_end(command) {
                    StopOutcome::Stopped(reason) => {
                        session.capture_paused_frames();
                        send_stopped_event(&mut writer, reason)?
                    }
                    StopOutcome::Terminated => {
                        session.terminated = true;
                        session.clear_paused_frames();
                        send_event(&mut writer, "terminated", Some(json!({})))?;
                    }
                }
            }
            "pause" => {
                send_response(&mut writer, seq, command, true, Some(json!({})), None)?;
                if !session.terminated {
                    session.capture_paused_frames();
                    send_stopped_event(&mut writer, "pause")?;
                }
            }
            "disconnect" => {
                session.terminated = true;
                session.clear_paused_frames();
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

fn load_program_state(program: Option<PathBuf>) -> Result<ProgramState, String> {
    let Some(program) = program else {
        return Err("Missing launch argument: program".to_string());
    };

    let source = fs::read_to_string(&program)
        .map_err(|e| format!("Failed to read source file '{}': {}", program.display(), e))?;
    let (ast, parse_errors) = kettu_parser::parse_file(&source);
    if !parse_errors.is_empty() {
        let all = parse_errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("Parse error(s): {}", all));
    }

    let mut ast = ast.ok_or_else(|| "Failed to parse file".to_string())?;
    annotate_closure_captures(&mut ast);

    let mut tests = list_tests_from_ast(&ast, &source);
    let mut closures = collect_closures_from_ast(&ast, &source);
    let debug_symbols = build_debug_symbols(&program, &source, &ast)?;
    apply_debug_symbols(&mut tests, &mut closures, &debug_symbols);
    build_test_traces(&mut tests, &closures, &source);
    let runtime_debug_artifacts = RuntimeDebugArtifacts::compile(&program, &source, &ast)?;
    build_runtime_test_traces(&runtime_debug_artifacts, &mut tests)?;

    Ok(ProgramState {
        source_text: source.clone(),
        source_lines: source.lines().map(ToString::to_string).collect(),
        tests,
        closures,
        debug_symbols,
        runtime_debug_artifacts,
    })
}

fn build_runtime_test_traces(
    runtime_debug_artifacts: &RuntimeDebugArtifacts,
    tests: &mut [ListedTest],
) -> Result<(), String> {
    for test in tests {
        let simulated_trace = std::mem::take(&mut test.trace);
        let actual_events = runtime_debug_artifacts.collect_trace_for_test(&test.name)?;
        if actual_events.is_empty() {
            test.trace = simulated_trace;
            continue;
        }

        if let Some(first_line) = actual_events.first().map(|event| event.line) {
            test.line = first_line;
        }
        if let Some(max_line) = actual_events.iter().map(|event| event.line).max() {
            test.end_line = test.end_line.max(max_line);
        }
        test.trace = merge_runtime_trace(actual_events, &simulated_trace);
    }

    Ok(())
}

fn wire_runtime_debug_imports(linker: &mut Linker<RuntimeTraceState>) -> Result<(), String> {
    linker
        .func_wrap(
            "kettu:debug",
            "line",
            |mut caller: wasmtime::Caller<'_, RuntimeTraceState>,
             subprogram_start_line: i32,
             line: i32| {
                if line > 0 {
                    let state = caller.data_mut();
                    state.events.push(RuntimeTraceEvent {
                        line: line as i64,
                        column: 1,
                        subprogram_start_line: (subprogram_start_line > 0)
                            .then_some(subprogram_start_line as i64),
                        locals: std::mem::take(&mut state.pending_locals),
                        pointer_derefs: std::mem::take(&mut state.pending_derefs),
                        active_closure_keys: state.active_closure_keys.clone(),
                    });
                }
            },
        )
        .map_err(|err| format!("Failed to wire debug line hook: {}", err))?;
    linker
        .func_wrap(
            "kettu:debug",
            "should_pause",
            |caller: wasmtime::Caller<'_, RuntimeTraceState>, line: i32, column: i32| -> i32 {
                if line <= 0 {
                    return 0;
                }

                let position = SourcePosition::new(line as i64, column as i64);
                let state = caller.data();
                if state.pause_event.is_some() {
                    return 0;
                }
                if state.pause_mode.should_pause(&position) {
                    1
                } else {
                    0
                }
            },
        )
        .map_err(|err| format!("Failed to wire debug should_pause hook: {}", err))?;
    linker
        .func_wrap(
            "kettu:debug",
            "pause",
            |mut caller: wasmtime::Caller<'_, RuntimeTraceState>,
             line: i32,
             column: i32|
             -> Result<(), wasmtime::Error> {
                if line > 0 {
                    let position = SourcePosition::new(line as i64, column as i64);
                    let state = caller.data_mut();
                    state.pause_event = Some(RuntimePauseEvent {
                        line: position.line,
                        column: position.column,
                        reason: "breakpoint",
                    });
                }
                Err(wasmtime::Error::msg(DEBUG_PAUSE_TRAP_MESSAGE))
            },
        )
        .map_err(|err| format!("Failed to wire debug pause hook: {}", err))?;
    linker
        .func_wrap(
            "kettu:debug",
            "local",
            |mut caller: wasmtime::Caller<'_, RuntimeTraceState>, local_index: i32, value: i32| {
                if local_index < 0 {
                    return;
                }
                let deref = if value > 0 {
                    caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .and_then(|memory| {
                            let ptr = value as usize;
                            if ptr + 8 > memory.data_size(&caller) {
                                return None;
                            }
                            let data = memory.data(&caller);
                            let disc =
                                i32::from_le_bytes(data[ptr..ptr + 4].try_into().unwrap_or([0; 4]));
                            let payload = i32::from_le_bytes(
                                data[ptr + 4..ptr + 8].try_into().unwrap_or([0; 4]),
                            );

                            let err_disc = variant_discriminant("err");
                            let err_string = if disc == err_disc && payload > 0 {
                                let str_ptr = payload as usize;
                                let len_ptr = str_ptr.saturating_sub(4);
                                if len_ptr + 4 <= data.len() {
                                    let str_len = u32::from_le_bytes(
                                        data[len_ptr..len_ptr + 4].try_into().unwrap_or([0; 4]),
                                    ) as usize;
                                    if str_ptr + str_len <= data.len() {
                                        Some(
                                            std::str::from_utf8(&data[str_ptr..str_ptr + str_len])
                                                .unwrap_or("<invalid utf8>")
                                                .to_string(),
                                        )
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            };

                            Some(PointerDeref {
                                discriminant: disc,
                                payload,
                                err_string,
                            })
                        })
                } else {
                    None
                };
                let state = caller.data_mut();
                state.pending_locals.insert(local_index as u32, value as i64);
                if let Some(d) = deref {
                    state.pending_derefs.insert(local_index as u32, d);
                }
            },
        )
        .map_err(|err| format!("Failed to wire debug local hook: {}", err))?;
    linker
        .func_wrap(
            "kettu:debug",
            "enter",
            |mut caller: wasmtime::Caller<'_, RuntimeTraceState>, closure_key: i32| {
                caller.data_mut().active_closure_keys.push(closure_key as i64);
            },
        )
        .map_err(|err| format!("Failed to wire debug enter hook: {}", err))?;
    linker
        .func_wrap(
            "kettu:debug",
            "exit",
            |mut caller: wasmtime::Caller<'_, RuntimeTraceState>, closure_key: i32| {
                let closure_key = closure_key as i64;
                let active = &mut caller.data_mut().active_closure_keys;
                if active.last().copied() == Some(closure_key) {
                    active.pop();
                } else if let Some(index) = active.iter().rposition(|key| *key == closure_key) {
                    active.remove(index);
                }
            },
        )
        .map_err(|err| format!("Failed to wire debug exit hook: {}", err))?;

    linker
        .func_wrap("kettu:contract", "fail", |_ptr: i32, _len: i32| -> () {})
        .map_err(|err| format!("Failed to wire kettu:contract/fail: {}", err))?;

    Ok(())
}

fn compile_debug_runtime_module(
    program: &PathBuf,
    source: &str,
    ast: &kettu_parser::WitFile,
) -> Result<Vec<u8>, String> {
    let imported_asts = crate::load_imported_asts(program, ast);
    let imported_aliases: HashSet<String> = imported_asts
        .iter()
        .map(|(alias, _)| alias.clone())
        .collect();

    let diagnostics = kettu_checker::check_with_source(ast, source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| matches!(diagnostic.severity, kettu_checker::Severity::Error))
        .filter(|diagnostic| {
            if diagnostic.message.starts_with("Unknown variable: ") {
                let variable = diagnostic.message.trim_start_matches("Unknown variable: ");
                !imported_aliases.contains(variable)
            } else {
                true
            }
        })
        .map(|diagnostic| diagnostic.message.clone())
        .collect();

    if !errors.is_empty() {
        return Err(format!(
            "Failed to build runtime debug module: {}",
            errors.join("; ")
        ));
    }

    let compile_options = kettu_codegen::CompileOptions {
        core_only: true,
        memory_pages: 1,
        wasip3: false,
        threads: false,
        emit_dwarf: false,
        keep_names: true,
        debug_source: Some(source.to_string()),
        debug_path: Some(program.display().to_string()),
        emit_debug_hooks: true,
    };

    if imported_asts.is_empty() {
        kettu_codegen::build_core_module(ast, &compile_options)
            .map_err(|err| format!("Failed to build runtime debug module: {}", err))
    } else {
        let imports_refs: Vec<_> = imported_asts
            .iter()
            .map(|(alias, ast)| (alias.clone(), ast))
            .collect();
        kettu_codegen::compile_module_with_imports(ast, &imports_refs, &compile_options)
            .map_err(|err| format!("Failed to build runtime debug module: {}", err))
    }
}

fn find_test_export_name(
    store: &mut Store<RuntimeTraceState>,
    instance: &wasmtime::Instance,
    test_name: &str,
) -> Option<String> {
    instance
        .exports(store)
        .map(|export| export.name().to_string())
        .find(|name| name == test_name || name.ends_with(&format!("#{}", test_name)))
}

fn merge_runtime_trace(
    events: Vec<RuntimeTraceEvent>,
    simulated_trace: &[TraceEvent],
) -> Vec<TraceEvent> {
    let mut search_start = 0usize;

    events
        .into_iter()
        .map(|event| {
            let line = event.line;
            let (env_before, column) = simulated_trace[search_start..]
                .iter()
                .position(|entry| entry.line == line)
                .and_then(|offset| {
                    let entry = simulated_trace.get(search_start + offset)?;
                    search_start += offset + 1;
                    Some((entry.env_before.clone(), entry.column))
                })
                .or_else(|| {
                    simulated_trace
                        .iter()
                        .find(|entry| entry.line == line)
                        .map(|entry| (entry.env_before.clone(), entry.column))
                })
                .unwrap_or_else(|| (HashMap::new(), event.column));

            TraceEvent {
                line,
                column,
                env_before,
                runtime_subprogram_start_line: event.subprogram_start_line,
                runtime_locals: event.locals,
                runtime_pointer_derefs: event.pointer_derefs,
                runtime_closure_keys: event.active_closure_keys,
            }
        })
        .collect()
}

fn build_stack_frames(session: &DebugSession) -> Vec<Value> {
    let Some(program) = &session.program else {
        return vec![];
    };

    session
        .current_frame_descriptors()
        .into_iter()
        .map(|frame| {
            json!({
                "id": frame.id,
                "name": frame.name,
                "line": frame.line.max(1),
                "column": frame.column.max(1),
                "source": {
                    "name": program.file_name().and_then(|n| n.to_str()).unwrap_or("program.kettu"),
                    "path": program.display().to_string()
                }
            })
        })
        .collect()
}

fn collect_stack_frames(session: &DebugSession) -> Vec<FrameDescriptor> {
    let mut frames = Vec::new();
    let mut next_id = 1;

    for closure_index in session.active_closure_indices().into_iter().rev() {
        frames.push(FrameDescriptor {
            id: next_id,
            name: session.closures[closure_index].name.clone(),
            line: session.current_line.max(1),
            column: session.closures[closure_index].start_column.max(1),
            target: FrameTarget::Closure(closure_index),
        });
        next_id += 1;
    }

    let _test_name = session
        .active_test()
        .map(|t| t.name.clone())
        .unwrap_or_default();

    if let Some(snapshot) = paused_runtime_snapshot(session) {
        if let Some(sub_start) = snapshot.subprogram_start_line {
            if let Some(func_idx) = session.debug_symbols.subprograms.iter().position(|sp| {
                if sp.name.starts_with("lambda#") {
                    return false;
                }
                if sp.start_line != sub_start {
                    return false;
                }
                let is_test = session.active_test().map_or(false, |t| {
                    sp.name == t.name || sp.name.ends_with(&format!("#{}", t.name))
                });
                !is_test
            }) {
                let sp = &session.debug_symbols.subprograms[func_idx];
                frames.push(FrameDescriptor {
                    id: next_id,
                    name: sp.name.clone(),
                    line: session.current_line.max(1),
                    column: session.current_column.max(1),
                    target: FrameTarget::Function(func_idx),
                });
                next_id += 1;
            } else if let Some(frame) = active_runtime_debug_frame(session, &snapshot) {
                if !frame.name.starts_with("lambda#")
                    && session.active_test().map_or(true, |test| {
                        frame.name != test.name && !frame.name.ends_with(&format!("#{}", test.name))
                    })
                {
                    frames.push(FrameDescriptor {
                        id: next_id,
                        name: frame.name.clone(),
                        line: session.current_line.max(1),
                        column: session.current_column.max(1),
                        target: FrameTarget::Function(usize::MAX),
                    });
                    next_id += 1;
                }
            }
        }
    }

    frames.push(FrameDescriptor {
        id: next_id,
        name: session
            .active_test()
            .map(|t| format!("@test {}", t.name))
            .unwrap_or_else(|| "@test <unknown>".to_string()),
        line: session.current_line.max(1),
        column: session.current_column.max(1),
        target: FrameTarget::Test,
    });

    frames
}

fn resolve_frame_target(session: &DebugSession, frame_id: i64) -> Option<FrameTarget> {
    session
        .current_frame_descriptors()
        .into_iter()
        .find(|frame| frame.id == frame_id)
        .map(|frame| frame.target)
}

fn build_scopes(session: &DebugSession, frame_id: i64) -> Vec<Value> {
    let Some(target) = resolve_frame_target(session, frame_id) else {
        return Vec::new();
    };

    let mut scopes = vec![json!({
        "name": "Locals",
        "presentationHint": "locals",
        "variablesReference": locals_reference(frame_id),
        "expensive": false
    })];

    if matches!(target, FrameTarget::Closure(_)) {
        let captures = frame_capture_variables(session, frame_id);
        if !captures.is_empty() {
            scopes.push(json!({
                "name": "Captures",
                "presentationHint": "registers",
                "variablesReference": captures_reference(frame_id),
                "expensive": false
            }));
        }
    }

    scopes
}

fn locals_reference(frame_id: i64) -> i64 {
    frame_id * 10 + 1
}

fn captures_reference(frame_id: i64) -> i64 {
    frame_id * 10 + 2
}

enum ScopeReferenceKind {
    Locals,
    Captures,
}

fn decode_scope_reference(reference: i64) -> Option<(i64, ScopeReferenceKind)> {
    if reference <= 0 {
        return None;
    }

    let frame_id = reference / 10;
    match reference % 10 {
        1 => Some((frame_id, ScopeReferenceKind::Locals)),
        2 => Some((frame_id, ScopeReferenceKind::Captures)),
        _ => None,
    }
}

fn variables_for_reference(session: &mut DebugSession, reference: i64) -> Vec<Variable> {
    if let Some(vars) = session.child_variables.get(&reference).cloned() {
        return vars;
    }

    if let Some((frame_id, scope_kind)) = decode_scope_reference(reference) {
        let vars = match scope_kind {
            ScopeReferenceKind::Locals => frame_local_variables(session, frame_id),
            ScopeReferenceKind::Captures => frame_capture_variables(session, frame_id),
        };
        return session.attach_child_references(vars);
    }

    Vec::new()
}

fn evaluate_in_frame(
    session: &DebugSession,
    frame_id: Option<i64>,
    expression: &str,
) -> Result<SimpleValue, String> {
    let frame_id = frame_id.unwrap_or(1);
    let env = frame_environment(session, frame_id);
    let source = session.source_lines.join("\n");

    if let Ok(expr) = parse_eval_expression(expression) {
        let value = evaluate_ast_expr(&expr, &session.closures, &source, &env);
        if !is_placeholder_unknown(&value) {
            return Ok(value);
        }
    }

    evaluate_expression(expression, &env).or_else(|_| Ok(infer_expr_value(expression, &env)))
}

fn parse_eval_expression(expression: &str) -> Result<kettu_parser::Expr, String> {
    let wrapped = format!(
        "package local:debug;\ninterface debug {{\n    eval: func() -> bool {{\n        return {};\n    }}\n}}",
        expression.trim()
    );

    let (ast, errors) = kettu_parser::parse_file(&wrapped);
    if !errors.is_empty() {
        let joined = errors
            .into_iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(joined);
    }

    let ast = ast.ok_or_else(|| "missing eval AST".to_string())?;
    let func = ast
        .items
        .iter()
        .find_map(|item| match item {
            kettu_parser::TopLevelItem::Interface(interface) => interface.items.iter().find_map(
                |item| match item {
                    kettu_parser::InterfaceItem::Func(func) if func.name.name == "eval" => {
                        Some(func)
                    }
                    _ => None,
                },
            ),
            _ => None,
        })
        .ok_or_else(|| "missing eval function".to_string())?;
    let body = func.body.as_ref().ok_or_else(|| "missing eval body".to_string())?;

    match body.statements.first() {
        Some(kettu_parser::Statement::Return(Some(expr))) => Ok(expr.clone()),
        Some(kettu_parser::Statement::Expr(expr)) => Ok(expr.clone()),
        Some(other) => Err(format!("unexpected eval statement: {other:?}")),
        None => Err("missing eval statement".to_string()),
    }
}

fn is_placeholder_unknown(value: &SimpleValue) -> bool {
    matches!(value, SimpleValue::Unknown(text) if text.starts_with('<') && text.ends_with('>'))
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

fn offset_to_line_col(source: &str, offset: usize) -> (usize, usize) {
    let clamped = offset.min(source.len());
    let prefix = &source[..clamped];
    let line = prefix.bytes().filter(|&b| b == b'\n').count() + 1;
    let column = prefix
        .rsplit_once('\n')
        .map(|(_, tail)| tail.chars().count() + 1)
        .unwrap_or_else(|| prefix.chars().count() + 1);
    (line, column)
}

fn offset_to_position(source: &str, offset: usize) -> SourcePosition {
    let (line, column) = offset_to_line_col(source, offset);
    SourcePosition::new(line as i64, column as i64)
}

fn list_tests_from_ast(ast: &kettu_parser::WitFile, source: &str) -> Vec<ListedTest> {
    let mut listed = Vec::new();

    for item in &ast.items {
        if let kettu_parser::TopLevelItem::Interface(iface) = item {
            for iface_item in &iface.items {
                if let kettu_parser::InterfaceItem::Func(func) = iface_item {
                    let is_test = func
                        .gates
                        .iter()
                        .any(|g| matches!(g, kettu_parser::Gate::Test));
                    if !is_test {
                        continue;
                    }

                    listed.push(ListedTest {
                        name: func.name.name.clone(),
                        line: offset_to_line(source, func.span.start) as i64,
                        end_line: offset_to_line(source, func.span.end) as i64,
                        body: func
                            .body
                            .as_ref()
                            .map(|body| body.statements.clone())
                            .unwrap_or_default(),
                        trace: Vec::new(),
                    });
                }
            }
        }
    }

    listed.sort_by_key(|test| test.line);
    listed
}

fn annotate_closure_captures(ast: &mut kettu_parser::WitFile) {
    for item in &mut ast.items {
        if let kettu_parser::TopLevelItem::Interface(iface) = item {
            for iface_item in &mut iface.items {
                if let kettu_parser::InterfaceItem::Func(func) = iface_item {
                    let Some(body) = &mut func.body else {
                        continue;
                    };

                    let mut scope = HashSet::new();
                    for param in &func.params {
                        scope.insert(param.name.name.clone());
                    }

                    for statement in &mut body.statements {
                        annotate_statement_captures(statement, &mut scope);
                    }
                }
            }
        }
    }
}

fn annotate_statement_captures(
    statement: &mut kettu_parser::Statement,
    scope: &mut HashSet<String>,
) {
    match statement {
        kettu_parser::Statement::Expr(expr) => kettu_parser::capture::analyze_captures(expr, scope),
        kettu_parser::Statement::Let { name, value } => {
            kettu_parser::capture::analyze_captures(value, scope);
            scope.insert(name.name.clone());
        }
        kettu_parser::Statement::Return(Some(expr)) => {
            kettu_parser::capture::analyze_captures(expr, scope)
        }
        kettu_parser::Statement::Return(None) => {}
        kettu_parser::Statement::Assign { value, .. }
        | kettu_parser::Statement::CompoundAssign { value, .. } => {
            kettu_parser::capture::analyze_captures(value, scope);
        }
        kettu_parser::Statement::Break {
            condition: Some(expr),
        }
        | kettu_parser::Statement::Continue {
            condition: Some(expr),
        } => {
            kettu_parser::capture::analyze_captures(expr, scope);
        }
        kettu_parser::Statement::Break { condition: None }
        | kettu_parser::Statement::Continue { condition: None } => {}
        kettu_parser::Statement::SharedLet {
            name,
            initial_value,
        } => {
            kettu_parser::capture::analyze_captures(initial_value, scope);
            scope.insert(name.name.clone());
        }
        kettu_parser::Statement::GuardLet {
            name,
            value,
            else_body,
        } => {
            kettu_parser::capture::analyze_captures(value, scope);
            let mut guard_scope = scope.clone();
            for stmt in else_body {
                annotate_statement_captures(stmt, &mut guard_scope);
            }
            scope.insert(name.name.clone());
        }
        kettu_parser::Statement::Guard {
            condition,
            else_body,
        } => {
            kettu_parser::capture::analyze_captures(condition, scope);
            let mut guard_scope = scope.clone();
            for stmt in else_body {
                annotate_statement_captures(stmt, &mut guard_scope);
            }
        }
    }
}

fn collect_closures_from_ast(ast: &kettu_parser::WitFile, source: &str) -> Vec<ClosureRange> {
    let mut closures = Vec::new();

    for item in &ast.items {
        if let kettu_parser::TopLevelItem::Interface(iface) = item {
            for iface_item in &iface.items {
                if let kettu_parser::InterfaceItem::Func(func) = iface_item {
                    let is_test = func
                        .gates
                        .iter()
                        .any(|g| matches!(g, kettu_parser::Gate::Test));
                    if !is_test {
                        continue;
                    }

                    if let Some(body) = &func.body {
                        collect_closures_from_statements(&body.statements, source, &mut closures);
                    }
                }
            }
        }
    }

    closures.sort_by_key(|closure| (closure.start_line, Reverse(closure.end_line)));
    closures
}

fn collect_closures_from_statements(
    statements: &[kettu_parser::Statement],
    source: &str,
    closures: &mut Vec<ClosureRange>,
) {
    for statement in statements {
        match statement {
            kettu_parser::Statement::Expr(expr) => {
                collect_closures_from_expr(expr, source, closures, None);
            }
            kettu_parser::Statement::Let { name, value } => {
                let preferred_name =
                    matches!(value, kettu_parser::Expr::Lambda { .. }).then(|| name.name.clone());
                collect_closures_from_expr(value, source, closures, preferred_name);
            }
            kettu_parser::Statement::Return(Some(expr)) => {
                collect_closures_from_expr(expr, source, closures, None);
            }
            kettu_parser::Statement::Return(None) => {}
            kettu_parser::Statement::Assign { value, .. }
            | kettu_parser::Statement::CompoundAssign { value, .. } => {
                collect_closures_from_expr(value, source, closures, None);
            }
            kettu_parser::Statement::Break {
                condition: Some(expr),
            }
            | kettu_parser::Statement::Continue {
                condition: Some(expr),
            } => {
                collect_closures_from_expr(expr, source, closures, None);
            }
            kettu_parser::Statement::Break { condition: None }
            | kettu_parser::Statement::Continue { condition: None } => {}
            kettu_parser::Statement::SharedLet { initial_value, .. } => {
                collect_closures_from_expr(initial_value, source, closures, None);
            }
            kettu_parser::Statement::GuardLet {
                value, else_body, ..
            } => {
                collect_closures_from_expr(value, source, closures, None);
                collect_closures_from_statements(else_body, source, closures);
            }
            kettu_parser::Statement::Guard {
                condition,
                else_body,
            } => {
                collect_closures_from_expr(condition, source, closures, None);
                collect_closures_from_statements(else_body, source, closures);
            }
        }
    }
}

fn collect_closures_from_expr(
    expr: &kettu_parser::Expr,
    source: &str,
    closures: &mut Vec<ClosureRange>,
    preferred_name: Option<String>,
) {
    match expr {
        kettu_parser::Expr::Lambda {
            params,
            body,
            captures,
            span,
        } => {
            let start = offset_to_position(source, span.start);
            let end = offset_to_position(source, span.end);
            let start_line = start.line;
            let end_line = end.line;
            let inline_invocation_line = preferred_name.is_none().then_some(start_line);
            let fallback_name = format!("closure#{}", closures.len() + 1);
            closures.push(ClosureRange {
                debug_key: span.start as i64,
                start_line,
                start_column: start.column,
                end_line,
                name: preferred_name.unwrap_or(fallback_name),
                params: params.iter().map(|param| param.name.clone()).collect(),
                captures: captures
                    .iter()
                    .map(|capture| capture.name.clone())
                    .collect(),
                body: body.as_ref().clone(),
                inline_invocation_line,
            });
            collect_closures_from_expr(body, source, closures, None);
        }
        kettu_parser::Expr::Binary { lhs, rhs, .. } => {
            collect_closures_from_expr(lhs, source, closures, None);
            collect_closures_from_expr(rhs, source, closures, None);
        }
        kettu_parser::Expr::Call { func, args, .. } => {
            collect_closures_from_expr(func, source, closures, None);
            for arg in args {
                collect_closures_from_expr(arg, source, closures, None);
            }
        }
        kettu_parser::Expr::Field { expr, .. }
        | kettu_parser::Expr::OptionalChain { expr, .. }
        | kettu_parser::Expr::Try { expr, .. }
        | kettu_parser::Expr::Await { expr, .. }
        | kettu_parser::Expr::AtomicLoad { addr: expr, .. }
        | kettu_parser::Expr::ThreadJoin { tid: expr, .. } => {
            collect_closures_from_expr(expr, source, closures, None);
        }
        kettu_parser::Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_closures_from_expr(cond, source, closures, None);
            collect_closures_from_statements(then_branch, source, closures);
            if let Some(else_branch) = else_branch {
                collect_closures_from_statements(else_branch, source, closures);
            }
        }
        kettu_parser::Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_closures_from_expr(scrutinee, source, closures, None);
            for arm in arms {
                collect_closures_from_statements(&arm.body, source, closures);
            }
        }
        kettu_parser::Expr::While {
            condition, body, ..
        } => {
            collect_closures_from_expr(condition, source, closures, None);
            collect_closures_from_statements(body, source, closures);
        }
        kettu_parser::Expr::For { range, body, .. } => {
            collect_closures_from_expr(range, source, closures, None);
            collect_closures_from_statements(body, source, closures);
        }
        kettu_parser::Expr::ForEach {
            collection, body, ..
        } => {
            collect_closures_from_expr(collection, source, closures, None);
            collect_closures_from_statements(body, source, closures);
        }
        kettu_parser::Expr::Range {
            start, end, step, ..
        } => {
            collect_closures_from_expr(start, source, closures, None);
            collect_closures_from_expr(end, source, closures, None);
            if let Some(step) = step {
                collect_closures_from_expr(step, source, closures, None);
            }
        }
        kettu_parser::Expr::Index { expr, index, .. } => {
            collect_closures_from_expr(expr, source, closures, None);
            collect_closures_from_expr(index, source, closures, None);
        }
        kettu_parser::Expr::Slice {
            expr, start, end, ..
        } => {
            collect_closures_from_expr(expr, source, closures, None);
            collect_closures_from_expr(start, source, closures, None);
            collect_closures_from_expr(end, source, closures, None);
        }
        kettu_parser::Expr::ListLiteral { elements, .. } => {
            for element in elements {
                collect_closures_from_expr(element, source, closures, None);
            }
        }
        kettu_parser::Expr::RecordLiteral { fields, .. } => {
            for (_, value) in fields {
                collect_closures_from_expr(value, source, closures, None);
            }
        }
        kettu_parser::Expr::Map { list, lambda, .. }
        | kettu_parser::Expr::Filter { list, lambda, .. } => {
            collect_closures_from_expr(list, source, closures, None);
            collect_closures_from_expr(lambda, source, closures, None);
        }
        kettu_parser::Expr::Reduce {
            list, init, lambda, ..
        } => {
            collect_closures_from_expr(list, source, closures, None);
            collect_closures_from_expr(init, source, closures, None);
            collect_closures_from_expr(lambda, source, closures, None);
        }
        kettu_parser::Expr::InterpolatedString(parts, _) => {
            for part in parts {
                if let kettu_parser::StringPart::Expr(expr) = part {
                    collect_closures_from_expr(expr, source, closures, None);
                }
            }
        }
        kettu_parser::Expr::Assert(expr, _)
        | kettu_parser::Expr::Not(expr, _)
        | kettu_parser::Expr::Neg(expr, _)
        | kettu_parser::Expr::StrLen(expr, _)
        | kettu_parser::Expr::ListLen(expr, _) => {
            collect_closures_from_expr(expr, source, closures, None);
        }
        kettu_parser::Expr::StrEq(lhs, rhs, _) | kettu_parser::Expr::ListPush(lhs, rhs, _) => {
            collect_closures_from_expr(lhs, source, closures, None);
            collect_closures_from_expr(rhs, source, closures, None);
        }
        kettu_parser::Expr::ListSet(list, index, value, _) => {
            collect_closures_from_expr(list, source, closures, None);
            collect_closures_from_expr(index, source, closures, None);
            collect_closures_from_expr(value, source, closures, None);
        }
        kettu_parser::Expr::VariantLiteral { payload, .. } => {
            if let Some(payload) = payload {
                collect_closures_from_expr(payload, source, closures, None);
            }
        }
        kettu_parser::Expr::AtomicStore { addr, value, .. }
        | kettu_parser::Expr::AtomicAdd { addr, value, .. }
        | kettu_parser::Expr::AtomicSub { addr, value, .. }
        | kettu_parser::Expr::AtomicNotify {
            addr, count: value, ..
        } => {
            collect_closures_from_expr(addr, source, closures, None);
            collect_closures_from_expr(value, source, closures, None);
        }
        kettu_parser::Expr::AtomicCmpxchg {
            addr,
            expected,
            replacement,
            ..
        } => {
            collect_closures_from_expr(addr, source, closures, None);
            collect_closures_from_expr(expected, source, closures, None);
            collect_closures_from_expr(replacement, source, closures, None);
        }
        kettu_parser::Expr::AtomicWait {
            addr,
            expected,
            timeout,
            ..
        } => {
            collect_closures_from_expr(addr, source, closures, None);
            collect_closures_from_expr(expected, source, closures, None);
            collect_closures_from_expr(timeout, source, closures, None);
        }
        kettu_parser::Expr::Spawn { body, .. } | kettu_parser::Expr::AtomicBlock { body, .. } => {
            collect_closures_from_statements(body, source, closures);
        }
        kettu_parser::Expr::SimdOp { args, .. } => {
            for arg in args {
                collect_closures_from_expr(arg, source, closures, None);
            }
        }
        kettu_parser::Expr::SimdForEach {
            collection, body, ..
        } => {
            collect_closures_from_expr(collection, source, closures, None);
            collect_closures_from_statements(body, source, closures);
        }
        kettu_parser::Expr::Integer(_, _)
        | kettu_parser::Expr::Bool(_, _)
        | kettu_parser::Expr::String(_, _)
        | kettu_parser::Expr::Ident(_) => {}
    }
}

fn build_debug_symbols(
    program: &PathBuf,
    source: &str,
    ast: &kettu_parser::WitFile,
) -> Result<DebugSymbols, String> {
    let imported_asts = crate::load_imported_asts(program, ast);
    let imported_aliases: HashSet<String> = imported_asts
        .iter()
        .map(|(alias, _)| alias.clone())
        .collect();

    let diagnostics = kettu_checker::check_with_source(ast, source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| matches!(diagnostic.severity, kettu_checker::Severity::Error))
        .filter(|diagnostic| {
            if diagnostic.message.starts_with("Unknown variable: ") {
                let variable = diagnostic.message.trim_start_matches("Unknown variable: ");
                !imported_aliases.contains(variable)
            } else {
                true
            }
        })
        .map(|diagnostic| diagnostic.message.clone())
        .collect();

    if !errors.is_empty() {
        return Err(format!(
            "Failed to build debug source map: {}",
            errors.join("; ")
        ));
    }

    let compile_options = kettu_codegen::CompileOptions {
        core_only: true,
        memory_pages: 1,
        wasip3: false,
        threads: false,
        emit_dwarf: true,
        keep_names: true,
        debug_source: Some(source.to_string()),
        debug_path: Some(program.display().to_string()),
        emit_debug_hooks: false,
    };

    let wasm = if imported_asts.is_empty() {
        kettu_codegen::build_core_module(ast, &compile_options)
    } else {
        let imports_refs: Vec<_> = imported_asts
            .iter()
            .map(|(alias, ast)| (alias.clone(), ast))
            .collect();
        kettu_codegen::compile_module_with_imports(ast, &imports_refs, &compile_options)
    }
    .map_err(|err| format!("Failed to build debug source map: {}", err))?;

    parse_debug_symbols(&wasm)
}

fn parse_debug_symbols(wasm: &[u8]) -> Result<DebugSymbols, String> {
    let mut sections = HashMap::new();

    for payload in Parser::new(0).parse_all(wasm) {
        match payload.map_err(|err| format!("Invalid debug wasm payload: {}", err))? {
            Payload::CustomSection(section) if section.name().starts_with(".debug_") => {
                sections.insert(section.name().to_owned(), section.data().to_vec());
            }
            _ => {}
        }
    }

    for required in [".debug_abbrev", ".debug_info", ".debug_line"] {
        if !sections.contains_key(required) {
            return Err(format!("Missing required DWARF section: {}", required));
        }
    }

    let dwarf_sections = DwarfSections::load(|id| -> Result<Vec<u8>, gimli::Error> {
        Ok(sections.get(id.name()).cloned().unwrap_or_default())
    })
    .map_err(|err| format!("Failed to load DWARF sections: {}", err))?;
    let dwarf = dwarf_sections.borrow(|section| EndianSlice::new(section.as_slice(), LittleEndian));

    let mut symbols = DebugSymbols::default();
    let mut unit_headers = dwarf.units();
    while let Some(unit_header) = unit_headers
        .next()
        .map_err(|err| format!("Failed to read DWARF unit header: {}", err))?
    {
        let unit = dwarf
            .unit(unit_header)
            .map_err(|err| format!("Failed to read DWARF unit: {}", err))?;
        let line_rows = collect_dwarf_line_rows(&unit)
            .map_err(|err| format!("Failed to read DWARF line rows: {}", err))?;

        let mut entries = unit.entries();
        let mut active_subprograms: Vec<Option<usize>> = Vec::new();
        while let Some(entry) = entries
            .next_dfs()
            .map_err(|err| format!("Failed to walk DWARF entries: {}", err))?
        {
            let depth = entry.depth().max(0) as usize;
            active_subprograms.truncate(depth + 1);
            if active_subprograms.len() <= depth {
                active_subprograms.resize(depth + 1, None);
            }

            let name = entry
                .attr_value(constants::DW_AT_name)
                .map(|name_attr| {
                    dwarf
                        .attr_string(&unit, name_attr)
                        .map(|value| value.to_string_lossy().into_owned())
                })
                .transpose()
                .map_err(|err| format!("Failed to read DWARF symbol name: {}", err))?;

            match entry.tag() {
                constants::DW_TAG_subprogram => {
                    let Some(name) = name else {
                        continue;
                    };
                    let start_line = entry
                        .attr_value(constants::DW_AT_decl_line)
                        .and_then(attribute_u64)
                        .map(|line| line as i64)
                        .unwrap_or(1);
                    let low_pc = entry
                        .attr_value(constants::DW_AT_low_pc)
                        .and_then(attribute_address);
                    let high_pc = entry
                        .attr_value(constants::DW_AT_high_pc)
                        .and_then(|value| attribute_range_end(value, low_pc));
                    let end_line =
                        max_line_for_range(&line_rows, low_pc, high_pc).unwrap_or(start_line);

                    register_debug_symbol(&mut symbols, name.clone(), start_line, end_line);
                    symbols.subprograms.push(DwarfSubprogram {
                        name,
                        start_line,
                        end_line,
                        bindings: Vec::new(),
                    });
                    active_subprograms[depth] = Some(symbols.subprograms.len() - 1);
                }
                constants::DW_TAG_formal_parameter | constants::DW_TAG_variable => {
                    let Some(name) = name else {
                        continue;
                    };
                    let Some(subprogram_index) =
                        active_subprograms.iter().rev().find_map(|index| *index)
                    else {
                        continue;
                    };
                    let kind = if entry.tag() == constants::DW_TAG_formal_parameter {
                        DwarfBindingKind::Parameter
                    } else {
                        DwarfBindingKind::Variable
                    };
                    let decl_line = entry
                        .attr_value(constants::DW_AT_decl_line)
                        .and_then(attribute_u64)
                        .map(|line| line as i64)
                        .unwrap_or(0);
                    let local_index = entry
                        .attr_value(constants::DW_AT_location)
                        .map(|value| parse_wasm_local(value, unit.encoding()))
                        .transpose()
                        .map_err(|err| format!("Failed to parse DWARF location: {}", err))?
                        .flatten();
                    symbols.subprograms[subprogram_index]
                        .bindings
                        .push(DwarfBinding {
                            name,
                            kind,
                            decl_line,
                            local_index,
                        });
                }
                _ => {}
            }
        }
    }

    Ok(symbols)
}

fn collect_dwarf_line_rows<R: Reader>(
    unit: &gimli::Unit<R>,
) -> Result<Vec<DwarfLineRow>, gimli::Error> {
    let mut line_rows = Vec::new();
    if let Some(program) = unit.line_program.clone() {
        let mut rows = program.rows();
        while let Some((_, row)) = rows.next_row()? {
            let Some(line) = row.line().map(|line| line.get() as i64) else {
                continue;
            };
            line_rows.push(DwarfLineRow {
                address: row.address(),
                line,
            });
        }
    }
    Ok(line_rows)
}

fn attribute_u64<R: Reader>(value: gimli::AttributeValue<R>) -> Option<u64> {
    match value {
        gimli::AttributeValue::Udata(value) => Some(value),
        gimli::AttributeValue::Data1(value) => Some(value.into()),
        gimli::AttributeValue::Data2(value) => Some(value.into()),
        gimli::AttributeValue::Data4(value) => Some(value.into()),
        gimli::AttributeValue::Data8(value) => Some(value),
        _ => None,
    }
}

fn parse_wasm_local<R: Reader>(
    value: gimli::AttributeValue<R>,
    encoding: gimli::Encoding,
) -> Result<Option<u32>, gimli::Error> {
    let gimli::AttributeValue::Exprloc(expression) = value else {
        return Ok(None);
    };
    let mut ops = expression.operations(encoding);
    match ops.next()? {
        Some(gimli::Operation::WasmLocal { index }) => Ok(Some(index)),
        _ => Ok(None),
    }
}

fn attribute_address<R: Reader>(value: gimli::AttributeValue<R>) -> Option<u64> {
    match value {
        gimli::AttributeValue::Addr(value) => Some(value),
        _ => None,
    }
}

fn attribute_range_end<R: Reader>(
    value: gimli::AttributeValue<R>,
    low_pc: Option<u64>,
) -> Option<u64> {
    match value {
        gimli::AttributeValue::Addr(value) => Some(value),
        gimli::AttributeValue::Udata(length) => low_pc.map(|low| low + length),
        gimli::AttributeValue::Data1(length) => low_pc.map(|low| low + u64::from(length)),
        gimli::AttributeValue::Data2(length) => low_pc.map(|low| low + u64::from(length)),
        gimli::AttributeValue::Data4(length) => low_pc.map(|low| low + u64::from(length)),
        gimli::AttributeValue::Data8(length) => low_pc.map(|low| low + length),
        _ => None,
    }
}

fn max_line_for_range(
    line_rows: &[DwarfLineRow],
    low_pc: Option<u64>,
    high_pc: Option<u64>,
) -> Option<i64> {
    line_rows
        .iter()
        .filter(|row| {
            let after_start = match low_pc {
                Some(low_pc) => row.address >= low_pc,
                None => true,
            };
            let before_end = match high_pc {
                Some(high_pc) => row.address < high_pc,
                None => true,
            };
            after_start && before_end
        })
        .map(|row| row.line)
        .max()
}

fn register_debug_symbol(symbols: &mut DebugSymbols, name: String, start_line: i64, end_line: i64) {
    let symbol = DebugSymbol {
        start_line,
        end_line,
    };

    if name.starts_with("lambda#") {
        symbols.lambdas.push(symbol);
    } else {
        symbols.functions.insert(name, symbol);
    }
}

fn resolve_dwarf_subprogram(
    session: &DebugSession,
    target: FrameTarget,
) -> Option<&DwarfSubprogram> {
    match target {
        FrameTarget::Test => {
            let test = session.active_test()?;
            session.debug_symbols.subprograms.iter().find(|subprogram| {
                subprogram.name == test.name
                    || subprogram.name.ends_with(&format!("#{}", test.name))
            })
        }
        FrameTarget::Closure(closure_index) => {
            let closure = &session.closures[closure_index];
            session
                .debug_symbols
                .subprograms
                .iter()
                .find(|subprogram| {
                    subprogram.name == closure.name
                        || (subprogram.name.starts_with("lambda#")
                            && subprogram.start_line == closure.start_line
                            && subprogram.end_line == closure.end_line)
                })
                .or_else(|| {
                    session.debug_symbols.subprograms.iter().find(|subprogram| {
                        subprogram.name.starts_with("lambda#")
                            && subprogram.start_line == closure.start_line
                    })
                })
        }
        FrameTarget::Function(idx) => session.debug_symbols.subprograms.get(idx),
    }
}

fn runtime_binding_kind(binding: &DebugResumeBindingMetadata) -> DwarfBindingKind {
    match binding.kind.as_str() {
        "parameter" => DwarfBindingKind::Parameter,
        _ => DwarfBindingKind::Variable,
    }
}

fn active_runtime_debug_frame<'a>(
    session: &'a DebugSession,
    snapshot: &PausedRuntimeSnapshot,
) -> Option<&'a DebugResumeFrameMetadata> {
    let start_line = snapshot.subprogram_start_line?;
    session
        .runtime_debug_artifacts
        .as_ref()?
        .debug_resume_frames
        .iter()
        .find(|frame| frame.start_line == start_line)
}

fn resolve_runtime_debug_frame<'a>(
    session: &'a DebugSession,
    target: FrameTarget,
) -> Option<&'a DebugResumeFrameMetadata> {
    let frames = &session.runtime_debug_artifacts.as_ref()?.debug_resume_frames;
    match target {
        FrameTarget::Test => {
            let test = session.active_test()?;
            frames.iter().find(|frame| {
                frame.name == test.name || frame.name.ends_with(&format!("#{}", test.name))
            })
        }
        FrameTarget::Closure(closure_index) => {
            let closure = &session.closures[closure_index];
            frames.iter().find(|frame| {
                frame.name == closure.name
                    || (frame.name.starts_with("lambda#") && frame.start_line == closure.start_line)
            })
        }
        FrameTarget::Function(idx) => {
            if let Some(subprogram) = session.debug_symbols.subprograms.get(idx) {
                frames.iter().find(|frame| {
                    frame.start_line == subprogram.start_line && frame.name == subprogram.name
                })
            } else {
                let snapshot = paused_runtime_snapshot(session)?;
                let active = active_runtime_debug_frame(session, &snapshot)?;
                (!active.name.starts_with("lambda#")).then_some(active)
            }
        }
    }
}

fn bindings_for_target(session: &DebugSession, target: FrameTarget) -> Vec<DwarfBinding> {
    if let Some(frame) = resolve_runtime_debug_frame(session, target) {
        return frame
            .bindings
            .iter()
            .map(|binding| DwarfBinding {
                name: binding.name.clone(),
                kind: runtime_binding_kind(binding),
                decl_line: binding.decl_line,
                local_index: Some(binding.local_index),
            })
            .collect();
    }

    resolve_dwarf_subprogram(session, target)
        .map(|subprogram| subprogram.bindings.clone())
        .unwrap_or_default()
}

fn current_trace_event(session: &DebugSession) -> Option<&TraceEvent> {
    let test = session.active_test()?;
    let index = session.current_trace_index?;
    test.trace.get(index)
}

fn paused_runtime_snapshot(session: &DebugSession) -> Option<PausedRuntimeSnapshot> {
    if session.runtime_pause_event.is_some() {
        if let Some(event) = session
            .active_runtime_session
            .as_ref()
            .and_then(RuntimeDebugSession::latest_event)
        {
            return Some(PausedRuntimeSnapshot {
                subprogram_start_line: event.subprogram_start_line,
                locals: event.locals.clone(),
                pointer_derefs: event.pointer_derefs.clone(),
                closure_keys: event.active_closure_keys.clone(),
            });
        }
    }

    current_trace_event(session).map(|entry| PausedRuntimeSnapshot {
        subprogram_start_line: entry.runtime_subprogram_start_line,
        locals: entry.runtime_locals.clone(),
        pointer_derefs: entry.runtime_pointer_derefs.clone(),
        closure_keys: entry.runtime_closure_keys.clone(),
    })
}

fn binding_visible_at_line(
    binding: &DwarfBinding,
    line: i64,
    capture_names: Option<&HashSet<String>>,
) -> bool {
    match binding.kind {
        DwarfBindingKind::Parameter => true,
        DwarfBindingKind::Variable => capture_names
            .filter(|captures| captures.contains(&binding.name))
            .map(|_| binding.decl_line <= line)
            .unwrap_or(binding.decl_line < line),
    }
}

fn runtime_value_to_simple(raw: i64, fallback: Option<&SimpleValue>) -> SimpleValue {
    match fallback {
        Some(SimpleValue::Bool(_)) => SimpleValue::Bool(raw != 0),
        Some(SimpleValue::String(value)) => SimpleValue::String(value.clone()),
        Some(SimpleValue::List(values)) => SimpleValue::List(values.clone()),
        Some(SimpleValue::Record(fields)) => SimpleValue::Record(fields.clone()),
        Some(SimpleValue::Result {
            discriminant,
            payload,
            payload_value,
            err_message,
        }) => SimpleValue::Result {
            discriminant: *discriminant,
            payload: *payload,
            payload_value: payload_value.clone(),
            err_message: err_message.clone(),
        },
        _ => SimpleValue::Int(raw),
    }
}

fn runtime_binding_values_for_target(
    session: &DebugSession,
    target: FrameTarget,
    fallback_values: &HashMap<String, SimpleValue>,
) -> Option<HashMap<String, SimpleValue>> {
    let snapshot = paused_runtime_snapshot(session)?;
    let bindings = if let Some(frame) = resolve_runtime_debug_frame(session, target) {
        if snapshot.subprogram_start_line != Some(frame.start_line) {
            return None;
        }
        frame
            .bindings
            .iter()
            .map(|binding| DwarfBinding {
                name: binding.name.clone(),
                kind: runtime_binding_kind(binding),
                decl_line: binding.decl_line,
                local_index: Some(binding.local_index),
            })
            .collect::<Vec<_>>()
    } else {
        let subprogram = resolve_dwarf_subprogram(session, target)?;
        if snapshot.subprogram_start_line != Some(subprogram.start_line) {
            return None;
        }
        subprogram.bindings.clone()
    };

    Some(
        bindings
            .iter()
            .filter_map(|binding| {
                let local_index = binding.local_index?;
                let raw = snapshot.locals.get(&local_index)?;
                let fallback_value = fallback_values.get(&binding.name);
                let simple = if let Some(deref) = snapshot.pointer_derefs.get(&local_index) {
                    let ok_disc = variant_discriminant("ok");
                    let err_disc = variant_discriminant("err");
                    let some_disc = variant_discriminant("some");
                    let none_disc = variant_discriminant("none");
                    if deref.discriminant == ok_disc
                        || deref.discriminant == err_disc
                        || deref.discriminant == some_disc
                        || deref.discriminant == none_disc
                    {
                        let payload_value = match fallback_value {
                            Some(SimpleValue::Result {
                                discriminant,
                                payload_value,
                                ..
                            }) if *discriminant == deref.discriminant => payload_value.clone(),
                            _ => None,
                        };

                        SimpleValue::Result {
                            discriminant: deref.discriminant,
                            payload: deref.payload,
                            payload_value,
                            err_message: deref.err_string.clone(),
                        }
                    } else {
                        runtime_value_to_simple(*raw, fallback_value)
                    }
                } else {
                    runtime_value_to_simple(*raw, fallback_value)
                };
                Some((binding.name.clone(), simple))
            })
            .collect(),
    )
}

fn variables_from_dwarf_bindings<'a>(
    bindings: impl IntoIterator<Item = &'a DwarfBinding>,
    values: &HashMap<String, SimpleValue>,
) -> Vec<Variable> {
    let mut variables = Vec::new();
    let mut seen = HashSet::new();

    for binding in bindings {
        if !seen.insert(binding.name.clone()) {
            continue;
        }
        let value = values
            .get(&binding.name)
            .cloned()
            .unwrap_or_else(|| SimpleValue::Unknown(binding.name.clone()));
        variables.push(Variable::from_value(binding.name.clone(), value));
    }

    for (name, value) in values {
        if seen.contains(name) || !is_identifier(name) {
            continue;
        }
        seen.insert(name.clone());
        variables.push(Variable::from_value(name.clone(), value.clone()));
    }

    variables.sort_by(|left, right| left.name.cmp(&right.name));

    variables
}

fn apply_debug_symbols(
    tests: &mut [ListedTest],
    closures: &mut [ClosureRange],
    debug_symbols: &DebugSymbols,
) {
    for test in tests {
        if let Some(symbol) = debug_symbols
            .functions
            .iter()
            .find(|(name, _)| *name == &test.name || name.ends_with(&format!("#{}", test.name)))
            .map(|(_, symbol)| symbol)
        {
            test.line = symbol.start_line;
            test.end_line = test.end_line.max(symbol.end_line);
        }
    }

    for closure in closures {
        if let Some(symbol) = debug_symbols
            .lambdas
            .iter()
            .find(|symbol| symbol.start_line == closure.start_line)
        {
            closure.start_line = symbol.start_line;
            closure.end_line = symbol.end_line;
        }
    }
}

fn build_test_traces(tests: &mut [ListedTest], closures: &[ClosureRange], source: &str) {
    for test in tests {
        let mut env = HashMap::new();
        let mut trace = Vec::new();
        let _ = simulate_statements(&test.body, closures, source, &mut env, &mut trace, true);
        if let Some(first) = trace.first() {
            test.line = first.line;
        }
        if let Some(last) = trace.last() {
            test.end_line = test.end_line.max(last.line);
        }
        test.trace = trace;
    }
}

fn simulate_statements(
    statements: &[kettu_parser::Statement],
    closures: &[ClosureRange],
    source: &str,
    env: &mut HashMap<String, SimpleValue>,
    trace: &mut Vec<TraceEvent>,
    tail_returns: bool,
) -> StatementFlow {
    for (index, statement) in statements.iter().enumerate() {
        let is_tail = tail_returns && index + 1 == statements.len();
        match simulate_statement(statement, closures, source, env, trace, is_tail) {
            StatementFlow::Continue => {}
            flow => return flow,
        }
    }

    StatementFlow::Continue
}

fn simulate_statement(
    statement: &kettu_parser::Statement,
    closures: &[ClosureRange],
    source: &str,
    env: &mut HashMap<String, SimpleValue>,
    trace: &mut Vec<TraceEvent>,
    is_tail: bool,
) -> StatementFlow {
    match statement {
        kettu_parser::Statement::Expr(expr) => {
            simulate_expr_statement(expr, closures, source, env, trace, is_tail)
        }
        kettu_parser::Statement::Let { name, value } => {
            record_trace_event(trace, offset_to_position(source, name.span.start), env);
            env.insert(
                name.name.clone(),
                evaluate_ast_expr(value, closures, source, env),
            );
            StatementFlow::Continue
        }
        kettu_parser::Statement::Return(Some(expr)) => {
            record_trace_event(trace, expr_start_position(expr, source), env);
            StatementFlow::Return(Some(evaluate_ast_expr(expr, closures, source, env)))
        }
        kettu_parser::Statement::Return(None) => {
            record_trace_event(trace, SourcePosition { line: 0, column: 0 }, env);
            StatementFlow::Return(None)
        }
        kettu_parser::Statement::Assign { name, value } => {
            record_trace_event(trace, offset_to_position(source, name.span.start), env);
            env.insert(
                name.name.clone(),
                evaluate_ast_expr(value, closures, source, env),
            );
            StatementFlow::Continue
        }
        kettu_parser::Statement::CompoundAssign { name, op, value } => {
            record_trace_event(trace, offset_to_position(source, name.span.start), env);
            let current = env
                .get(&name.name)
                .cloned()
                .unwrap_or_else(|| SimpleValue::Unknown(name.name.clone()));
            let rhs = evaluate_ast_expr(value, closures, source, env);
            env.insert(
                name.name.clone(),
                apply_binary_op(current, rhs, bin_op_symbol(*op))
                    .unwrap_or_else(|_| SimpleValue::Unknown(name.name.clone())),
            );
            StatementFlow::Continue
        }
        kettu_parser::Statement::Break { condition } => {
            let position = condition
                .as_deref()
                .map(|expr| expr_start_position(expr, source))
                .unwrap_or(SourcePosition { line: 0, column: 0 });
            record_trace_event(trace, position, env);
            match condition.as_deref() {
                Some(expr) => match evaluate_ast_expr(expr, closures, source, env) {
                    SimpleValue::Bool(true) => StatementFlow::Break,
                    _ => StatementFlow::Continue,
                },
                None => StatementFlow::Break,
            }
        }
        kettu_parser::Statement::Continue { condition } => {
            let position = condition
                .as_deref()
                .map(|expr| expr_start_position(expr, source))
                .unwrap_or(SourcePosition { line: 0, column: 0 });
            record_trace_event(trace, position, env);
            match condition.as_deref() {
                Some(expr) => match evaluate_ast_expr(expr, closures, source, env) {
                    SimpleValue::Bool(true) => StatementFlow::ContinueLoop,
                    _ => StatementFlow::Continue,
                },
                None => StatementFlow::ContinueLoop,
            }
        }
        kettu_parser::Statement::SharedLet {
            name,
            initial_value,
        } => {
            record_trace_event(trace, offset_to_position(source, name.span.start), env);
            env.insert(
                name.name.clone(),
                evaluate_ast_expr(initial_value, closures, source, env),
            );
            StatementFlow::Continue
        }
        kettu_parser::Statement::Guard {
            condition,
            else_body,
        } => {
            record_trace_event(trace, expr_start_position(condition, source), env);
            match evaluate_ast_expr(condition, closures, source, env) {
                SimpleValue::Bool(true) => StatementFlow::Continue,
                _ => simulate_statements(else_body, closures, source, env, trace, is_tail),
            }
        }
        kettu_parser::Statement::GuardLet {
            name,
            value,
            else_body,
        } => {
            record_trace_event(trace, offset_to_position(source, name.span.start), env);
            let evaluated = evaluate_ast_expr(value, closures, source, env);
            if matches!(evaluated, SimpleValue::Unknown(_)) {
                simulate_statements(else_body, closures, source, env, trace, is_tail)
            } else {
                env.insert(name.name.clone(), evaluated);
                StatementFlow::Continue
            }
        }
    }
}

fn simulate_expr_statement(
    expr: &kettu_parser::Expr,
    closures: &[ClosureRange],
    source: &str,
    env: &mut HashMap<String, SimpleValue>,
    trace: &mut Vec<TraceEvent>,
    is_tail: bool,
) -> StatementFlow {
    match expr {
        kettu_parser::Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            record_trace_event(trace, expr_start_position(expr, source), env);
            match evaluate_ast_expr(cond, closures, source, env) {
                SimpleValue::Bool(true) => {
                    simulate_statements(then_branch, closures, source, env, trace, is_tail)
                }
                SimpleValue::Bool(false) => simulate_statements(
                    else_branch.as_deref().unwrap_or(&[]),
                    closures,
                    source,
                    env,
                    trace,
                    is_tail,
                ),
                _ => simulate_statements(then_branch, closures, source, env, trace, is_tail),
            }
        }
        kettu_parser::Expr::While {
            condition, body, ..
        } => loop {
            record_trace_event(trace, expr_start_position(expr, source), env);
            match evaluate_ast_expr(condition, closures, source, env) {
                SimpleValue::Bool(true) => {
                    match simulate_statements(body, closures, source, env, trace, false) {
                        StatementFlow::Continue => continue,
                        StatementFlow::Break => return StatementFlow::Continue,
                        StatementFlow::ContinueLoop => continue,
                        flow => return flow,
                    }
                }
                SimpleValue::Bool(false) => return StatementFlow::Continue,
                _ => return StatementFlow::Continue,
            }
        },
        kettu_parser::Expr::For {
            variable,
            range,
            body,
            ..
        } => {
            record_trace_event(trace, expr_start_position(expr, source), env);
            if let Some(values) = evaluate_range_values(range, closures, source, env) {
                for value in values {
                    env.insert(variable.name.clone(), SimpleValue::Int(value));
                    match simulate_statements(body, closures, source, env, trace, false) {
                        StatementFlow::Continue => {}
                        StatementFlow::Break => break,
                        StatementFlow::ContinueLoop => continue,
                        flow => return flow,
                    }
                }
            }
            StatementFlow::Continue
        }
        kettu_parser::Expr::ForEach {
            variable,
            collection,
            body,
            ..
        } => {
            record_trace_event(trace, expr_start_position(expr, source), env);
            if let SimpleValue::List(values) = evaluate_ast_expr(collection, closures, source, env) {
                for value in values {
                    env.insert(variable.name.clone(), value);
                    match simulate_statements(body, closures, source, env, trace, false) {
                        StatementFlow::Continue => {}
                        StatementFlow::Break => break,
                        StatementFlow::ContinueLoop => continue,
                        flow => return flow,
                    }
                }
            }
            StatementFlow::Continue
        }
        kettu_parser::Expr::Match {
            scrutinee, arms, ..
        } => {
            record_trace_event(trace, expr_start_position(expr, source), env);
            let scrutinee = evaluate_ast_expr(scrutinee, closures, source, env);
            if let Some((arm, bindings)) =
                select_match_arm(arms, &scrutinee, closures, source, env)
            {
                let mut match_env = env.clone();
                match_env.extend(bindings);
                simulate_statements(&arm.body, closures, source, &mut match_env, trace, is_tail)
            } else {
                StatementFlow::Continue
            }
        }
        _ => {
            record_trace_event(trace, expr_start_position(expr, source), env);
            let value = evaluate_ast_expr(expr, closures, source, env);
            if is_tail {
                StatementFlow::Return(Some(value))
            } else {
                StatementFlow::Continue
            }
        }
    }
}

fn evaluate_ast_expr(
    expr: &kettu_parser::Expr,
    closures: &[ClosureRange],
    source: &str,
    env: &HashMap<String, SimpleValue>,
) -> SimpleValue {
    match expr {
        kettu_parser::Expr::Ident(id) => env
            .get(&id.name)
            .cloned()
            .unwrap_or_else(|| SimpleValue::Unknown(id.name.clone())),
        kettu_parser::Expr::Integer(value, _) => SimpleValue::Int(*value),
        kettu_parser::Expr::String(value, _) => SimpleValue::String(value.clone()),
        kettu_parser::Expr::Bool(value, _) => SimpleValue::Bool(*value),
        kettu_parser::Expr::InterpolatedString(parts, _) => {
            let mut result = String::new();
            for part in parts {
                match part {
                    kettu_parser::StringPart::Literal(value) => result.push_str(value),
                    kettu_parser::StringPart::Expr(expr) => {
                        result.push_str(&evaluate_ast_expr(expr, closures, source, env).display())
                    }
                }
            }
            SimpleValue::String(result)
        }
        kettu_parser::Expr::ListLiteral { elements, .. } => SimpleValue::List(
            elements
                .iter()
                .map(|element| evaluate_ast_expr(element, closures, source, env))
                .collect(),
        ),
        kettu_parser::Expr::RecordLiteral { fields, .. } => SimpleValue::Record(
            fields
                .iter()
                .map(|(name, value)| {
                    (
                        name.name.clone(),
                        evaluate_ast_expr(value, closures, source, env),
                    )
                })
                .collect(),
        ),
        kettu_parser::Expr::VariantLiteral {
            case_name, payload, ..
        } => simple_variant_value(
            &case_name.name,
            payload
                .as_ref()
                .map(|value| evaluate_ast_expr(value, closures, source, env)),
        ),
        kettu_parser::Expr::Field { expr, field, .. } => {
            match evaluate_ast_expr(expr, closures, source, env) {
                SimpleValue::Record(fields) => fields
                    .get(&field.name)
                    .cloned()
                    .unwrap_or_else(|| SimpleValue::Unknown(field.name.clone())),
                _ => SimpleValue::Unknown("<field>".to_string()),
            }
        }
        kettu_parser::Expr::OptionalChain { expr, field, .. } => {
            match evaluate_ast_expr(expr, closures, source, env) {
                SimpleValue::Result {
                    discriminant,
                    payload,
                    payload_value,
                    err_message,
                } if discriminant == variant_discriminant("some") => {
                    match variant_payload_value(
                        discriminant,
                        payload,
                        payload_value.as_deref(),
                        err_message.as_ref(),
                    ) {
                        Some(SimpleValue::Record(fields)) => fields
                            .get(&field.name)
                            .cloned()
                            .map(|value| simple_variant_value("some", Some(value)))
                            .unwrap_or_else(|| simple_variant_value("none", None)),
                        Some(value) => simple_variant_value("some", Some(value)),
                        None => simple_variant_value("none", None),
                    }
                }
                SimpleValue::Result { discriminant, .. }
                    if discriminant == variant_discriminant("none") =>
                {
                    simple_variant_value("none", None)
                }
                _ => SimpleValue::Unknown("<optional-chain>".to_string()),
            }
        }
        kettu_parser::Expr::Try { expr, .. } => match evaluate_ast_expr(expr, closures, source, env) {
            SimpleValue::Result {
                discriminant,
                payload,
                payload_value,
                err_message,
            } if discriminant == variant_discriminant("some")
                || discriminant == variant_discriminant("ok") => {
                variant_payload_value(
                    discriminant,
                    payload,
                    payload_value.as_deref(),
                    err_message.as_ref(),
                )
                    .unwrap_or_else(|| SimpleValue::Unknown("<try>".to_string()))
            }
            result @ SimpleValue::Result { .. } => result,
            other => other,
        },
        kettu_parser::Expr::Index { expr, index, .. } => {
            let list = evaluate_ast_expr(expr, closures, source, env);
            let index = evaluate_ast_expr(index, closures, source, env);
            match (list, index) {
                (SimpleValue::List(values), SimpleValue::Int(index)) if index >= 0 => values
                    .get(index as usize)
                    .cloned()
                    .unwrap_or_else(|| SimpleValue::Unknown("<index>".to_string())),
                _ => SimpleValue::Unknown("<index>".to_string()),
            }
        }
        kettu_parser::Expr::Binary { lhs, op, rhs, .. } => apply_binary_op(
            evaluate_ast_expr(lhs, closures, source, env),
            evaluate_ast_expr(rhs, closures, source, env),
            bin_op_symbol(*op),
        )
        .unwrap_or_else(|_| SimpleValue::Unknown("<binary>".to_string())),
        kettu_parser::Expr::Not(inner, _) => {
            match evaluate_ast_expr(inner, closures, source, env) {
                SimpleValue::Bool(value) => SimpleValue::Bool(!value),
                _ => SimpleValue::Unknown("<not>".to_string()),
            }
        }
        kettu_parser::Expr::Neg(inner, _) => {
            match evaluate_ast_expr(inner, closures, source, env) {
                SimpleValue::Int(value) => SimpleValue::Int(-value),
                SimpleValue::Float(value) => SimpleValue::Float(-value),
                _ => SimpleValue::Unknown("<neg>".to_string()),
            }
        }
        kettu_parser::Expr::Assert(inner, _) => evaluate_ast_expr(inner, closures, source, env),
        kettu_parser::Expr::StrLen(inner, _) => {
            match evaluate_ast_expr(inner, closures, source, env) {
                SimpleValue::String(value) => SimpleValue::Int(value.len() as i64),
                _ => SimpleValue::Unknown("<str-len>".to_string()),
            }
        }
        kettu_parser::Expr::StrEq(lhs, rhs, _) => {
            let lhs = evaluate_ast_expr(lhs, closures, source, env);
            let rhs = evaluate_ast_expr(rhs, closures, source, env);
            SimpleValue::Bool(lhs == rhs)
        }
        kettu_parser::Expr::ListLen(inner, _) => {
            match evaluate_ast_expr(inner, closures, source, env) {
                SimpleValue::List(values) => SimpleValue::Int(values.len() as i64),
                _ => SimpleValue::Unknown("<list-len>".to_string()),
            }
        }
        kettu_parser::Expr::ListPush(list, value, _) => {
            match evaluate_ast_expr(list, closures, source, env) {
                SimpleValue::List(mut values) => {
                    values.push(evaluate_ast_expr(value, closures, source, env));
                    SimpleValue::List(values)
                }
                _ => SimpleValue::Unknown("<list-push>".to_string()),
            }
        }
        kettu_parser::Expr::ListSet(list, index, value, _) => {
            let list = evaluate_ast_expr(list, closures, source, env);
            let index = evaluate_ast_expr(index, closures, source, env);
            let value = evaluate_ast_expr(value, closures, source, env);
            match (list, index) {
                (SimpleValue::List(mut values), SimpleValue::Int(index)) if index >= 0 => {
                    if let Some(slot) = values.get_mut(index as usize) {
                        *slot = value;
                        SimpleValue::List(values)
                    } else {
                        SimpleValue::Unknown("<list-set>".to_string())
                    }
                }
                _ => SimpleValue::Unknown("<list-set>".to_string()),
            }
        }
        kettu_parser::Expr::Slice {
            expr, start, end, ..
        } => {
            let list = evaluate_ast_expr(expr, closures, source, env);
            let start = evaluate_ast_expr(start, closures, source, env);
            let end = evaluate_ast_expr(end, closures, source, env);
            match (list, start, end) {
                (SimpleValue::List(values), SimpleValue::Int(start), SimpleValue::Int(end)) => {
                    let len = values.len();
                    let start = start.max(0) as usize;
                    let end = end.max(0) as usize;
                    let start = start.min(len);
                    let end = end.min(len);
                    if start > end {
                        SimpleValue::List(Vec::new())
                    } else {
                        SimpleValue::List(values[start..end].to_vec())
                    }
                }
                _ => SimpleValue::Unknown("<slice>".to_string()),
            }
        }
        kettu_parser::Expr::Map { list, lambda, .. } => {
            match evaluate_ast_expr(list, closures, source, env) {
                SimpleValue::List(values) => SimpleValue::List(
                    values
                        .into_iter()
                        .map(|value| apply_lambda_expr(lambda, vec![value], closures, source, env))
                        .collect(),
                ),
                _ => SimpleValue::Unknown("<map>".to_string()),
            }
        }
        kettu_parser::Expr::Filter { list, lambda, .. } => {
            match evaluate_ast_expr(list, closures, source, env) {
                SimpleValue::List(values) => SimpleValue::List(
                    values
                        .into_iter()
                        .filter(|value| {
                            matches!(
                                apply_lambda_expr(
                                    lambda,
                                    vec![value.clone()],
                                    closures,
                                    source,
                                    env,
                                ),
                                SimpleValue::Bool(true)
                            )
                        })
                        .collect(),
                ),
                _ => SimpleValue::Unknown("<filter>".to_string()),
            }
        }
        kettu_parser::Expr::Reduce {
            list, init, lambda, ..
        } => {
            let values = match evaluate_ast_expr(list, closures, source, env) {
                SimpleValue::List(values) => values,
                _ => return SimpleValue::Unknown("<reduce>".to_string()),
            };
            let mut acc = evaluate_ast_expr(init, closures, source, env);
            for value in values {
                acc = apply_lambda_expr(lambda, vec![acc, value], closures, source, env);
            }
            acc
        }
        kettu_parser::Expr::Range { .. } => evaluate_range_values(expr, closures, source, env)
            .map(|values| {
                SimpleValue::List(values.into_iter().map(SimpleValue::Int).collect())
            })
            .unwrap_or_else(|| SimpleValue::Unknown("<range>".to_string())),
        kettu_parser::Expr::While { .. }
        | kettu_parser::Expr::For { .. }
        | kettu_parser::Expr::ForEach { .. } => evaluate_simulated_expr_value(expr, closures, source, env)
            .unwrap_or_else(|| SimpleValue::Unknown("<loop>".to_string())),
        kettu_parser::Expr::Call { func, args, .. } => {
            if let kettu_parser::Expr::Ident(id) = func.as_ref() {
                if let Some(closure) = closures
                    .iter()
                    .rev()
                    .find(|closure| closure.name == id.name)
                {
                    let mut closure_env = env.clone();
                    for (param, arg) in closure.params.iter().zip(args.iter()) {
                        closure_env
                            .insert(param.clone(), evaluate_ast_expr(arg, closures, source, env));
                    }
                    return evaluate_ast_expr(&closure.body, closures, source, &closure_env);
                }
            }
            SimpleValue::Unknown("<call>".to_string())
        }
        kettu_parser::Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => match evaluate_ast_expr(cond, closures, source, env) {
            SimpleValue::Bool(true) => {
                evaluate_tail_block_value(then_branch, closures, source, env)
            }
            SimpleValue::Bool(false) => evaluate_tail_block_value(
                else_branch.as_deref().unwrap_or(&[]),
                closures,
                source,
                env,
            ),
            _ => SimpleValue::Unknown("<if>".to_string()),
        },
        kettu_parser::Expr::Match {
            scrutinee, arms, ..
        } => {
            let scrutinee = evaluate_ast_expr(scrutinee, closures, source, env);
            if let Some((arm, bindings)) = select_match_arm(arms, &scrutinee, closures, source, env)
            {
                let mut match_env = env.clone();
                match_env.extend(bindings);
                evaluate_tail_block_value(&arm.body, closures, source, &match_env)
            } else {
                SimpleValue::Unknown("<match>".to_string())
            }
        }
        _ => SimpleValue::Unknown("<expr>".to_string()),
    }
}

fn evaluate_simulated_expr_value(
    expr: &kettu_parser::Expr,
    closures: &[ClosureRange],
    source: &str,
    env: &HashMap<String, SimpleValue>,
) -> Option<SimpleValue> {
    let mut env = env.clone();
    let mut trace = Vec::new();
    match simulate_expr_statement(expr, closures, source, &mut env, &mut trace, true) {
        StatementFlow::Return(Some(value)) => Some(value),
        _ => None,
    }
}

fn apply_lambda_expr(
    lambda: &kettu_parser::Expr,
    args: Vec<SimpleValue>,
    closures: &[ClosureRange],
    source: &str,
    env: &HashMap<String, SimpleValue>,
) -> SimpleValue {
    match lambda {
        kettu_parser::Expr::Lambda { params, body, .. } => {
            let mut lambda_env = env.clone();
            for (param, value) in params.iter().zip(args.into_iter()) {
                lambda_env.insert(param.name.clone(), value);
            }
            evaluate_ast_expr(body, closures, source, &lambda_env)
        }
        kettu_parser::Expr::Ident(id) => {
            if let Some(closure) = closures.iter().rev().find(|closure| closure.name == id.name) {
                let mut closure_env = env.clone();
                for (param, value) in closure.params.iter().zip(args.into_iter()) {
                    closure_env.insert(param.clone(), value);
                }
                evaluate_ast_expr(&closure.body, closures, source, &closure_env)
            } else {
                SimpleValue::Unknown("<lambda>".to_string())
            }
        }
        _ => SimpleValue::Unknown("<lambda>".to_string()),
    }
}

fn evaluate_tail_block_value(
    statements: &[kettu_parser::Statement],
    closures: &[ClosureRange],
    source: &str,
    env: &HashMap<String, SimpleValue>,
) -> SimpleValue {
    let mut env = env.clone();
    let mut trace = Vec::new();
    match simulate_statements(statements, closures, source, &mut env, &mut trace, true) {
        StatementFlow::Return(Some(value)) => value,
        _ => SimpleValue::Unknown("<block>".to_string()),
    }
}

fn evaluate_range_values(
    range: &kettu_parser::Expr,
    closures: &[ClosureRange],
    source: &str,
    env: &HashMap<String, SimpleValue>,
) -> Option<Vec<i64>> {
    let kettu_parser::Expr::Range {
        start,
        end,
        step,
        descending,
        ..
    } = range
    else {
        return None;
    };

    let start = match evaluate_ast_expr(start, closures, source, env) {
        SimpleValue::Int(value) => value,
        _ => return None,
    };
    let end = match evaluate_ast_expr(end, closures, source, env) {
        SimpleValue::Int(value) => value,
        _ => return None,
    };
    let step = match step {
        Some(step) => match evaluate_ast_expr(step, closures, source, env) {
            SimpleValue::Int(value) if value > 0 => value,
            _ => return None,
        },
        None => 1,
    };

    let mut values = Vec::new();
    if *descending {
        let mut current = start;
        while current >= end {
            values.push(current);
            current -= step;
        }
    } else {
        let mut current = start;
        while current <= end {
            values.push(current);
            current += step;
        }
    }

    Some(values)
}

fn select_match_arm<'a>(
    arms: &'a [kettu_parser::MatchArm],
    scrutinee: &SimpleValue,
    closures: &[ClosureRange],
    source: &str,
    env: &HashMap<String, SimpleValue>,
) -> Option<(&'a kettu_parser::MatchArm, HashMap<String, SimpleValue>)> {
    arms.iter().find_map(|arm| match &arm.pattern {
        kettu_parser::Pattern::Wildcard(_) => Some((arm, HashMap::new())),
        kettu_parser::Pattern::Literal(expr) => {
            (evaluate_ast_expr(expr, closures, source, env) == *scrutinee)
                .then(|| (arm, HashMap::new()))
        }
        kettu_parser::Pattern::Variant {
            case_name, binding, ..
        } => match scrutinee {
            SimpleValue::Result { discriminant, .. }
                if *discriminant == variant_discriminant(&case_name.name) =>
            {
                let mut bindings = HashMap::new();
                if let Some(binding) = binding {
                    if let Some(value) = variant_pattern_binding_value(scrutinee) {
                        bindings.insert(binding.name.clone(), value);
                    }
                }
                Some((arm, bindings))
            }
            _ => None,
        },
    })
}

fn record_trace_event(
    trace: &mut Vec<TraceEvent>,
    position: SourcePosition,
    env: &HashMap<String, SimpleValue>,
) {
    if position.line <= 0 {
        return;
    }
    trace.push(TraceEvent {
        line: position.line,
        column: position.column,
        env_before: env.clone(),
        runtime_subprogram_start_line: None,
        runtime_locals: HashMap::new(),
        runtime_pointer_derefs: HashMap::new(),
        runtime_closure_keys: Vec::new(),
    });
}

fn expr_start_position(expr: &kettu_parser::Expr, source: &str) -> SourcePosition {
    match expr {
        kettu_parser::Expr::Ident(id) => offset_to_position(source, id.span.start),
        kettu_parser::Expr::Integer(_, span)
        | kettu_parser::Expr::String(_, span)
        | kettu_parser::Expr::InterpolatedString(_, span)
        | kettu_parser::Expr::Bool(_, span)
        | kettu_parser::Expr::Call { span, .. }
        | kettu_parser::Expr::Field { span, .. }
        | kettu_parser::Expr::OptionalChain { span, .. }
        | kettu_parser::Expr::Try { span, .. }
        | kettu_parser::Expr::Binary { span, .. }
        | kettu_parser::Expr::If { span, .. }
        | kettu_parser::Expr::Assert(_, span)
        | kettu_parser::Expr::Not(_, span)
        | kettu_parser::Expr::Neg(_, span)
        | kettu_parser::Expr::StrLen(_, span)
        | kettu_parser::Expr::StrEq(_, _, span)
        | kettu_parser::Expr::ListLen(_, span)
        | kettu_parser::Expr::ListSet(_, _, _, span)
        | kettu_parser::Expr::ListPush(_, _, span)
        | kettu_parser::Expr::Lambda { span, .. }
        | kettu_parser::Expr::Map { span, .. }
        | kettu_parser::Expr::Filter { span, .. }
        | kettu_parser::Expr::Reduce { span, .. }
        | kettu_parser::Expr::RecordLiteral { span, .. }
        | kettu_parser::Expr::VariantLiteral { span, .. }
        | kettu_parser::Expr::Match { span, .. }
        | kettu_parser::Expr::While { span, .. }
        | kettu_parser::Expr::Range { span, .. }
        | kettu_parser::Expr::For { span, .. }
        | kettu_parser::Expr::ListLiteral { span, .. }
        | kettu_parser::Expr::Index { span, .. }
        | kettu_parser::Expr::Slice { span, .. }
        | kettu_parser::Expr::ForEach { span, .. }
        | kettu_parser::Expr::Await { span, .. }
        | kettu_parser::Expr::AtomicLoad { span, .. }
        | kettu_parser::Expr::AtomicStore { span, .. }
        | kettu_parser::Expr::AtomicAdd { span, .. }
        | kettu_parser::Expr::AtomicSub { span, .. }
        | kettu_parser::Expr::AtomicCmpxchg { span, .. }
        | kettu_parser::Expr::AtomicWait { span, .. }
        | kettu_parser::Expr::AtomicNotify { span, .. }
        | kettu_parser::Expr::Spawn { span, .. }
        | kettu_parser::Expr::ThreadJoin { span, .. }
        | kettu_parser::Expr::AtomicBlock { span, .. }
        | kettu_parser::Expr::SimdOp { span, .. }
        | kettu_parser::Expr::SimdForEach { span, .. } => offset_to_position(source, span.start),
    }
}

fn bin_op_symbol(op: kettu_parser::BinOp) -> &'static str {
    match op {
        kettu_parser::BinOp::Add => "+",
        kettu_parser::BinOp::Sub => "-",
        kettu_parser::BinOp::Mul => "*",
        kettu_parser::BinOp::Div => "/",
        kettu_parser::BinOp::Eq => "==",
        kettu_parser::BinOp::Ne => "!=",
        kettu_parser::BinOp::Lt => "<",
        kettu_parser::BinOp::Le => "<=",
        kettu_parser::BinOp::Gt => ">",
        kettu_parser::BinOp::Ge => ">=",
        kettu_parser::BinOp::And => "&&",
        kettu_parser::BinOp::Or => "||",
    }
}

fn extract_call_name(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut paren = None;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'(' {
            paren = Some(i);
            break;
        }
    }
    let idx = paren?;
    if idx == 0 {
        return None;
    }
    let mut start = idx;
    while start > 0 {
        let c = bytes[start - 1] as char;
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            start -= 1;
        } else {
            break;
        }
    }
    if start == idx {
        return None;
    }
    Some(line[start..idx].trim().to_string())
}

fn find_invoked_closure(
    session: &DebugSession,
    current_line: i64,
) -> Option<(
    usize,
    i64,
    HashMap<String, SimpleValue>,
    HashMap<String, SimpleValue>,
)> {
    if current_line <= 0 {
        return None;
    }

    let line = session.source_lines.get((current_line - 1) as usize)?;

    if let Some(call_name) = extract_call_name(line) {
        if let Some(closure_index) = session
            .closures
            .iter()
            .enumerate()
            .find(|(_, closure)| closure.name == call_name)
            .map(|(index, _)| index)
        {
            let closure = &session.closures[closure_index];
            let call_env = test_environment_for_line(session, current_line);
            let capture_bindings = capture_values_for_closure(session, closure_index, &call_env);
            let param_bindings = build_param_bindings(closure, line, &call_env);
            return Some((
                closure_index,
                current_line + 1,
                param_bindings,
                capture_bindings,
            ));
        }
    }

    let inline_closure = session
        .closures
        .iter()
        .enumerate()
        .filter(|(_, closure)| closure.inline_invocation_line == Some(current_line))
        .min_by_key(|(_, closure)| {
            let current_column = session.current_column.max(1);
            let distance = if closure.start_line == current_line && current_column == closure.start_column {
                0
            } else if closure.start_line == current_line && closure.start_column >= current_column {
                closure.start_column - current_column
            } else {
                i64::MAX / 4 + closure.start_column
            };
            (distance, closure.start_column)
        })
        .or_else(|| {
            session
                .closures
                .iter()
                .enumerate()
                .filter(|(_, closure)| closure.inline_invocation_line == Some(current_line))
                .max_by_key(|(_, closure)| (closure.start_line, closure.end_line, closure.start_column))
        });

    inline_closure.map(|(closure_index, closure)| {
        let parent_target = session
            .active_closures
            .last()
            .map(|active| FrameTarget::Closure(active.closure_index))
            .unwrap_or(FrameTarget::Test);
        let call_env = frame_base_environment(session, parent_target);
        let capture_bindings = capture_values_for_closure(session, closure_index, &call_env);
        let source_env = infer_values_in_range(
            &session.source_lines,
            1,
            current_line,
            &HashMap::new(),
        );
        let param_bindings = build_inline_hof_param_bindings(closure, line, &call_env)
            .or_else(|| build_inline_hof_param_bindings(closure, line, &source_env))
            .unwrap_or_else(|| {
                closure
                    .params
                    .iter()
                    .map(|param| (param.clone(), SimpleValue::Unknown("<param>".to_string())))
                    .collect()
            });
            (
                closure_index,
                current_line + 1,
                param_bindings,
                capture_bindings,
            )
        })
}

#[cfg(test)]
fn parse_closures(source_lines: &[String]) -> Vec<ClosureRange> {
    let source = source_lines.join("\n");
    let (ast, errors) = kettu_parser::parse_file(&source);
    if !errors.is_empty() {
        return Vec::new();
    }

    let Some(mut ast) = ast else {
        return Vec::new();
    };

    annotate_closure_captures(&mut ast);
    collect_closures_from_ast(&ast, &source)
}

#[cfg(test)]
fn infer_locals(source_lines: &[String], current_line: i64) -> Vec<Variable> {
    variables_from_env(&infer_values_in_range(
        source_lines,
        1,
        current_line,
        &HashMap::new(),
    ))
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
    chars.all(|c| c == '_' || c == '-' || c.is_ascii_alphanumeric())
}

fn frame_local_variables(session: &DebugSession, frame_id: i64) -> Vec<Variable> {
    let Some(target) = resolve_frame_target(session, frame_id) else {
        return Vec::new();
    };

    let target_bindings = bindings_for_target(session, target);

    match target {
        FrameTarget::Test => {
            let locals = test_environment_for_line(session, session.current_line);
            if target_bindings.is_empty() {
                variables_from_env(&locals)
            } else {
                variables_from_dwarf_bindings(
                    target_bindings.iter().filter(|binding| {
                        binding_visible_at_line(binding, session.current_line, None)
                    }),
                    &locals,
                )
            }
        }
        FrameTarget::Closure(closure_index) => {
            let closure = &session.closures[closure_index];
            let capture_names: HashSet<String> = closure.captures.iter().cloned().collect();
            let mut locals = HashMap::new();

            if let Some(active) = session.find_active_closure_state(closure_index) {
                for (name, value) in &active.param_bindings {
                    locals.insert(name.clone(), value.clone());
                }

                let mut seed_env = active.capture_bindings.clone();
                seed_env.extend(active.param_bindings.clone());
                let inferred = infer_values_in_range(
                    &session.source_lines,
                    closure.start_line + 1,
                    session.current_line,
                    &seed_env,
                );
                for (name, value) in inferred {
                    if !capture_names.contains(&name) {
                        locals.insert(name, value);
                    }
                }
            } else {
                for param in &closure.params {
                    locals.insert(param.clone(), SimpleValue::Unknown("<param>".to_string()));
                }

                let seed_env = frame_base_environment(session, FrameTarget::Closure(closure_index));
                let inferred = infer_values_in_range(
                    &session.source_lines,
                    closure.start_line + 1,
                    session.current_line,
                    &seed_env,
                );
                for (name, value) in inferred {
                    if !capture_names.contains(&name) {
                        locals.insert(name, value);
                    }
                }
            }

            if let Some(runtime_values) =
                runtime_binding_values_for_target(session, target, &locals)
            {
                for (name, value) in runtime_values {
                    if !capture_names.contains(&name) {
                        locals.insert(name, value);
                    }
                }
            }

            if target_bindings.is_empty() {
                variables_from_env(&locals)
            } else {
                variables_from_dwarf_bindings(
                    target_bindings.iter().filter(|binding| {
                        !capture_names.contains(&binding.name)
                            && binding_visible_at_line(
                                binding,
                                session.current_line,
                                Some(&capture_names),
                            )
                    }),
                    &locals,
                )
            }
        }
        FrameTarget::Function(func_idx) => {
            let sp = session.debug_symbols.subprograms.get(func_idx);
            let mut locals = HashMap::new();

            for binding in target_bindings.iter().filter(|binding| {
                matches!(binding.kind, DwarfBindingKind::Parameter)
            }) {
                if matches!(binding.kind, DwarfBindingKind::Parameter) {
                    locals.insert(
                        binding.name.clone(),
                        SimpleValue::Unknown("<param>".to_string()),
                    );
                }
            }

            let inferred = infer_values_in_range(
                &session.source_lines,
                sp.map(|subprogram| subprogram.start_line)
                    .or_else(|| resolve_runtime_debug_frame(session, target).map(|frame| frame.start_line))
                    .unwrap_or(session.current_line)
                    + 1,
                session.current_line,
                &locals,
            );
            for (name, value) in inferred {
                locals.insert(name, value);
            }

            if let Some(runtime_values) =
                runtime_binding_values_for_target(session, target, &locals)
            {
                for (name, value) in runtime_values {
                    locals.insert(name, value);
                }
            }

            if target_bindings.is_empty() {
                variables_from_env(&locals)
            } else {
                variables_from_dwarf_bindings(
                    target_bindings.iter().filter(|binding| {
                        binding_visible_at_line(binding, session.current_line, None)
                    }),
                    &locals,
                )
            }
        }
    }
}

fn frame_capture_variables(session: &DebugSession, frame_id: i64) -> Vec<Variable> {
    let Some(FrameTarget::Closure(closure_index)) = resolve_frame_target(session, frame_id) else {
        return Vec::new();
    };

    let captures = if let Some(active) = session.find_active_closure_state(closure_index) {
        active.capture_bindings.clone()
    } else {
        let base = frame_base_environment(session, FrameTarget::Closure(closure_index));
        capture_values_for_closure(session, closure_index, &base)
    };
    let mut captures = captures;

    let capture_names: HashSet<String> = session.closures[closure_index]
        .captures
        .iter()
        .cloned()
        .collect();

    if let Some(runtime_values) =
        runtime_binding_values_for_target(session, FrameTarget::Closure(closure_index), &captures)
    {
        for (name, value) in runtime_values {
            if capture_names.contains(&name) {
                captures.insert(name, value);
            }
        }
    }

    resolve_dwarf_subprogram(session, FrameTarget::Closure(closure_index))
        .map(|subprogram| {
            variables_from_dwarf_bindings(
                subprogram.bindings.iter().filter(|binding| {
                    binding.kind == DwarfBindingKind::Variable
                        && capture_names.contains(&binding.name)
                        && binding_visible_at_line(
                            binding,
                            session.current_line,
                            Some(&capture_names),
                        )
                }),
                &captures,
            )
        })
        .unwrap_or_else(|| variables_from_env(&captures))
}

fn frame_environment(session: &DebugSession, frame_id: i64) -> HashMap<String, SimpleValue> {
    let Some(target) = resolve_frame_target(session, frame_id) else {
        return HashMap::new();
    };

    match target {
        FrameTarget::Test => test_environment_for_line(session, session.current_line),
        FrameTarget::Closure(closure_index) => {
            let mut env = if let Some(active) = session.find_active_closure_state(closure_index) {
                active.capture_bindings.clone()
            } else {
                let base = frame_base_environment(session, FrameTarget::Closure(closure_index));
                capture_values_for_closure(session, closure_index, &base)
            };

            for variable in frame_local_variables(session, frame_id) {
                env.insert(
                    variable.name,
                    parse_variable_display(&variable.value, &variable.var_type),
                );
            }

            env
        }
        FrameTarget::Function(_) => {
            let mut env = HashMap::new();
            for variable in frame_local_variables(session, frame_id) {
                env.insert(
                    variable.name,
                    parse_variable_display(&variable.value, &variable.var_type),
                );
            }
            env
        }
    }
}

fn frame_base_environment(
    session: &DebugSession,
    target: FrameTarget,
) -> HashMap<String, SimpleValue> {
    let mut env = test_environment_for_line(session, session.current_line);

    if let FrameTarget::Closure(target_index) = target {
        for active in &session.active_closures {
            for (name, value) in &active.capture_bindings {
                env.insert(name.clone(), value.clone());
            }
            for (name, value) in &active.param_bindings {
                env.insert(name.clone(), value.clone());
            }
            if active.closure_index == target_index {
                break;
            }
        }
    }

    env
}

fn test_environment_for_line(session: &DebugSession, line: i64) -> HashMap<String, SimpleValue> {
    let Some(test) = session
        .tests
        .iter()
        .find(|test| line >= test.line && line <= test.end_line)
        .or_else(|| session.tests.get(session.current_test))
    else {
        return HashMap::new();
    };

    let overlay_runtime_locals = |mut env: HashMap<String, SimpleValue>, snapshot: &PausedRuntimeSnapshot| {
        let bindings = bindings_for_target(session, FrameTarget::Test);
        let start_line = resolve_runtime_debug_frame(session, FrameTarget::Test)
            .map(|frame| frame.start_line)
            .or_else(|| resolve_dwarf_subprogram(session, FrameTarget::Test).map(|subprogram| subprogram.start_line));
        let Some(start_line) = start_line else {
            return env;
        };
        if snapshot.subprogram_start_line != Some(start_line) {
            return env;
        }
        for binding in bindings
            .iter()
            .filter(|binding| binding_visible_at_line(binding, line, None))
        {
            let Some(local_index) = binding.local_index else {
                continue;
            };
            let Some(raw) = snapshot.locals.get(&local_index) else {
                continue;
            };
            env.insert(
                binding.name.clone(),
                runtime_value_to_simple(*raw, env.get(&binding.name)),
            );
        }
        env
    };

    if line == session.current_line {
        if let Some(snapshot) = paused_runtime_snapshot(session) {
            let base_env = current_trace_event(session)
                .map(|entry| entry.env_before.clone())
                .unwrap_or_else(|| {
                    infer_values_in_range(&session.source_lines, test.line, line, &HashMap::new())
                });
            return overlay_runtime_locals(base_env, &snapshot);
        }
    }

    if !test.trace.is_empty() {
        if line == session.current_line {
            if let Some(index) = session.current_trace_index {
                if let Some(entry) = test.trace.get(index) {
                    let snapshot = PausedRuntimeSnapshot {
                        subprogram_start_line: entry.runtime_subprogram_start_line,
                        locals: entry.runtime_locals.clone(),
                        pointer_derefs: entry.runtime_pointer_derefs.clone(),
                        closure_keys: entry.runtime_closure_keys.clone(),
                    };
                    return overlay_runtime_locals(entry.env_before.clone(), &snapshot);
                }
            }
        }

        if let Some(entry) = test.trace.iter().find(|entry| entry.line == line) {
            let snapshot = PausedRuntimeSnapshot {
                subprogram_start_line: entry.runtime_subprogram_start_line,
                locals: entry.runtime_locals.clone(),
                pointer_derefs: entry.runtime_pointer_derefs.clone(),
                closure_keys: entry.runtime_closure_keys.clone(),
            };
            return overlay_runtime_locals(entry.env_before.clone(), &snapshot);
        }
    }

    infer_values_in_range(&session.source_lines, test.line, line, &HashMap::new())
}

fn capture_values_for_closure(
    session: &DebugSession,
    closure_index: usize,
    env: &HashMap<String, SimpleValue>,
) -> HashMap<String, SimpleValue> {
    session.closures[closure_index]
        .captures
        .iter()
        .map(|capture| {
            (
                capture.clone(),
                env.get(capture)
                    .cloned()
                    .unwrap_or_else(|| SimpleValue::Unknown("<capture>".to_string())),
            )
        })
        .collect()
}

fn build_param_bindings(
    closure: &ClosureRange,
    line: &str,
    env: &HashMap<String, SimpleValue>,
) -> HashMap<String, SimpleValue> {
    let args = extract_call_arguments(line).unwrap_or_default();

    closure
        .params
        .iter()
        .enumerate()
        .map(|(index, param)| {
            let value = args
                .get(index)
                .map(|arg| infer_expr_value(arg, env))
                .unwrap_or_else(|| SimpleValue::Unknown("<param>".to_string()));
            (param.clone(), value)
        })
        .collect()
}

fn first_list_element(value: SimpleValue) -> Option<SimpleValue> {
    match value {
        SimpleValue::List(values) => values.into_iter().next(),
        _ => None,
    }
}

fn build_inline_hof_param_bindings(
    closure: &ClosureRange,
    line: &str,
    env: &HashMap<String, SimpleValue>,
) -> Option<HashMap<String, SimpleValue>> {
    let call_name = extract_call_name(line)?;
    let args = extract_call_arguments(line)?;

    match call_name.as_str() {
        "map" | "filter" if closure.params.len() == 1 => {
            let element = args
                .first()
                .map(|arg| infer_expr_value(arg, env))
                .and_then(first_list_element)?;
            Some(HashMap::from([(closure.params[0].clone(), element)]))
        }
        "reduce" if closure.params.len() >= 2 && args.len() >= 2 => {
            let acc = infer_expr_value(&args[1], env);
            let element = first_list_element(infer_expr_value(&args[0], env))?;
            let mut bindings = HashMap::new();
            bindings.insert(closure.params[0].clone(), acc);
            bindings.insert(closure.params[1].clone(), element);
            for param in closure.params.iter().skip(2) {
                bindings.insert(param.clone(), SimpleValue::Unknown("<param>".to_string()));
            }
            Some(bindings)
        }
        _ => None,
    }
}

fn extract_call_arguments(line: &str) -> Option<Vec<String>> {
    let start = line.find('(')?;
    let mut depth = 0i64;
    let mut in_string = false;
    let mut escape = false;
    let mut end = None;

    for (index, ch) in line.char_indices().skip(start) {
        if in_string {
            if escape {
                escape = false;
                continue;
            }
            match ch {
                '\\' => escape = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(index);
                    break;
                }
            }
            _ => {}
        }
    }

    let end = end?;
    let args = &line[start + 1..end];
    Some(split_top_level_arguments(args))
}

fn split_top_level_arguments(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut paren_depth = 0i64;
    let mut bracket_depth = 0i64;
    let mut brace_depth = 0i64;
    let mut in_string = false;
    let mut escape = false;

    for ch in input.chars() {
        if in_string {
            current.push(ch);
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                current.push(ch);
            }
            '(' => {
                paren_depth += 1;
                current.push(ch);
            }
            ')' => {
                paren_depth -= 1;
                current.push(ch);
            }
            '[' => {
                bracket_depth += 1;
                current.push(ch);
            }
            ']' => {
                bracket_depth -= 1;
                current.push(ch);
            }
            '{' => {
                brace_depth += 1;
                current.push(ch);
            }
            '}' => {
                brace_depth -= 1;
                current.push(ch);
            }
            ',' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    args.push(trimmed.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() {
        args.push(trimmed.to_string());
    }

    args
}

fn infer_values_in_range(
    source_lines: &[String],
    start_line: i64,
    end_line: i64,
    seed_env: &HashMap<String, SimpleValue>,
) -> HashMap<String, SimpleValue> {
    let mut values = seed_env.clone();
    if end_line < start_line || end_line <= 0 {
        return values;
    }

    let start_index = start_line.max(1).saturating_sub(1) as usize;
    let end_index = (end_line as usize).min(source_lines.len());

    for line in source_lines
        .iter()
        .skip(start_index)
        .take(end_index.saturating_sub(start_index))
    {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("shared let ") {
            if let Some((name, expr)) = split_assignment(rest) {
                if is_identifier(name) {
                    values.insert(name.to_string(), infer_expr_value(expr, &values));
                }
            }
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("let ") {
            if let Some((name, expr)) = split_assignment(rest) {
                if is_identifier(name) {
                    values.insert(name.to_string(), infer_expr_value(expr, &values));
                }
            }
            continue;
        }

        if let Some((name, expr)) = split_assignment(trimmed) {
            if is_identifier(name) {
                values.insert(name.to_string(), infer_expr_value(expr, &values));
            }
        }
    }

    values
}

fn variables_from_env(values: &HashMap<String, SimpleValue>) -> Vec<Variable> {
    let mut vars: Vec<Variable> = values
        .iter()
        .map(|(name, value)| Variable::from_value(name.clone(), value.clone()))
        .collect();
    vars.sort_by(|left, right| left.name.cmp(&right.name));
    vars
}

fn infer_expr_value(expr: &str, env: &HashMap<String, SimpleValue>) -> SimpleValue {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return SimpleValue::Unknown("<unknown>".to_string());
    }

    evaluate_expression(trimmed, env).unwrap_or_else(|_| parse_variant_or_literal(trimmed, env))
}

fn parse_variant_or_literal(expr: &str, env: &HashMap<String, SimpleValue>) -> SimpleValue {
    let trimmed = expr.trim();

    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        let inner = &trimmed[1..trimmed.len() - 1];
        return SimpleValue::List(
            split_top_level_arguments(inner)
                .into_iter()
                .map(|value| infer_expr_value(&value, env))
                .collect(),
        );
    }

    if trimmed == "none" {
        return simple_variant_value("none", None);
    }

    if let Some((constructor, rest)) = trimmed.split_once('(') {
        if let Some(inner) = rest.strip_suffix(')') {
            match constructor.trim() {
                "some" | "ok" | "err" => {
                    return simple_variant_value(
                        constructor.trim(),
                        Some(infer_expr_value(inner, env)),
                    );
                }
                _ => {}
            }
        }
    }

    parse_literal_or_unknown(trimmed)
}

fn parse_literal_or_unknown(expr: &str) -> SimpleValue {
    let trimmed = expr.trim();
    if trimmed == "true" {
        return SimpleValue::Bool(true);
    }
    if trimmed == "false" {
        return SimpleValue::Bool(false);
    }
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        return SimpleValue::String(trimmed[1..trimmed.len() - 1].to_string());
    }
    if let Ok(value) = trimmed.parse::<i64>() {
        return SimpleValue::Int(value);
    }
    if let Ok(value) = trimmed.parse::<f64>() {
        return SimpleValue::Float(value);
    }
    SimpleValue::Unknown(trimmed.to_string())
}

fn parse_variable_display(value: &str, var_type: &str) -> SimpleValue {
    match var_type {
        "bool" => SimpleValue::Bool(value == "true"),
        "i64" => value
            .parse::<i64>()
            .map(SimpleValue::Int)
            .unwrap_or_else(|_| SimpleValue::Unknown(value.to_string())),
        "f64" => value
            .parse::<f64>()
            .map(SimpleValue::Float)
            .unwrap_or_else(|_| SimpleValue::Unknown(value.to_string())),
        "string" => {
            if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
                SimpleValue::String(value[1..value.len() - 1].to_string())
            } else {
                SimpleValue::String(value.to_string())
            }
        }
        _ => SimpleValue::Unknown(value.to_string()),
    }
}

#[derive(Clone, Debug, PartialEq)]
enum EvalToken {
    LParen,
    RParen,
    Plus,
    Minus,
    Star,
    Slash,
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Ident(String),
}

struct EvalParser<'a> {
    tokens: Vec<EvalToken>,
    position: usize,
    env: &'a HashMap<String, SimpleValue>,
}

impl<'a> EvalParser<'a> {
    fn new(tokens: Vec<EvalToken>, env: &'a HashMap<String, SimpleValue>) -> Self {
        Self {
            tokens,
            position: 0,
            env,
        }
    }

    fn parse(mut self) -> Result<SimpleValue, String> {
        let value = self.parse_or()?;
        if self.peek().is_some() {
            return Err("Unexpected trailing tokens".to_string());
        }
        Ok(value)
    }

    fn parse_or(&mut self) -> Result<SimpleValue, String> {
        let mut value = self.parse_and()?;
        while matches!(self.peek(), Some(EvalToken::OrOr)) {
            self.position += 1;
            let rhs = self.parse_and()?;
            value = apply_binary_op(value, rhs, "||")?;
        }
        Ok(value)
    }

    fn parse_and(&mut self) -> Result<SimpleValue, String> {
        let mut value = self.parse_equality()?;
        while matches!(self.peek(), Some(EvalToken::AndAnd)) {
            self.position += 1;
            let rhs = self.parse_equality()?;
            value = apply_binary_op(value, rhs, "&&")?;
        }
        Ok(value)
    }

    fn parse_equality(&mut self) -> Result<SimpleValue, String> {
        let mut value = self.parse_comparison()?;
        loop {
            let op = match self.peek() {
                Some(EvalToken::EqEq) => "==",
                Some(EvalToken::NotEq) => "!=",
                _ => break,
            };
            self.position += 1;
            let rhs = self.parse_comparison()?;
            value = apply_binary_op(value, rhs, op)?;
        }
        Ok(value)
    }

    fn parse_comparison(&mut self) -> Result<SimpleValue, String> {
        let mut value = self.parse_additive()?;
        loop {
            let op = match self.peek() {
                Some(EvalToken::Lt) => "<",
                Some(EvalToken::Le) => "<=",
                Some(EvalToken::Gt) => ">",
                Some(EvalToken::Ge) => ">=",
                _ => break,
            };
            self.position += 1;
            let rhs = self.parse_additive()?;
            value = apply_binary_op(value, rhs, op)?;
        }
        Ok(value)
    }

    fn parse_additive(&mut self) -> Result<SimpleValue, String> {
        let mut value = self.parse_multiplicative()?;
        loop {
            let op = match self.peek() {
                Some(EvalToken::Plus) => "+",
                Some(EvalToken::Minus) => "-",
                _ => break,
            };
            self.position += 1;
            let rhs = self.parse_multiplicative()?;
            value = apply_binary_op(value, rhs, op)?;
        }
        Ok(value)
    }

    fn parse_multiplicative(&mut self) -> Result<SimpleValue, String> {
        let mut value = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Some(EvalToken::Star) => "*",
                Some(EvalToken::Slash) => "/",
                _ => break,
            };
            self.position += 1;
            let rhs = self.parse_unary()?;
            value = apply_binary_op(value, rhs, op)?;
        }
        Ok(value)
    }

    fn parse_unary(&mut self) -> Result<SimpleValue, String> {
        if matches!(self.peek(), Some(EvalToken::Minus)) {
            self.position += 1;
            let value = self.parse_unary()?;
            return match value {
                SimpleValue::Int(inner) => Ok(SimpleValue::Int(-inner)),
                SimpleValue::Float(inner) => Ok(SimpleValue::Float(-inner)),
                _ => Err("Unary '-' requires a numeric operand".to_string()),
            };
        }

        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<SimpleValue, String> {
        match self.next() {
            Some(EvalToken::Bool(value)) => Ok(SimpleValue::Bool(value)),
            Some(EvalToken::Int(value)) => Ok(SimpleValue::Int(value)),
            Some(EvalToken::Float(value)) => Ok(SimpleValue::Float(value)),
            Some(EvalToken::String(value)) => Ok(SimpleValue::String(value)),
            Some(EvalToken::Ident(name)) => self
                .env
                .get(&name)
                .cloned()
                .ok_or_else(|| format!("Unknown variable in evaluate: {}", name)),
            Some(EvalToken::LParen) => {
                let value = self.parse_or()?;
                match self.next() {
                    Some(EvalToken::RParen) => Ok(value),
                    _ => Err("Missing ')' in expression".to_string()),
                }
            }
            _ => Err("Expected an expression".to_string()),
        }
    }

    fn peek(&self) -> Option<&EvalToken> {
        self.tokens.get(self.position)
    }

    fn next(&mut self) -> Option<EvalToken> {
        let token = self.tokens.get(self.position).cloned();
        if token.is_some() {
            self.position += 1;
        }
        token
    }
}

fn evaluate_expression(
    expression: &str,
    env: &HashMap<String, SimpleValue>,
) -> Result<SimpleValue, String> {
    let tokens = tokenize_expression(expression)?;
    EvalParser::new(tokens, env).parse()
}

fn tokenize_expression(expression: &str) -> Result<Vec<EvalToken>, String> {
    let chars: Vec<char> = expression.chars().collect();
    let mut index = 0;
    let mut tokens = Vec::new();

    while index < chars.len() {
        let ch = chars[index];
        if ch.is_whitespace() {
            index += 1;
            continue;
        }

        match ch {
            '(' => {
                tokens.push(EvalToken::LParen);
                index += 1;
            }
            ')' => {
                tokens.push(EvalToken::RParen);
                index += 1;
            }
            '+' => {
                tokens.push(EvalToken::Plus);
                index += 1;
            }
            '-' => {
                tokens.push(EvalToken::Minus);
                index += 1;
            }
            '*' => {
                tokens.push(EvalToken::Star);
                index += 1;
            }
            '/' => {
                tokens.push(EvalToken::Slash);
                index += 1;
            }
            '&' if chars.get(index + 1) == Some(&'&') => {
                tokens.push(EvalToken::AndAnd);
                index += 2;
            }
            '|' if chars.get(index + 1) == Some(&'|') => {
                tokens.push(EvalToken::OrOr);
                index += 2;
            }
            '=' if chars.get(index + 1) == Some(&'=') => {
                tokens.push(EvalToken::EqEq);
                index += 2;
            }
            '!' if chars.get(index + 1) == Some(&'=') => {
                tokens.push(EvalToken::NotEq);
                index += 2;
            }
            '<' if chars.get(index + 1) == Some(&'=') => {
                tokens.push(EvalToken::Le);
                index += 2;
            }
            '>' if chars.get(index + 1) == Some(&'=') => {
                tokens.push(EvalToken::Ge);
                index += 2;
            }
            '<' => {
                tokens.push(EvalToken::Lt);
                index += 1;
            }
            '>' => {
                tokens.push(EvalToken::Gt);
                index += 1;
            }
            '"' => {
                let start = index + 1;
                index += 1;
                let mut escaped = false;
                let mut value = String::new();
                while index < chars.len() {
                    let current = chars[index];
                    if escaped {
                        value.push(current);
                        escaped = false;
                    } else if current == '\\' {
                        escaped = true;
                    } else if current == '"' {
                        break;
                    } else {
                        value.push(current);
                    }
                    index += 1;
                }
                if index >= chars.len() {
                    return Err(format!("Unterminated string starting at {}", start));
                }
                index += 1;
                tokens.push(EvalToken::String(value));
            }
            _ if ch.is_ascii_digit() => {
                let start = index;
                index += 1;
                let mut is_float = false;
                while index < chars.len()
                    && (chars[index].is_ascii_digit() || (!is_float && chars[index] == '.'))
                {
                    if chars[index] == '.' {
                        is_float = true;
                    }
                    index += 1;
                }
                let number: String = chars[start..index].iter().collect();
                if is_float {
                    tokens.push(EvalToken::Float(
                        number
                            .parse::<f64>()
                            .map_err(|_| format!("Invalid float literal: {}", number))?,
                    ));
                } else {
                    tokens.push(EvalToken::Int(
                        number
                            .parse::<i64>()
                            .map_err(|_| format!("Invalid integer literal: {}", number))?,
                    ));
                }
            }
            _ if ch == '_' || ch.is_ascii_alphabetic() => {
                let start = index;
                index += 1;
                while index < chars.len()
                    && (chars[index] == '_'
                        || chars[index] == '-'
                        || chars[index].is_ascii_alphanumeric())
                {
                    index += 1;
                }
                let ident: String = chars[start..index].iter().collect();
                match ident.as_str() {
                    "true" => tokens.push(EvalToken::Bool(true)),
                    "false" => tokens.push(EvalToken::Bool(false)),
                    _ => tokens.push(EvalToken::Ident(ident)),
                }
            }
            _ => return Err(format!("Unsupported token in expression: {}", ch)),
        }
    }

    Ok(tokens)
}

fn apply_binary_op(lhs: SimpleValue, rhs: SimpleValue, op: &str) -> Result<SimpleValue, String> {
    match op {
        "+" => match (lhs, rhs) {
            (SimpleValue::Int(lhs), SimpleValue::Int(rhs)) => Ok(SimpleValue::Int(lhs + rhs)),
            (SimpleValue::Float(lhs), SimpleValue::Float(rhs)) => Ok(SimpleValue::Float(lhs + rhs)),
            (SimpleValue::Int(lhs), SimpleValue::Float(rhs)) => {
                Ok(SimpleValue::Float(lhs as f64 + rhs))
            }
            (SimpleValue::Float(lhs), SimpleValue::Int(rhs)) => {
                Ok(SimpleValue::Float(lhs + rhs as f64))
            }
            (SimpleValue::String(lhs), SimpleValue::String(rhs)) => {
                Ok(SimpleValue::String(lhs + &rhs))
            }
            _ => Err("Operator '+' requires numeric or string operands".to_string()),
        },
        "-" => numeric_binary_op(lhs, rhs, |lhs, rhs| lhs - rhs),
        "*" => numeric_binary_op(lhs, rhs, |lhs, rhs| lhs * rhs),
        "/" => numeric_binary_op(lhs, rhs, |lhs, rhs| lhs / rhs),
        "==" => Ok(SimpleValue::Bool(lhs == rhs)),
        "!=" => Ok(SimpleValue::Bool(lhs != rhs)),
        "<" => compare_binary_op(lhs, rhs, |lhs, rhs| lhs < rhs),
        "<=" => compare_binary_op(lhs, rhs, |lhs, rhs| lhs <= rhs),
        ">" => compare_binary_op(lhs, rhs, |lhs, rhs| lhs > rhs),
        ">=" => compare_binary_op(lhs, rhs, |lhs, rhs| lhs >= rhs),
        "&&" => match (lhs, rhs) {
            (SimpleValue::Bool(lhs), SimpleValue::Bool(rhs)) => Ok(SimpleValue::Bool(lhs && rhs)),
            _ => Err("Operator '&&' requires boolean operands".to_string()),
        },
        "||" => match (lhs, rhs) {
            (SimpleValue::Bool(lhs), SimpleValue::Bool(rhs)) => Ok(SimpleValue::Bool(lhs || rhs)),
            _ => Err("Operator '||' requires boolean operands".to_string()),
        },
        _ => Err(format!("Unsupported operator: {}", op)),
    }
}

fn numeric_binary_op(
    lhs: SimpleValue,
    rhs: SimpleValue,
    op: impl Fn(f64, f64) -> f64,
) -> Result<SimpleValue, String> {
    match (lhs, rhs) {
        (SimpleValue::Int(lhs), SimpleValue::Int(rhs)) => {
            Ok(SimpleValue::Int(op(lhs as f64, rhs as f64) as i64))
        }
        (SimpleValue::Float(lhs), SimpleValue::Float(rhs)) => Ok(SimpleValue::Float(op(lhs, rhs))),
        (SimpleValue::Int(lhs), SimpleValue::Float(rhs)) => {
            Ok(SimpleValue::Float(op(lhs as f64, rhs)))
        }
        (SimpleValue::Float(lhs), SimpleValue::Int(rhs)) => {
            Ok(SimpleValue::Float(op(lhs, rhs as f64)))
        }
        _ => Err("Numeric operator requires numeric operands".to_string()),
    }
}

fn compare_binary_op(
    lhs: SimpleValue,
    rhs: SimpleValue,
    op: impl Fn(f64, f64) -> bool,
) -> Result<SimpleValue, String> {
    match (lhs, rhs) {
        (SimpleValue::Int(lhs), SimpleValue::Int(rhs)) => {
            Ok(SimpleValue::Bool(op(lhs as f64, rhs as f64)))
        }
        (SimpleValue::Float(lhs), SimpleValue::Float(rhs)) => Ok(SimpleValue::Bool(op(lhs, rhs))),
        (SimpleValue::Int(lhs), SimpleValue::Float(rhs)) => {
            Ok(SimpleValue::Bool(op(lhs as f64, rhs)))
        }
        (SimpleValue::Float(lhs), SimpleValue::Int(rhs)) => {
            Ok(SimpleValue::Bool(op(lhs, rhs as f64)))
        }
        _ => Err("Comparison operator requires numeric operands".to_string()),
    }
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
            let raw =
                line.split(':').nth(1).map(str::trim).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "Invalid DAP header")
                })?;
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
    use super::{
        infer_locals, parse_closures, parse_debug_symbols, DebugSession, DwarfBindingKind,
        ListedTest,
    };
    use kettu_codegen::CompileOptions;
    use serde_json::json;
    use std::collections::{BTreeSet, HashMap};
    use std::path::PathBuf;

    #[test]
    fn closure_ranges_are_detected() {
        let source = vec![
            "package local:test;".to_string(),
            "interface tests {".to_string(),
            "    @test".to_string(),
            "    t: func() -> bool {".to_string(),
            "        let f = |x| x + 1;".to_string(),
            "        return true;".to_string(),
            "    }".to_string(),
            "}".to_string(),
        ];

        let closures = parse_closures(&source);
        assert_eq!(closures.len(), 1);
        assert_eq!(closures[0].start_line, 5);
        assert_eq!(closures[0].end_line, 5);
    }

    #[test]
    fn closure_captures_are_collected() {
        let source = vec![
            "package local:test;".to_string(),
            "interface tests {".to_string(),
            "    @test".to_string(),
            "    t: func() -> bool {".to_string(),
            "        let x = 10;".to_string(),
            "        let add-x = |n| n + x;".to_string(),
            "        return true;".to_string(),
            "    }".to_string(),
            "}".to_string(),
        ];

        let closures = parse_closures(&source);
        assert_eq!(closures.len(), 1);
        assert_eq!(closures[0].captures, vec!["x"]);
    }

    #[test]
    fn stepping_always_progresses_or_ends() {
        let mut session = DebugSession::new();
        session.tests = vec![ListedTest {
            name: "t".to_string(),
            line: 10,
            end_line: 12,
            body: Vec::new(),
            trace: Vec::new(),
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
    fn evaluate_ast_expr_supports_record_literals_and_fields() {
        let source = r#"package local:test;
interface tests {
    @test
    t: func() -> bool {
        let r = { a: 1, b: 2, c: 3 };
        return r.b == 2;
    }
}"#;

        let (ast, errors) = kettu_parser::parse_file(source);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        let ast = ast.expect("ast");
        let func = ast
            .items
            .iter()
            .find_map(|item| match item {
                kettu_parser::TopLevelItem::Interface(interface) => interface
                    .items
                    .iter()
                    .find_map(|item| match item {
                        kettu_parser::InterfaceItem::Func(func) if func.name.name == "t" => {
                            Some(func)
                        }
                        _ => None,
                    }),
                _ => None,
            })
            .expect("test function");

        let body = func.body.as_ref().expect("function body");
        let mut env = HashMap::new();
        let record_value = match &body.statements[0] {
            kettu_parser::Statement::Let { value, .. } => {
                super::evaluate_ast_expr(value, &[], source, &env)
            }
            other => panic!("expected let statement, got {other:?}"),
        };
        env.insert("r".to_string(), record_value.clone());

        assert_eq!(record_value.display(), "{ a: 1, b: 2, c: 3 }");

        let field_value = match &body.statements[1] {
            kettu_parser::Statement::Return(Some(kettu_parser::Expr::Binary { lhs, .. })) => {
                super::evaluate_ast_expr(lhs, &[], source, &env)
            }
            other => panic!("expected return binary statement, got {other:?}"),
        };
        assert_eq!(field_value, super::SimpleValue::Int(2));
    }

    #[test]
    fn evaluate_ast_expr_supports_variant_literals_and_match() {
        let source = r#"package local:test;
interface tests {
    @test
    t: func() -> bool {
        let r = ok(10);
        return match r {
            #ok(v) => v == 10,
            #err(_) => false,
        };
    }
}"#;

        let (ast, errors) = kettu_parser::parse_file(source);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        let ast = ast.expect("ast");
        let func = ast
            .items
            .iter()
            .find_map(|item| match item {
                kettu_parser::TopLevelItem::Interface(interface) => interface
                    .items
                    .iter()
                    .find_map(|item| match item {
                        kettu_parser::InterfaceItem::Func(func) if func.name.name == "t" => {
                            Some(func)
                        }
                        _ => None,
                    }),
                _ => None,
            })
            .expect("test function");

        let body = func.body.as_ref().expect("function body");
        let mut env = HashMap::new();
        let variant_value = match &body.statements[0] {
            kettu_parser::Statement::Let { value, .. } => {
                super::evaluate_ast_expr(value, &[], source, &env)
            }
            other => panic!("expected let statement, got {other:?}"),
        };
        env.insert("r".to_string(), variant_value.clone());

        assert_eq!(variant_value.display(), "ok(10)");

        let match_value = match &body.statements[1] {
            kettu_parser::Statement::Return(Some(expr)) => {
                super::evaluate_ast_expr(expr, &[], source, &env)
            }
            other => panic!("expected return statement, got {other:?}"),
        };
        assert_eq!(match_value, super::SimpleValue::Bool(true));
    }

    #[test]
    fn parse_eval_expression_supports_list_transforms() {
        let expr = super::parse_eval_expression("map([1, 2, 3], |v| v + 1)")
            .expect("parse eval expression");
        let value = super::evaluate_ast_expr(&expr, &[], "", &HashMap::new());

        assert_eq!(
            value,
            super::SimpleValue::List(vec![
                super::SimpleValue::Int(2),
                super::SimpleValue::Int(3),
                super::SimpleValue::Int(4),
            ])
        );
    }

    #[test]
    fn parse_eval_expression_supports_try_and_list_set() {
        let try_expr = super::parse_eval_expression("ok(10)? + 5").expect("parse try expression");
        let try_value = super::evaluate_ast_expr(&try_expr, &[], "", &HashMap::new());
        assert_eq!(try_value, super::SimpleValue::Int(15));

        let list_set_expr = super::parse_eval_expression("list-set([1, 2, 3], 1, 9)")
            .expect("parse list-set expression");
        let list_set_value = super::evaluate_ast_expr(&list_set_expr, &[], "", &HashMap::new());
        assert_eq!(
            list_set_value,
            super::SimpleValue::List(vec![
                super::SimpleValue::Int(1),
                super::SimpleValue::Int(9),
                super::SimpleValue::Int(3),
            ])
        );
    }

    #[test]
    fn parse_eval_expression_preserves_structured_variant_payloads() {
        let ok_expr = super::parse_eval_expression("ok([1, 2])").expect("parse ok expression");
        let ok_value = super::evaluate_ast_expr(&ok_expr, &[], "", &HashMap::new());
        assert_eq!(ok_value.display(), "ok([1, 2])");

        let try_expr = super::parse_eval_expression("ok([1, 2])?")
            .expect("parse structured try expression");
        let try_value = super::evaluate_ast_expr(&try_expr, &[], "", &HashMap::new());
        assert_eq!(
            try_value,
            super::SimpleValue::List(vec![
                super::SimpleValue::Int(1),
                super::SimpleValue::Int(2),
            ])
        );

        let optional_expr = super::parse_eval_expression("some({ total: 9 })?.total")
            .expect("parse optional-chain expression");
        let optional_value = super::evaluate_ast_expr(&optional_expr, &[], "", &HashMap::new());
        assert_eq!(optional_value.display(), "some(9)");
    }

    #[test]
    fn offset_to_line_col_tracks_columns() {
        let source = "first\n  second\nthird";

        assert_eq!(super::offset_to_line_col(source, 0), (1, 1));
        assert_eq!(super::offset_to_line_col(source, 6), (2, 1));
        assert_eq!(super::offset_to_line_col(source, 8), (2, 3));
        assert_eq!(super::offset_to_line_col(source, source.len()), (3, 6));
    }

    #[test]
    fn stack_trace_includes_closure_frame() {
        let lines = vec![
            "package local:test;".to_string(),
            "interface tests {".to_string(),
            "    @test".to_string(),
            "    t: func() -> bool {".to_string(),
            "        let y = |n|".to_string(),
            "            n + 1;".to_string(),
            "        let total = y(2);".to_string(),
            "        return total > 0;".to_string(),
            "    }".to_string(),
            "}".to_string(),
        ];

        let closures = parse_closures(&lines);
        let mut session = DebugSession::new();
        session.program = Some(PathBuf::from("/tmp/file.kettu"));
        session.source_lines = lines;
        session.tests = vec![ListedTest {
            name: "t".into(),
            line: 4,
            end_line: 10,
            body: Vec::new(),
            trace: Vec::new(),
        }];
        session.current_line = 6; // inside closure
        session.closures = closures.clone();

        let frames = super::build_stack_frames(&session);
        assert!(frames.len() >= 2);
        assert_eq!(frames[0].get("name"), Some(&json!("y")));
        assert_eq!(
            frames[1].get("name"),
            Some(&json!('@'.to_string() + "test t"))
        );
    }

    #[test]
    fn evaluate_expression_reads_known_variables() {
        let mut env = std::collections::HashMap::new();
        env.insert("base".to_string(), super::SimpleValue::Int(10));
        env.insert("n".to_string(), super::SimpleValue::Int(5));

        let value = super::evaluate_expression("base + n", &env).unwrap();
        assert_eq!(value, super::SimpleValue::Int(15));
    }

    #[test]
    fn parse_debug_symbols_reads_dwarf_bindings() {
        let source = r#"package local:test;
interface math {
    add: func(a: s32, b: s32) -> s32 {
        let sum = a + b;
        return sum;
    }
}"#;
        let (ast, errors) = kettu_parser::parse_file(source);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        let ast = ast.expect("ast");

        let wasm = kettu_codegen::build_core_module(
            &ast,
            &CompileOptions {
                core_only: true,
                memory_pages: 1,
                wasip3: false,
                threads: false,
                emit_dwarf: true,
                keep_names: true,
                debug_source: Some(source.to_string()),
                debug_path: Some("math.kettu".to_string()),
                emit_debug_hooks: false,
            },
        )
        .expect("build debug wasm");

        let symbols = parse_debug_symbols(&wasm).expect("parse debug symbols");
        let add = symbols
            .subprograms
            .iter()
            .find(|subprogram| subprogram.name == "add" || subprogram.name.ends_with("#add"))
            .expect("add subprogram");

        assert!(add.bindings.iter().any(|binding| {
            binding.name == "a"
                && binding.kind == DwarfBindingKind::Parameter
                && binding.decl_line == 3
                && binding.local_index == Some(0)
        }));
        assert!(add.bindings.iter().any(|binding| {
            binding.name == "b"
                && binding.kind == DwarfBindingKind::Parameter
                && binding.decl_line == 3
                && binding.local_index == Some(1)
        }));
        assert!(add.bindings.iter().any(|binding| {
            binding.name == "sum"
                && binding.kind == DwarfBindingKind::Variable
                && binding.decl_line == 4
                && binding.local_index == Some(2)
        }));
    }

    #[test]
    fn runtime_debug_artifacts_collect_trace_for_test() {
        let source = r#"package local:test;
interface tests {
    @test
    t: func() -> bool {
        let x = 1;
        let y = x + 2;
        return y == 3;
    }
}"#;

        let (ast, errors) = kettu_parser::parse_file(source);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        let ast = ast.expect("ast");
        let program = PathBuf::from("runtime_debug_test.kettu");

        let artifacts = super::RuntimeDebugArtifacts::compile(&program, source, &ast)
            .expect("compile runtime debug artifacts");
        let events = artifacts
            .collect_trace_for_test("t")
            .expect("collect runtime trace");

        assert!(!events.is_empty(), "expected runtime trace events");
        assert!(events.iter().any(|event| event.line >= 5));
    }

    #[test]
    fn runtime_debug_artifacts_parse_debug_resume_metadata() {
        let source = r#"package local:test;
interface tests {
    @test
    t: func() -> bool {
        let base = 10;
        let add = |n| n + base;
        return add(2) == 12;
    }
}"#;

        let (ast, errors) = kettu_parser::parse_file(source);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        let ast = ast.expect("ast");
        let program = PathBuf::from("runtime_debug_resume_metadata_test.kettu");

        let artifacts = super::RuntimeDebugArtifacts::compile(&program, source, &ast)
            .expect("compile runtime debug artifacts");

        let test_frame = artifacts
            .debug_resume_frames
            .iter()
            .find(|frame| frame.name == "t")
            .expect("expected test frame metadata");
        assert_eq!(test_frame.start_line, 3);
        assert_eq!(test_frame.slot_count, 2);
        assert!(test_frame.byte_size >= 16);
        assert!(test_frame.bindings.iter().any(|binding| {
            binding.name == "base" && binding.kind == "variable" && binding.decl_line == 5
        }));

        let lambda_frame = artifacts
            .debug_resume_frames
            .iter()
            .find(|frame| frame.name == "lambda#0")
            .expect("expected lambda frame metadata");
        assert_eq!(lambda_frame.start_line, 6);
        assert!(lambda_frame.state_offset >= test_frame.state_offset + test_frame.byte_size);
        assert!(lambda_frame.bindings.iter().any(|binding| {
            binding.name == "base" && binding.kind == "variable"
        }));
        assert!(lambda_frame.bindings.iter().any(|binding| {
            binding.name == "n" && binding.kind == "parameter" && binding.local_index == 0
        }));
    }

    #[test]
    fn runtime_debug_session_pauses_at_breakpoint() {
        let source = r#"package local:test;
interface tests {
    @test
    t: func() -> bool {
        let x = 1;
        let y = x + 2;
        return y == 3;
    }
}"#;

        let (ast, errors) = kettu_parser::parse_file(source);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        let ast = ast.expect("ast");
        let program = PathBuf::from("runtime_debug_pause_test.kettu");

        let artifacts = super::RuntimeDebugArtifacts::compile(&program, source, &ast)
            .expect("compile runtime debug artifacts");
        let mut session = artifacts.start_test("t").expect("start runtime session");
        let breakpoints = BTreeSet::from([super::BreakpointLocation::new(5, None)]);

        let pause = session
            .run_until_pause(breakpoints)
            .expect("run until pause")
            .expect("pause event");

        assert_eq!(pause.reason, "breakpoint");
        assert_eq!(pause.line, 5);
        assert_eq!(pause.column, 1);
        assert!(session.events().iter().any(|event| event.line == 5));
    }

    #[test]
    fn runtime_debug_session_surfaces_real_runtime_faults() {
        let source = r#"package local:test;
interface tests {
    @test
    t: func() -> bool {
        let x = 1;
        let y = x / 0;
        return y == 3;
    }
}"#;

        let (ast, errors) = kettu_parser::parse_file(source);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        let ast = ast.expect("ast");
        let program = PathBuf::from("runtime_debug_fault_test.kettu");

        let artifacts = super::RuntimeDebugArtifacts::compile(&program, source, &ast)
            .expect("compile runtime debug artifacts");
        let mut session = artifacts.start_test("t").expect("start runtime session");

        let error = session
            .run_until_pause(BTreeSet::new())
            .expect_err("expected runtime fault");

        assert!(error.contains("Failed to execute test export"));
        assert!(session.store.data().pause_event.is_none());
    }

    #[test]
    fn debug_session_owns_runtime_pause_state() {
        let source = r#"package local:test;
interface tests {
    @test
    t: func() -> bool {
        let x = 1;
        let y = x + 2;
        return y == 3;
    }
}"#;

        let (ast, errors) = kettu_parser::parse_file(source);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        let ast = ast.expect("ast");
        let program = PathBuf::from("runtime_debug_session_owner_test.kettu");
        let runtime_debug_artifacts = super::RuntimeDebugArtifacts::compile(&program, source, &ast)
            .expect("compile runtime debug artifacts");

        let mut session = DebugSession::new();
        session.program = Some(program.clone());
        session.tests = vec![ListedTest {
            name: "t".into(),
            line: 4,
            end_line: 8,
            body: Vec::new(),
            trace: Vec::new(),
        }];
        session.runtime_debug_artifacts = Some(runtime_debug_artifacts);
        session
            .breakpoints
            .insert(super::normalize_path_key(&program.display().to_string()), BTreeSet::from([
                super::BreakpointLocation::new(5, None),
            ]));

        assert!(session.start_runtime_session_for_test(0).expect("start runtime session"));
        let pause = session
            .run_active_runtime_until_pause()
            .expect("run active runtime until pause")
            .expect("pause event");

        assert_eq!(pause.line, 5);
        assert_eq!(session.current_line, 5);
        assert_eq!(session.current_column, 1);
        assert!(session.active_runtime_session.is_some());
        assert!(session.runtime_pause_event.is_some());
    }

    #[test]
    fn runtime_continue_syncs_trace_cursor_on_pause() {
        let source = r#"package local:test;
interface tests {
    @test
    t: func() -> bool {
        let x = 1;
        let y = x + 2;
        return y == 3;
    }
}"#;

        let (ast, errors) = kettu_parser::parse_file(source);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        let ast = ast.expect("ast");
        let program = PathBuf::from("runtime_debug_continue_sync_test.kettu");
        let runtime_debug_artifacts = super::RuntimeDebugArtifacts::compile(&program, source, &ast)
            .expect("compile runtime debug artifacts");

        let mut tests = vec![ListedTest {
            name: "t".into(),
            line: 4,
            end_line: 8,
            body: Vec::new(),
            trace: Vec::new(),
        }];
        super::build_runtime_test_traces(&runtime_debug_artifacts, &mut tests)
            .expect("build runtime test traces");

        let mut session = DebugSession::new();
        session.program = Some(program.clone());
        session.tests = tests;
        session.current_line = 3;
        session.runtime_debug_artifacts = Some(runtime_debug_artifacts);
        session
            .breakpoints
            .insert(super::normalize_path_key(&program.display().to_string()), BTreeSet::from([
                super::BreakpointLocation::new(6, None),
            ]));

        let outcome = session
            .run_live_runtime_until_breakpoint_or_end()
            .expect("runtime-backed continue outcome");

        match outcome {
            super::StopOutcome::Stopped(reason) => assert_eq!(reason, "breakpoint"),
            super::StopOutcome::Terminated => panic!("expected breakpoint stop"),
        }

        assert_eq!(session.current_line, 6);
        assert!(session.current_trace_index.is_some());
        assert!(session.active_runtime_session.is_some());
        assert!(session.runtime_pause_event.is_some());
    }

    #[test]
    fn synthetic_step_discards_live_runtime_session() {
        let source = r#"package local:test;
interface tests {
    @test
    t: func() -> bool {
        let x = 1;
        let y = x + 2;
        return y == 3;
    }
}"#;

        let (ast, errors) = kettu_parser::parse_file(source);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        let ast = ast.expect("ast");
        let program = PathBuf::from("runtime_debug_step_boundary_test.kettu");
        let runtime_debug_artifacts = super::RuntimeDebugArtifacts::compile(&program, source, &ast)
            .expect("compile runtime debug artifacts");

        let mut tests = vec![ListedTest {
            name: "t".into(),
            line: 4,
            end_line: 8,
            body: Vec::new(),
            trace: Vec::new(),
        }];
        super::build_runtime_test_traces(&runtime_debug_artifacts, &mut tests)
            .expect("build runtime test traces");

        let mut session = DebugSession::new();
        session.program = Some(program.clone());
        session.tests = tests;
        session.current_line = 3;
        session.runtime_debug_artifacts = Some(runtime_debug_artifacts);
        session
            .breakpoints
            .insert(super::normalize_path_key(&program.display().to_string()), BTreeSet::from([
                super::BreakpointLocation::new(5, None),
            ]));

        let outcome = session
            .run_live_runtime_until_breakpoint_or_end()
            .expect("runtime-backed continue outcome");
        assert!(matches!(outcome, super::StopOutcome::Stopped("breakpoint")));
        assert!(session.active_runtime_session.is_some());

        session.reset_runtime_pause_state();
        let step_outcome = session.step_once_or_end("next");

        assert!(matches!(step_outcome, super::StopOutcome::Stopped("step")));
        assert!(session.active_runtime_session.is_none());
        assert!(!session.can_continue_with_runtime());
    }

    #[test]
    fn paused_runtime_snapshot_survives_without_trace_cursor() {
        let source = r#"package local:test;
interface tests {
    @test
    t: func() -> bool {
        let x = 1;
        let y = x + 2;
        return y == 3;
    }
}"#;

        let (ast, errors) = kettu_parser::parse_file(source);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        let ast = ast.expect("ast");
        let program = PathBuf::from("runtime_debug_snapshot_test.kettu");
        let runtime_debug_artifacts = super::RuntimeDebugArtifacts::compile(&program, source, &ast)
            .expect("compile runtime debug artifacts");

        let mut tests = vec![ListedTest {
            name: "t".into(),
            line: 4,
            end_line: 8,
            body: Vec::new(),
            trace: Vec::new(),
        }];
        super::build_runtime_test_traces(&runtime_debug_artifacts, &mut tests)
            .expect("build runtime test traces");

        let mut session = DebugSession::new();
        session.program = Some(program.clone());
        session.tests = tests;
        session.current_line = 3;
        session.runtime_debug_artifacts = Some(runtime_debug_artifacts);
        session
            .breakpoints
            .insert(super::normalize_path_key(&program.display().to_string()), BTreeSet::from([
                super::BreakpointLocation::new(5, None),
            ]));

        let outcome = session
            .run_live_runtime_until_breakpoint_or_end()
            .expect("runtime-backed continue outcome");
        assert!(matches!(outcome, super::StopOutcome::Stopped("breakpoint")));

        session.current_trace_index = None;
        let snapshot = super::paused_runtime_snapshot(&session).expect("paused runtime snapshot");

        assert!(!snapshot.locals.is_empty());
    }

    #[test]
    fn paused_runtime_snapshot_keeps_function_frame_without_trace_cursor() {
        let source = r#"package local:test;
interface tests {
    helper: func(a: s32) -> s32 {
        let inc = a + 1;
        return inc;
    }

    @test
    t: func() -> bool {
        let value = helper(2);
        return value == 3;
    }
}"#;

        let (ast, errors) = kettu_parser::parse_file(source);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        let ast = ast.expect("ast");
        let program = PathBuf::from("runtime_debug_function_frame_test.kettu");
        let runtime_debug_artifacts = super::RuntimeDebugArtifacts::compile(&program, source, &ast)
            .expect("compile runtime debug artifacts");
        let debug_symbols = super::build_debug_symbols(&program, source, &ast)
            .expect("build debug symbols");

        let mut tests = vec![ListedTest {
            name: "t".into(),
            line: 8,
            end_line: 11,
            body: Vec::new(),
            trace: Vec::new(),
        }];
        super::build_runtime_test_traces(&runtime_debug_artifacts, &mut tests)
            .expect("build runtime test traces");

        let mut session = DebugSession::new();
        session.program = Some(program.clone());
        session.source_lines = source.lines().map(str::to_string).collect();
        session.tests = tests;
        session.closures = super::parse_closures(&session.source_lines);
        session.debug_symbols = debug_symbols;
        session.current_line = 7;
        session.runtime_debug_artifacts = Some(runtime_debug_artifacts);
        session
            .breakpoints
            .insert(super::normalize_path_key(&program.display().to_string()), BTreeSet::from([
                super::BreakpointLocation::new(5, None),
            ]));

        let outcome = session
            .run_live_runtime_until_breakpoint_or_end()
            .expect("runtime-backed continue outcome");
        assert!(matches!(outcome, super::StopOutcome::Stopped("breakpoint")));

        session.current_trace_index = None;
        let frames = super::build_stack_frames(&session);

        assert!(frames.iter().any(|frame| {
            frame["name"]
                .as_str()
                .map(|name| name == "helper" || name.ends_with("#helper"))
                .unwrap_or(false)
        }));
        assert!(frames.iter().any(|frame| frame["name"] == json!("@test t")));
    }

    #[test]
    fn runtime_metadata_recovers_function_frame_without_dwarf_symbols() {
        let source = r#"package local:test;
interface tests {
    helper: func(a: s32) -> s32 {
        let inc = a + 1;
        return inc;
    }

    @test
    t: func() -> bool {
        let value = helper(2);
        return value == 3;
    }
}"#;

        let (ast, errors) = kettu_parser::parse_file(source);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        let ast = ast.expect("ast");
        let program = PathBuf::from("runtime_debug_metadata_function_frame_test.kettu");
        let runtime_debug_artifacts = super::RuntimeDebugArtifacts::compile(&program, source, &ast)
            .expect("compile runtime debug artifacts");

        let mut tests = vec![ListedTest {
            name: "t".into(),
            line: 8,
            end_line: 11,
            body: Vec::new(),
            trace: Vec::new(),
        }];
        super::build_runtime_test_traces(&runtime_debug_artifacts, &mut tests)
            .expect("build runtime test traces");

        let mut session = DebugSession::new();
        session.program = Some(program.clone());
        session.source_lines = source.lines().map(str::to_string).collect();
        session.tests = tests;
        session.closures = super::parse_closures(&session.source_lines);
        session.current_line = 7;
        session.runtime_debug_artifacts = Some(runtime_debug_artifacts);
        session
            .breakpoints
            .insert(super::normalize_path_key(&program.display().to_string()), BTreeSet::from([
                super::BreakpointLocation::new(4, None),
            ]));

        let outcome = session
            .run_live_runtime_until_breakpoint_or_end()
            .expect("runtime-backed continue outcome");
        assert!(matches!(outcome, super::StopOutcome::Stopped("breakpoint")));

        session.current_trace_index = None;
        session.capture_paused_frames();

        let helper_frame = session
            .current_frame_descriptors()
            .into_iter()
            .find(|frame| frame.name == "helper" || frame.name.ends_with("#helper"))
            .expect("expected helper frame from runtime metadata");

        let locals = super::frame_local_variables(&session, helper_frame.id);
        assert!(locals.iter().any(|var| var.name == "a" && var.value == "2"));
        assert!(locals.iter().any(|var| var.name == "inc"));
    }

    #[test]
    fn paused_frame_snapshot_keeps_frame_ids_stable() {
        let lines = vec![
            "package local:test;".to_string(),
            "interface tests {".to_string(),
            "    @test".to_string(),
            "    t: func() -> bool {".to_string(),
            "        let y = |n|".to_string(),
            "            n + 1;".to_string(),
            "        let total = y(2);".to_string(),
            "        return total > 0;".to_string(),
            "    }".to_string(),
            "}".to_string(),
        ];

        let closures = parse_closures(&lines);
        let mut session = DebugSession::new();
        session.program = Some(PathBuf::from("/tmp/frame_snapshot_test.kettu"));
        session.source_lines = lines;
        session.tests = vec![ListedTest {
            name: "t".into(),
            line: 4,
            end_line: 10,
            body: Vec::new(),
            trace: Vec::new(),
        }];
        session.current_line = 6;
        session.current_column = 1;
        session.closures = closures;
        session.capture_paused_frames();

        let first_frames = session.current_frame_descriptors();
        assert!(matches!(
            first_frames.first().map(|frame| frame.target),
            Some(super::FrameTarget::Closure(_))
        ));
        let first_frame_id = first_frames[0].id;

        session.current_line = 8;
        session.active_closures.clear();

        let second_frames = session.current_frame_descriptors();
        assert_eq!(second_frames[0].id, first_frame_id);
        assert_eq!(second_frames[0].name, first_frames[0].name);
        assert!(matches!(
            super::resolve_frame_target(&session, first_frame_id),
            Some(super::FrameTarget::Closure(_))
        ));

        session.clear_paused_frames();
        let rebuilt_frames = session.current_frame_descriptors();
        assert_eq!(rebuilt_frames.len(), 1);
        assert_eq!(rebuilt_frames[0].name, "@test t");
    }
}
