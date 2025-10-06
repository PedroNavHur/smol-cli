use crate::diff;
use regex::Regex;

pub fn format_answer(answer: &str) -> String {
    let trimmed = answer.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let code_blocks = extract_code_blocks(trimmed);
    if code_blocks.is_empty() {
        return trimmed.to_string();
    }

    let mut output = String::new();

    let plain_segments = collect_plain_segments(trimmed);
    if !plain_segments.is_empty() {
        output.push_str(&plain_segments.join("\n\n"));
        output.push_str("\n\n");
    }

    for (idx, block) in code_blocks.iter().enumerate() {
        let ext = language_to_ext(block.language.as_deref());
        let path = format!("answer.{}", ext);
        let mut code = block.code.trim_matches('\n').to_string();
        if !code.ends_with('\n') {
            code.push('\n');
        }
        let diff = diff::unified_diff("", &code, &path);
        output.push_str(&diff);
        if idx + 1 < code_blocks.len() {
            output.push_str("\n");
        }
    }

    output.trim_end().to_string()
}

struct CodeBlock {
    language: Option<String>,
    code: String,
}

fn extract_code_blocks(answer: &str) -> Vec<CodeBlock> {
    let re = Regex::new(r"(?s)```([a-zA-Z0-9_+\-.]*)\n(.*?)```\s*").unwrap();
    re.captures_iter(answer)
        .map(|caps| CodeBlock {
            language: caps
                .get(1)
                .map(|m| m.as_str().trim().to_string())
                .filter(|s| !s.is_empty()),
            code: caps
                .get(2)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
        })
        .collect()
}

fn collect_plain_segments(answer: &str) -> Vec<String> {
    let re = Regex::new(r"(?s)```[a-zA-Z0-9_+\-.]*\n.*?```\s*").unwrap();
    let mut segments = Vec::new();
    let mut last_end = 0;
    for mat in re.find_iter(answer) {
        let before = &answer[last_end..mat.start()];
        if !before.trim().is_empty() {
            segments.push(before.trim().to_string());
        }
        last_end = mat.end();
    }
    let after = &answer[last_end..];
    if !after.trim().is_empty() {
        segments.push(after.trim().to_string());
    }
    segments
}

fn language_to_ext(lang: Option<&str>) -> &'static str {
    match lang.map(|s| s.to_ascii_lowercase()) {
        Some(ref l) if l == "rs" || l == "rust" => "rs",
        Some(ref l) if l == "ts" || l == "typescript" => "ts",
        Some(ref l) if l == "js" || l == "javascript" => "js",
        Some(ref l) if l == "jsx" => "jsx",
        Some(ref l) if l == "tsx" => "tsx",
        Some(ref l) if l == "py" || l == "python" => "py",
        Some(ref l) if l == "html" => "html",
        Some(ref l) if l == "css" => "css",
        Some(ref l) if l == "json" => "json",
        Some(ref l) if l == "toml" => "toml",
        Some(ref l) if l == "yaml" || l == "yml" => "yaml",
        Some(ref l) if l == "sh" || l == "bash" => "sh",
        Some(ref l) if l == "sql" => "sql",
        Some(ref l) if l == "java" => "java",
        Some(ref l) if l == "c" => "c",
        Some(ref l) if l == "cpp" || l == "c++" => "cpp",
        _ => "txt",
    }
}
