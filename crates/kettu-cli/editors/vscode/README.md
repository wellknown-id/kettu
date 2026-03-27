# Kettu VS Code Extension

Language support for Kettu (`.kettu`) and WIT (`.wit`) files, including syntax highlighting and LSP integration.

## Installation

### Quick Start (Development)

```bash
# 1. Build the Kettu CLI (includes the LSP server)
cd /path/to/kettu
cargo build --release --bin kettu

# 2. Install extension dependencies
cd editors/vscode
npm install

# 3. Link extension to VS Code
ln -s "$(pwd)" ~/.vscode/extensions/kettu
```

### Package as VSIX

```bash
npm install -g @vscode/vsce
cd editors/vscode
vsce package
code --install-extension kettu-0.1.0.vsix
```

## Configuration

| Setting | Description |
|---------|-------------|
| `kettu.serverPath` | Path to `kettu` binary. Leave empty to use PATH. |

Example `settings.json`:
```json
{
  "kettu.serverPath": "/path/to/kettu/target/release/kettu"
}
```

## Features

- **Syntax Highlighting** — TextMate grammars for `.kettu` and `.wit` files
- **Diagnostics** — Parse and type-check errors with line/column
- **Hover** — Documentation for keywords and types
- **Completion** — Keywords, primitive types, and user-defined types
- **Document Symbols** — Outline view (Ctrl+Shift+O)
- **Go to Definition** — Jump to definitions (F12)
- **Test Debugging** — Run `@test` functions in the current `.kettu` file with breakpoints

## Debugging Kettu Tests

1. Open a `.kettu` file containing `@test` functions.
2. Set breakpoints on test function definition lines.
3. Run **Kettu: Debug Tests in Current File** from the Command Palette.
4. Start debugging and use Continue/Step to advance between paused tests.

Notes:
- Breakpoints map to discovered test function lines.
- The debug adapter runs one test at a time via `kettu test --filter <name> --exact`.

## Development

Open the `kettu` workspace in VS Code and press F5 to launch the Extension Development Host.
