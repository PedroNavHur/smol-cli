use similar::TextDiff;

pub fn unified_diff(old: &str, new: &str, path: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::new();
    out.push_str(&format!("--- a/{path}\n+++ b/{path}\n"));
    for group in diff.grouped_ops(3) {
        let (mut old_ln, mut new_ln) = (0, 0);
        for op in group {
            for change in diff.iter_changes(&op) {
                match change.tag() {
                    similar::ChangeTag::Delete => {
                        old_ln += 1;
                        out.push_str(&format!("-{}", change));
                    }
                    similar::ChangeTag::Insert => {
                        new_ln += 1;
                        out.push_str(&format!("+{}", change));
                    }
                    similar::ChangeTag::Equal => {
                        old_ln += 1;
                        new_ln += 1;
                        out.push_str(&format!(" {}", change));
                    }
                }
            }
        }
    }
    out
}
