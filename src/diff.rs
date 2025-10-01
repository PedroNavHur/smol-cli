use similar::TextDiff;

pub fn unified_diff(old: &str, new: &str, path: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let a_path = format!("a/{}", path);
    let b_path = format!("b/{}", path);
    diff.unified_diff().header(&a_path, &b_path).to_string()
}
