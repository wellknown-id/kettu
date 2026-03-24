# kettu-lsp

Language server for the Kettu language.

This crate provides the Language Server Protocol (LSP) implementation for the Kettu programming language, enabling rich editor support.

## Key Features

- **Standard LSP Protocol**: Built using `tower-lsp` for robust, asynchronous editor integration.
- **Rich Interactive Features**:
  - **Syntax & Semantic Diagnostics**: Real-time error reporting from `kettu-parser` and `kettu-checker`.
  - **Go-to-Definition**: Navigation between Kettu symbols across module boundaries.
  - **Hover Information**: Context-aware documentation and type signatures (see `HOVER-CAPABILITIES.md`).
  - **Code Completion**: Intelligent suggestions for symbols and language constructs.
- **Cross-Module Analysis**: Leverages the Kettu compiler to provide consistent analysis across multiple files in a project.

## Components

- `lib.rs`: The main LSP handler and state management.
