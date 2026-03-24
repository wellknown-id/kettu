# kettu-checker

Semantic checker for the Kettu language.

The `kettu-checker` crate provides the static analysis and type validation layer of the Kettu compiler.

## Key Features

- **Type Checking**: Comprehensive type inference and verification for the Kettu AST.
- **Semantic Validation**: Ensures that Kettu code adheres to language-level rules, such as scope isolation and proper function usage.
- **Name Resolution**: Handles symbol lookup and scoping across modules.
- **Detailed Diagnostics**: Produces context-aware error messages and warnings for common developer mistakes.
- **Compiler Pass**: Designed as a unified pass that accepts a Kettu AST and returns a semantically verified tree.

## Structure

- `lib.rs`: The main checker implementation, containing the visitor logic and semantic rules.
