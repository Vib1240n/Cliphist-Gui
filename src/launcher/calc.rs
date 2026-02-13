use std::io::Write;
use std::process::Command;

pub fn calc_eval(expr: &str) -> Option<String> {
    let e = expr.trim().trim_matches('=').to_lowercase();
    if e.is_empty() {
        return None;
    }

    let allowed = |c: char| c.is_ascii_digit() || "+-*/.^() ".contains(c);
    if !e.chars().all(allowed) {
        return None;
    }

    let mut child = Command::new("bc")
        .arg("-l")
        .env("BC_LINE_LENGTH", "0")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    if let Some(mut stdin) = child.stdin.take() {
        let query = format!("scale=4; {}\n", e);
        let _ = stdin.write_all(query.as_bytes());
    }

    let output = child.wait_with_output().ok()?;
    if output.status.success() {
        let res = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if res.contains('.') {
            let cleaned = res.trim_end_matches('0').trim_end_matches('.').to_string();
            if cleaned.is_empty() || cleaned == "-" {
                return Some("0".to_string());
            }
            return Some(cleaned);
        }
        Some(res)
    } else {
        None
    }
}
