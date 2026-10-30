//! Keeping one tool result small enough to hand to the program: the longest strings in it (a
//! command's stdout, a file's content) lose their middle until the whole fits. The log keeps
//! the result as the tool returned it.

use crate::output::{ceil_char, floor_char};
use serde_json::{Value, json};

/// Shrinks `v` until its JSON is at most `max` bytes. Returns how many bytes it was before, when
/// it had to shrink.
pub(crate) fn fit(v: &mut Value, max: usize) -> Option<usize> {
    let before = json_len(v);
    if before <= max {
        return None;
    }
    // Each pass cuts the longest string by what's still over, plus room for its marker. A
    // result made of many small strings (a huge array) can't be fitted this way, and is
    // replaced by a note after a few passes.
    for _ in 0..32 {
        let over = json_len(v).saturating_sub(max);
        if over == 0 {
            return Some(before);
        }
        let Some(s) = longest_string(v) else { break };
        if s.len() < 256 {
            break;
        }
        let keep = s.len().saturating_sub(over + 80);
        *s = cut_middle(s, keep);
    }
    if json_len(v) > max {
        *v = json!({ "error": format!("result too large to keep: {before} bytes") });
    }
    Some(before)
}

fn json_len(v: &Value) -> usize {
    serde_json::to_vec(v).map(|b| b.len()).unwrap_or(usize::MAX)
}

fn longest_string(v: &mut Value) -> Option<&mut String> {
    match v {
        Value::String(s) => Some(s),
        Value::Array(a) => a
            .iter_mut()
            .filter_map(longest_string)
            .max_by_key(|s| s.len()),
        Value::Object(o) => o
            .values_mut()
            .filter_map(longest_string)
            .max_by_key(|s| s.len()),
        _ => None,
    }
}

/// `s` with its middle replaced by a marker, keeping about `keep` bytes split between head and tail.
fn cut_middle(s: &str, keep: usize) -> String {
    let head = floor_char(s, keep / 2);
    let tail = ceil_char(s, s.len() - (keep - keep / 2));
    let dropped = tail - head;
    format!(
        "{}\n[... {dropped} bytes dropped ...]\n{}",
        &s[..head],
        &s[tail..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_results_are_left_alone() {
        let mut v = json!({ "stdout": "ok" });
        assert_eq!(fit(&mut v, 100), None);
        assert_eq!(v["stdout"], "ok");
    }

    #[test]
    fn the_longest_string_loses_its_middle() {
        let big = format!("start{}end", "x".repeat(10_000));
        let mut v = json!({ "stdout": big, "stderr": "warn", "exit_code": 0 });
        assert_eq!(fit(&mut v, 1_000), Some(10_051));
        let out = v["stdout"].as_str().unwrap();
        assert!(out.starts_with("start") && out.ends_with("end"), "{out}");
        assert!(out.contains("bytes dropped"));
        assert_eq!(v["stderr"], "warn");
        assert!(serde_json::to_vec(&v).unwrap().len() <= 1_000);
    }

    #[test]
    fn many_small_strings_become_a_note() {
        let mut v = Value::Array((0..1_000).map(|i| json!(format!("item {i}"))).collect());
        assert!(fit(&mut v, 1_000).is_some());
        assert!(v["error"].as_str().unwrap().starts_with("result too large"));
    }
}
