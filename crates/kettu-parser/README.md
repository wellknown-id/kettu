# kettu-parser

Parser for the Kettu language.

This crate provides the grammar-driven parser and abstract syntax tree (AST) for the Kettu programming language.

## Key Features

- **AST Generation**: Transforms Kettu source code into a structured abstract syntax tree (`ast.rs`) suitable for semantic analysis and code generation.
- **Rust-Sitter Integration**: Utilizes `rust-sitter` for a powerful, grammar-first parsing approach that integrates seamlessly with Rust's type system.
- **Error Diagnostics**: Built-in support for high-quality, formatted error reporting via `ariadne`.
- **Lexer & Capture**: A robust lexing implementation and capture system for tracking source locations throughout the compilation pipeline.

## Components

- `ast.rs`: The definitive structure of the Kettu language.
- `lexer.rs`: Tokenization logic for Kettu source.
- `grammar/`: Rust-Sitter grammar definitions.
- `lib.rs`: The main parser entry point and API.
