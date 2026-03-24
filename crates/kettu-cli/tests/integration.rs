//! Integration tests for the full Kettu compilation pipeline

use std::io::Write;
use std::process::Command;
use tempfile::NamedTempFile;

#[test]
fn test_parse_command() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "package local:test;").unwrap();
    writeln!(file, "interface api {{").unwrap();
    writeln!(file, "    greet: func(name: string) -> string;").unwrap();
    writeln!(file, "}}").unwrap();

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "kettu-cli",
            "--",
            "parse",
            file.path().to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run kettu parse");

    assert!(output.status.success(), "Parse command should succeed");
}

#[test]
fn test_check_command() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "package local:test;").unwrap();
    writeln!(file, "interface api {{").unwrap();
    writeln!(file, "    greet: func(name: string) -> string;").unwrap();
    writeln!(file, "}}").unwrap();

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "kettu-cli",
            "--",
            "check",
            file.path().to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run kettu check");

    assert!(output.status.success(), "Check command should succeed");
}

#[test]
fn test_emit_wit_command() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "package local:test;").unwrap();
    writeln!(file, "interface api {{").unwrap();
    writeln!(file, "    greet: func(name: string) -> string {{").unwrap();
    writeln!(file, "        format(name);").unwrap();
    writeln!(file, "    }}").unwrap();
    writeln!(file, "}}").unwrap();

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "kettu-cli",
            "--",
            "emit-wit",
            file.path().to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run kettu emit-wit");

    assert!(output.status.success(), "Emit-wit command should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("interface api"),
        "Output should contain interface"
    );
    assert!(
        stdout.contains("greet: func"),
        "Output should contain function"
    );
    // Should NOT contain function body
    assert!(
        !stdout.contains("format"),
        "Output should NOT contain function body"
    );
}

#[test]
fn test_build_core_command() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "package local:test;").unwrap();
    writeln!(file, "interface api {{").unwrap();
    writeln!(file, "    greet: func(x: s32) -> s32 {{").unwrap();
    writeln!(file, "        return x;").unwrap();
    writeln!(file, "    }}").unwrap();
    writeln!(file, "}}").unwrap();

    let output_file = NamedTempFile::new().unwrap();

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "kettu-cli",
            "--",
            "build",
            "--core",
            file.path().to_str().unwrap(),
            "-o",
            output_file.path().to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run kettu build");

    assert!(
        output.status.success(),
        "Build command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Check output file exists and has valid WASM magic
    let wasm = std::fs::read(output_file.path()).unwrap();
    assert!(wasm.len() > 8, "Output should be non-empty WASM");
    assert_eq!(
        &wasm[0..4],
        b"\0asm",
        "Output should have WASM magic number"
    );
}

#[test]
fn test_build_with_expressions() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "package local:test;").unwrap();
    writeln!(file, "interface math {{").unwrap();
    writeln!(file, "    add: func(a: s32, b: s32) -> s32 {{").unwrap();
    writeln!(file, "        let sum = a + b;").unwrap();
    writeln!(file, "        return sum;").unwrap();
    writeln!(file, "    }}").unwrap();
    writeln!(file, "}}").unwrap();

    let output_file = NamedTempFile::new().unwrap();

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "kettu-cli",
            "--",
            "build",
            "--core",
            file.path().to_str().unwrap(),
            "-o",
            output_file.path().to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run kettu build");

    assert!(
        output.status.success(),
        "Build with expressions should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let wasm = std::fs::read(output_file.path()).unwrap();
    assert!(wasm.len() > 8, "Output should be valid WASM");
    assert_eq!(
        &wasm[0..4],
        b"\0asm",
        "Output should have WASM magic number"
    );
}

#[test]
fn test_check_type_error() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "package local:test;").unwrap();
    writeln!(file, "interface broken {{").unwrap();
    writeln!(file, "    bad: func(x: s32) -> s32 {{").unwrap();
    writeln!(file, "        return undefined_var;").unwrap();
    writeln!(file, "    }}").unwrap();
    writeln!(file, "}}").unwrap();

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "kettu-cli",
            "--",
            "check",
            file.path().to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run kettu check");

    // The check command should report an error for undefined variable
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}{}", stdout, stderr);

    // Check should detect the unknown variable
    assert!(
        combined.contains("Unknown variable")
            || combined.contains("undefined_var")
            || combined.contains("error"),
        "Check should report unknown variable error: {}",
        combined
    );
}
