//! Shared frontmatter reader for Skills, agent profiles, and installation.
//! This leaf is also included by the installation acceptance harness.

use std::collections::HashMap;

/// Parsed frontmatter: lowercased metadata keys and the body after the fence.
pub(crate) type Frontmatter<'a> = (HashMap<String, String>, &'a str);

/// Split a Markdown file into its `---` frontmatter metadata and body.
///
/// Returns `Ok(None)` when the file does not open with a `---` fence. Keys are
/// lowercased; values are unquoted, and YAML block scalars (`>`, `|`, with
/// chomping) are folded the way `SKILL.md` has always read them. This is the
/// one frontmatter reader: skills and Claude Code agent files both use it.
pub(crate) fn parse_frontmatter(
    content: &str,
) -> std::result::Result<Option<Frontmatter<'_>>, String> {
    let content = content
        .strip_prefix('\u{feff}')
        .unwrap_or(content)
        .trim_start();
    let opening = content.split_inclusive('\n').next().unwrap_or_default();
    if opening.trim_end() != "---" {
        return Ok(None);
    }
    let rest = &content[opening.len()..];
    let mut offset = 0;
    let end = rest
        .split_inclusive('\n')
        .find_map(|line| {
            let start = offset;
            offset += line.len();
            (line.trim_end() == "---").then_some(start)
        })
        .ok_or_else(|| "missing frontmatter closing delimiter".to_string())?;
    let frontmatter = &rest[..end];
    let body = &rest[end + 3..];

    let mut metadata = HashMap::new();
    let indentation = |line: &str| line.chars().take_while(|ch| ch.is_whitespace()).count();
    let lines: Vec<&str> = frontmatter.lines().collect();
    let mut i = 0;
    let mut maps: Vec<(usize, String)> = Vec::new();
    while i < lines.len() {
        let raw = lines[i];
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            i += 1;
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let indent = indentation(raw);
            while maps
                .last()
                .is_some_and(|(parent_indent, _)| *parent_indent >= indent)
            {
                maps.pop();
            }
            let key = key.trim().to_ascii_lowercase();
            let key = maps
                .last()
                .map_or_else(|| key.clone(), |(_, parent)| format!("{parent}.{key}"));
            let value = value.trim();
            // Check for YAML block scalar indicators: > (folded), | (literal),
            // optionally with chomping: >-, >+, |-, |+
            let is_block_scalar = matches!(value, ">" | "|" | ">-" | ">+" | "|-" | "|+");
            if is_block_scalar {
                let is_folded = value.starts_with('>');
                let chomp = if value.ends_with('-') {
                    "strip"
                } else if value.ends_with('+') {
                    "keep"
                } else {
                    "clip"
                };
                // Determine the base indentation from the key line
                let base_indent = indentation(raw);
                let mut block_lines: Vec<&str> = Vec::new();
                let mut content_indent: Option<usize> = None;
                i += 1;
                while i < lines.len() {
                    let raw_line = lines[i];
                    if raw_line.trim().is_empty() {
                        // Empty lines are part of the block
                        block_lines.push("");
                        i += 1;
                        continue;
                    }
                    let line_indent = indentation(raw_line);
                    if line_indent > base_indent {
                        // Track content indent from the first non-empty
                        // line so we strip only that one level of
                        // leading whitespace, preserving any deeper
                        // relative indentation (YAML §8.1.2).
                        if content_indent.is_none() {
                            content_indent = Some(line_indent);
                        }
                        block_lines.push(raw_line);
                        i += 1;
                    } else {
                        break;
                    }
                }
                let content_indent = content_indent.unwrap_or(base_indent);
                // Strip only the content indent from each non-empty
                // line so nested indentation survives.
                let block_lines: Vec<&str> = block_lines
                    .iter()
                    .map(|raw| {
                        if raw.is_empty() {
                            ""
                        } else {
                            let indent = indentation(raw);
                            let strip = std::cmp::min(indent, content_indent);
                            let byte = raw.char_indices().nth(strip).map_or(raw.len(), |(i, _)| i);
                            &raw[byte..]
                        }
                    })
                    .collect();
                // Apply chomping to trailing empty lines before folding.
                // Chomping operates on the raw block_lines (before join), so
                // strip / keep / clip behave per the YAML spec.
                let block_lines = if matches!(chomp, "strip") {
                    // strip: remove all trailing empty lines
                    let mut lines = block_lines;
                    while lines.last().is_some_and(|s| s.is_empty()) {
                        lines.pop();
                    }
                    lines
                } else if matches!(chomp, "keep") {
                    // keep: no modification
                    block_lines
                } else {
                    // clip: keep at most one trailing empty line
                    let mut lines = block_lines;
                    while lines.len() >= 2
                        && lines[lines.len() - 1].is_empty()
                        && lines[lines.len() - 2].is_empty()
                    {
                        lines.pop();
                    }
                    lines
                };
                let description = if is_folded {
                    // Folded: join non-empty lines with spaces; empty
                    // lines become paragraph breaks.
                    let mut result = String::new();
                    let mut pending_space = false;
                    for line in &block_lines {
                        if line.is_empty() {
                            result.push('\n');
                            pending_space = false;
                        } else {
                            if pending_space {
                                result.push(' ');
                            }
                            result.push_str(line);
                            pending_space = true;
                        }
                    }
                    result
                } else {
                    // Literal: join with newlines.
                    block_lines.join("\n")
                };
                metadata.insert(key, description);
            } else if value.is_empty()
                && lines
                    .get(i + 1)
                    .is_some_and(|next| is_block_sequence_item(next))
            {
                // A block sequence (`tools:` then `  - Read` lines) becomes
                // one comma-separated value, the same as the flow form
                // `tools: Read, Grep`. Dropping it would read as "no list".
                let mut items = Vec::new();
                i += 1;
                while let Some(next) = lines.get(i).filter(|next| is_block_sequence_item(next)) {
                    let item = next.trim()[1..].trim();
                    let item = item
                        .strip_prefix('"')
                        .and_then(|v| v.strip_suffix('"'))
                        .or_else(|| item.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                        .unwrap_or(item);
                    if !item.is_empty() {
                        items.push(item);
                    }
                    i += 1;
                }
                metadata.insert(key, items.join(", "));
            } else if value.is_empty() {
                // Child fields retain their map path. In particular,
                // metadata.name must never replace the skill's own name.
                metadata.insert(key.clone(), String::new());
                maps.push((indent, key));
                i += 1;
            } else if value.starts_with('[') {
                // Reuse the installed YAML reader for quoted flow items rather
                // than splitting commas inside quoted tool names or aliases.
                let documents = yaml_rust2::YamlLoader::load_from_str(value)
                    .map_err(|err| format!("invalid frontmatter sequence `{key}`: {err}"))?;
                let items = documents
                    .first()
                    .and_then(yaml_rust2::Yaml::as_vec)
                    .ok_or_else(|| format!("frontmatter `{key}` must be a flow sequence"))?;
                let values: Result<Vec<_>, _> = items
                    .iter()
                    .map(|item| {
                        item.as_str().ok_or_else(|| {
                            format!("frontmatter `{key}` sequence items must be strings")
                        })
                    })
                    .collect();
                metadata.insert(key, values?.join(", "));
                i += 1;
            } else {
                let unquoted = match value {
                    v if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
                        || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2) =>
                    {
                        &v[1..v.len() - 1]
                    }
                    _ => value,
                };
                i += 1;
                let mut text = unquoted.to_string();
                // Wrapped plain scalars continue at a deeper indentation.
                // A colon in that continuation belongs to the value, not a
                // new metadata key. Quoted/flow values retain their grammar.
                if !value.is_empty() && !value.starts_with(['"', '\'', '[', '{']) {
                    while let Some(next) = lines.get(i) {
                        if next.trim().is_empty() || indentation(next) <= indentation(raw) {
                            break;
                        }
                        if !next.trim_start().starts_with('#') {
                            text.push(' ');
                            text.push_str(next.trim());
                        }
                        i += 1;
                    }
                }
                metadata.insert(key, text);
            }
        } else {
            i += 1;
        }
    }

    Ok(Some((metadata, body)))
}

/// A YAML block-sequence entry: `- item` (or a bare `-`) on its own line.
fn is_block_sequence_item(line: &str) -> bool {
    let line = line.trim();
    line == "-" || line.starts_with("- ")
}
