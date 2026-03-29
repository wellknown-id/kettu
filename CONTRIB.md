# Contributing to Kettu Documentation

Kettu's documentation is uniquely integrated: it's written in Markdown, stored in the `docs/` directory, and **embedded directly into the `kettu` executable** at build time. This ensures that the language guide is always available offline via the CLI.

## Adding New Documentation

To add a new topic to the documentation:

1.  Create a new `.md` file in `./docs/` or one of its subdirectories (e.g., `./docs/language-guide/`).
2.  Add the required **frontmatter** at the very top of the file.

### Frontmatter Format

The frontmatter is delimited by `---` and uses `//` comment syntax for metadata:

```markdown
---
// section: "Language Topics"
// order: 5
// title: "Pattern Matching"
// file: "match"
// keywords: "match, switch, patterns, destructuring"
---

# Pattern Matching in Kettu

Content goes here...
```

*   **`section`**: The high-level category (e.g., "Getting Started", "Language Topics", "Advanced Topics").
*   **`order`**: An integer determining the sorting order within that section.
*   **`title`**: The display name in the `kettu docs` index.
*   **`file`**: (Optional) A unique slug used for stable link rewriting.
*   **`keywords`**: (Optional) Comma-separated terms to improve searchability in `kettu docs search`.

### Linking Between Docs

You can use standard Markdown relative links: `[Types](./types.md)`.
The build system automatically rewrites these links into CLI-friendly selectors, e.g., `Types (→ kettu docs 2.1)`.

## Doc-Testing

Code snippets in the documentation are **automatically verified** to ensure they remain correct as the language evolves.

### Testing Snippets

Any fenced code block marked as ` ```kettu ` will be tested:

*   **` ```kettu `** (or ` ```kettu check `): Parses and type-checks the snippet.
*   **` ```kettu parse `**: Only verifies that the snippet is syntactically valid.
*   **` ```kettu nocheck `**: Skips verification entirely.

### Snippet Wrapping

Unless a snippet contains a `package`, `interface`, or `world` declaration, it is automatically wrapped in a template:

```kettu
// Your snippet:
let x = 42;
```

Is internally wrapped into:

```kettu
package local:doctest;
interface snippet {
    run: func() {
        let x = 42;
    }
}
```

### Using Preambles

If a snippet depends on declarations (like a resource or a function) that shouldn't be part of the visible example, you can define a **preamble** in the frontmatter:

```markdown
---
// title: "Advanced Resources"
// ...
// preamble-start
// resource database {
//     query: func(q: string) -> string;
// }
// preamble-end
---

```kettu
let db = database();
db.query("SELECT *");
```
```

The content between `// preamble-start` and `// preamble-end` will be prepended to every snippet in that file during testing.

## Verifying Your Changes

After adding or modifying documentation, you can verify it using the `kettu` CLI:

1.  **Rebuild the CLI**: `cargo build -p kettu-cli` (required to embed the new content).
2.  **View the Index**: `cargo run -- docs`
3.  **Read your Topic**: `cargo run -- docs <section>.<topic>` (e.g., `cargo run -- docs 2.5`)
4.  **Run Doc-Tests**: `cargo run -- docs --check`

---

Thank you for helping improve Kettu's documentation!
