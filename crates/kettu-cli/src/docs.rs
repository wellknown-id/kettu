//! Embedded documentation viewer for `kettu docs`.
//!
//! All documentation markdown files from `docs/` are compiled into the binary
//! at build time.  A `build.rs` script walks the `docs/` tree, discovers `.md`
//! files with frontmatter, and generates `docs_generated.rs` containing
//! `include_str!` entries so that new doc files are picked up automatically.
//!
//! Frontmatter uses kettu-style `//` comments:
//! ```text
//! ---
//! // section: "Language Topics"
//! // order: 1
//! // title: "Packages & Interfaces"
//! ---
//! ```

// Pull in the auto-generated include_str! array.
mod docs_generated {
    include!("docs_generated.rs");
}

/// A single documentation page parsed from an embedded markdown file.
struct DocPage {
    section: String,
    order: u32,
    title: String,
    /// The markdown body *without* frontmatter.
    content: String,
    /// Original filename stem, used for link rewriting (e.g. "simd", "types").
    filename: String,
}

/// Parse a kettu-style frontmatter value from a `// key: "value"` or `// key: value` line.
fn parse_meta(line: &str, key: &str) -> Option<String> {
    let trimmed = line.trim().trim_start_matches("//").trim();
    if let Some(rest) = trimmed.strip_prefix(key) {
        let rest = rest.trim_start_matches(':').trim();
        let value = rest.trim_matches('"');
        Some(value.to_string())
    } else {
        None
    }
}

/// Strip frontmatter (everything between the first pair of `---` lines) and
/// return the remaining markdown body.
fn strip_frontmatter(source: &str) -> &str {
    if !source.starts_with("---") {
        return source;
    }
    if let Some(end) = source[3..].find("\n---") {
        let after = end + 3 + 4; // skip "\n---"
        if after < source.len() {
            return source[after..].trim_start_matches('\n');
        }
    }
    source
}

/// Extract the filename stem from embedded source content.
/// We look at the first `# Heading` line to derive a logical name, but that's
/// fragile.  Instead, we embed a `// file:` hint in the frontmatter from build.rs.
/// Fallback: derive from the title.
fn extract_filename(front: &str, title: &str) -> String {
    for line in front.lines() {
        if let Some(v) = parse_meta(line, "file") {
            return v;
        }
    }
    // Fallback: slugify the title
    title
        .to_lowercase()
        .replace(|c: char| !c.is_alphanumeric(), "-")
        .trim_matches('-')
        .to_string()
}

/// Parse a single source string into a `DocPage`, extracting frontmatter metadata.
fn parse_page(source: &str) -> Option<DocPage> {
    if !source.starts_with("---") {
        return None;
    }
    let end = source[3..].find("\n---")?;
    let front = &source[3..3 + end];

    let mut section = None;
    let mut order = None;
    let mut title = None;
    let mut is_index = false;

    for line in front.lines() {
        if let Some(v) = parse_meta(line, "section") {
            section = Some(v);
        }
        if let Some(v) = parse_meta(line, "order") {
            order = v.parse::<u32>().ok();
        }
        if let Some(v) = parse_meta(line, "title") {
            title = Some(v);
        }
        if let Some(v) = parse_meta(line, "index") {
            if v == "true" {
                is_index = true;
            }
        }
    }

    // Skip index pages — they're not navigable topics.
    if is_index {
        return None;
    }

    let title_str = title?;
    let filename = extract_filename(front, &title_str);

    Some(DocPage {
        section: section?,
        order: order?,
        title: title_str,
        content: strip_frontmatter(source).to_string(),
        filename,
    })
}

/// Load and parse all embedded doc pages, sorted by section order then page order.
fn load_docs() -> Vec<DocPage> {
    let mut pages: Vec<DocPage> = docs_generated::SOURCES
        .iter()
        .filter_map(|s| parse_page(s))
        .collect();

    pages.sort_by(|a, b| {
        let sec_ord = |s: &str| -> u32 {
            match s {
                "Language Topics" => 1,
                "Advanced Topics" => 2,
                _ => 99,
            }
        };
        sec_ord(&a.section)
            .cmp(&sec_ord(&b.section))
            .then(a.order.cmp(&b.order))
    });
    pages
}

/// Group pages by section, preserving order.  Returns `(section_name, pages)` pairs.
fn grouped(pages: &[DocPage]) -> Vec<(&str, Vec<&DocPage>)> {
    let mut groups: Vec<(&str, Vec<&DocPage>)> = Vec::new();
    for page in pages {
        if let Some(last) = groups.last_mut() {
            if last.0 == page.section.as_str() {
                last.1.push(page);
                continue;
            }
        }
        groups.push((page.section.as_str(), vec![page]));
    }
    groups
}

/// Build a lookup table from filename stem → "X.Y" selector string.
fn build_link_map(pages: &[DocPage]) -> std::collections::HashMap<String, String> {
    let groups = grouped(pages);
    let mut map = std::collections::HashMap::new();
    for (sec_idx, (_section, topics)) in groups.iter().enumerate() {
        let sec_num = sec_idx + 1;
        for (topic_idx, topic) in topics.iter().enumerate() {
            let sub_num = topic_idx + 1;
            let selector = format!("{}.{}", sec_num, sub_num);
            map.insert(topic.filename.clone(), selector);
        }
    }
    map
}

/// Rewrite markdown links like `[SIMD](../simd.md)` to `SIMD (→ kettu docs 2.1)`.
fn rewrite_links(content: &str, link_map: &std::collections::HashMap<String, String>) -> String {
    let mut result = String::with_capacity(content.len());
    let mut remaining = content;

    while let Some(bracket_start) = remaining.find('[') {
        // Push everything before the `[`
        result.push_str(&remaining[..bracket_start]);

        let after_bracket = &remaining[bracket_start + 1..];

        // Find closing `]`
        if let Some(bracket_end) = after_bracket.find(']') {
            let link_text = &after_bracket[..bracket_end];
            let after_close = &after_bracket[bracket_end + 1..];

            // Check for `(` immediately after `]`
            if after_close.starts_with('(') {
                if let Some(paren_end) = after_close.find(')') {
                    let url = &after_close[1..paren_end];

                    // Check if this is a local .md link
                    if url.ends_with(".md") || url.contains(".md#") {
                        // Extract the filename stem from the URL
                        let md_part = url.split('#').next().unwrap_or(url);
                        let stem = md_part
                            .rsplit('/')
                            .next()
                            .unwrap_or(md_part)
                            .trim_end_matches(".md");

                        if let Some(selector) = link_map.get(stem) {
                            result.push_str(&format!(
                                "{} (\u{2192} kettu docs {})",
                                link_text, selector
                            ));
                            remaining = &after_close[paren_end + 1..];
                            continue;
                        }
                    }

                    // Not a rewritable link — keep as-is
                    result.push('[');
                    result.push_str(link_text);
                    result.push(']');
                    result.push('(');
                    result.push_str(url);
                    result.push(')');
                    remaining = &after_close[paren_end + 1..];
                    continue;
                }
            }

            // Not a link — just a bracket. Push `[` and continue after it.
            result.push('[');
            remaining = after_bracket;
        } else {
            // No closing bracket — push `[` and move on
            result.push('[');
            remaining = after_bracket;
        }
    }

    result.push_str(remaining);
    result
}

/// Print the topic index.
pub fn print_index() {
    let pages = load_docs();
    let groups = grouped(&pages);

    println!("\x1b[1mKettu Language Guide\x1b[0m");
    println!();

    for (sec_idx, (section, topics)) in groups.iter().enumerate() {
        let sec_num = sec_idx + 1;
        println!("  \x1b[1;36m{}\x1b[0m  \x1b[1m{}\x1b[0m", sec_num, section);
        for (topic_idx, topic) in topics.iter().enumerate() {
            let sub_num = topic_idx + 1;
            println!(
                "     \x1b[36m{}.{}\x1b[0m  {}",
                sec_num, sub_num, topic.title
            );
        }
        println!();
    }

    println!(
        "Run: \x1b[1mkettu docs <number>\x1b[0m  (e.g. \x1b[36mkettu docs 1.2\x1b[0m)"
    );
}

/// Print a specific topic or section overview.
pub fn print_topic(selector: &str) {
    let pages = load_docs();
    let link_map = build_link_map(&pages);
    let groups = grouped(&pages);

    let parts: Vec<&str> = selector.split('.').collect();
    let sec_num: usize = match parts[0].parse::<usize>() {
        Ok(n) if n >= 1 => n,
        _ => {
            eprintln!("Invalid topic number: {}", selector);
            eprintln!("Run `kettu docs` to see available topics.");
            std::process::exit(1);
        }
    };

    if sec_num > groups.len() {
        eprintln!(
            "Section {} does not exist. There are {} section(s).",
            sec_num,
            groups.len()
        );
        eprintln!("Run `kettu docs` to see available topics.");
        std::process::exit(1);
    }

    let (section_name, topics) = &groups[sec_num - 1];

    if parts.len() == 1 {
        println!("\x1b[1m{}\x1b[0m", section_name);
        println!();
        for (i, topic) in topics.iter().enumerate() {
            println!("  {}.{}  {}", sec_num, i + 1, topic.title);
        }
        println!();
        println!(
            "Run: \x1b[1mkettu docs {}.N\x1b[0m to read a topic.",
            sec_num
        );
        return;
    }

    let topic_num: usize = match parts[1].parse::<usize>() {
        Ok(n) if n >= 1 => n,
        _ => {
            eprintln!("Invalid topic number: {}", selector);
            eprintln!("Run `kettu docs` to see available topics.");
            std::process::exit(1);
        }
    };

    if topic_num > topics.len() {
        eprintln!(
            "Topic {}.{} does not exist. Section \"{}\" has {} topic(s).",
            sec_num,
            topic_num,
            section_name,
            topics.len()
        );
        eprintln!("Run `kettu docs {}` to see topics in this section.", sec_num);
        std::process::exit(1);
    }

    let topic = topics[topic_num - 1];
    let rewritten = rewrite_links(&topic.content, &link_map);
    println!("{}", rewritten);
}
