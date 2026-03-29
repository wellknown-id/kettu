fn main() {
    println!("cargo:rerun-if-changed=src");
    rust_sitter_tool::build_parser("src/grammar/mod.rs");

    // Generate TextMate grammars from the same grammar source
    use rust_sitter_tool::TextMateBuilder;

    // Kettu textmate grammar
    if let Some(json) = TextMateBuilder::default()
        .scope_name("kettu")
        .build("src/grammar/mod.rs")
    {
        let out =
            std::path::Path::new("../kettu-cli/editors/vscode/syntaxes/kettu.tmLanguage.json");
        std::fs::write(out, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    }

    // WIT textmate grammar (same grammar, different scope)
    if let Some(json) = TextMateBuilder::default()
        .scope_name("wit")
        .build("src/grammar/mod.rs")
    {
        let out = std::path::Path::new("../kettu-cli/editors/vscode/syntaxes/wit.tmLanguage.json");
        std::fs::write(out, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    }

    // Generate preview highlighter files (JS + CSS) for markdown preview
    let preview_dir = std::path::Path::new("../kettu-cli/editors/vscode/preview");
    std::fs::create_dir_all(preview_dir).unwrap();

    if let Some(preview) = TextMateBuilder::default()
        .scope_name("kettu")
        .build_preview("src/grammar/mod.rs", "kettu")
    {
        std::fs::write(preview_dir.join("kettu-preview.js"), &preview.js).unwrap();
        std::fs::write(preview_dir.join("kettu-preview.css"), &preview.css).unwrap();
    }

    if let Some(preview) = TextMateBuilder::default()
        .scope_name("wit")
        .build_preview("src/grammar/mod.rs", "wit")
    {
        std::fs::write(preview_dir.join("wit-preview.js"), &preview.js).unwrap();
        std::fs::write(preview_dir.join("wit-preview.css"), &preview.css).unwrap();
    }
}
