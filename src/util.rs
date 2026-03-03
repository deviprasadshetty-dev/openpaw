use chrono::Utc;

/// Format bytes as human-readable string (e.g. "3.4 MB")
pub fn format_bytes(bytes: u64) -> (f64, &'static str) {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut idx = 0;
    while size >= 1024.0 && idx < units.len() - 1 {
        size /= 1024.0;
        idx += 1;
    }
    (size, units[idx])
}

/// Get current timestamp as ISO 8601 string
pub fn timestamp() -> String {
    let now = Utc::now();
    now.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Append a string to a buffer with JSON escaping (quotes, backslashes, control chars).
/// Used by embedding providers, vector stores, and API backends when building JSON payloads.
pub fn append_json_escaped(buf: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '"' => buf.push_str("\\\""),
            '\\' => buf.push_str("\\\\"),
            '\n' => buf.push_str("\\n"),
            '\r' => buf.push_str("\\r"),
            '\t' => buf.push_str("\\t"),
            _ => {
                if (ch as u32) < 0x20 {
                    let hex = format!("\\u{:04x}", ch as u32);
                    buf.push_str(&hex);
                } else {
                    buf.push(ch);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        let (val, unit) = format_bytes(3_500_000);
        assert!(val > 3.3 && val < 3.4);
        assert_eq!(unit, "MB");
    }

    #[test]
    fn test_format_bytes_zero() {
        let (val, unit) = format_bytes(0);
        assert_eq!(val, 0.0);
        assert_eq!(unit, "B");
    }

    #[test]
    fn test_format_bytes_exact_kb() {
        let (val, unit) = format_bytes(1024);
        assert_eq!(val, 1.0);
        assert_eq!(unit, "KB");
    }

    #[test]
    fn test_format_bytes_exact_mb() {
        let (val, unit) = format_bytes(1024 * 1024);
        assert_eq!(val, 1.0);
        assert_eq!(unit, "MB");
    }

    #[test]
    fn test_format_bytes_exact_gb() {
        let (val, unit) = format_bytes(1024 * 1024 * 1024);
        assert_eq!(val, 1.0);
        assert_eq!(unit, "GB");
    }

    #[test]
    fn test_format_bytes_exact_tb() {
        let (val, unit) = format_bytes(1024 * 1024 * 1024 * 1024);
        assert_eq!(val, 1.0);
        assert_eq!(unit, "TB");
    }

    #[test]
    fn test_format_bytes_small() {
        let (val, unit) = format_bytes(500);
        assert_eq!(val, 500.0);
        assert_eq!(unit, "B");
    }

    #[test]
    fn test_format_bytes_1_byte() {
        let (val, unit) = format_bytes(1);
        assert_eq!(val, 1.0);
        assert_eq!(unit, "B");
    }

    #[test]
    fn test_format_bytes_large() {
        let (val, unit) = format_bytes(5 * 1024 * 1024 * 1024 * 1024);
        assert!(val > 4.9 && val < 5.1);
        assert_eq!(unit, "TB");
    }

    #[test]
    fn test_timestamp_ends_with_z() {
        let ts = timestamp();
        assert!(ts.ends_with('Z'));
    }

    #[test]
    fn test_timestamp_contains_t() {
        let ts = timestamp();
        assert!(ts.contains('T'));
    }

    #[test]
    fn test_append_json_escaped_basic() {
        let mut buf = String::new();
        append_json_escaped(&mut buf, "hello world");
        assert_eq!(buf, "hello world");
    }

    #[test]
    fn test_append_json_escaped_special() {
        let mut buf = String::new();
        append_json_escaped(&mut buf, "say \"hello\"\nnewline\\backslash");
        assert_eq!(buf, "say \\\"hello\\\"\\nnewline\\\\backslash");
    }

    #[test]
    fn test_append_json_escaped_control() {
        let mut buf = String::new();
        append_json_escaped(&mut buf, "tab\there\rreturn");
        assert_eq!(buf, "tab\\there\\rreturn");
    }

    #[test]
    fn test_append_json_escaped_empty() {
        let mut buf = String::new();
        append_json_escaped(&mut buf, "");
        assert_eq!(buf, "");
    }
}
