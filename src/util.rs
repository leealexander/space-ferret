use std::time::{SystemTime, UNIX_EPOCH};

/// 1024-based human size: "812 B", "4.20 MB", "31.7 GB", "118 GB".
pub fn human(b: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if b < 1024 {
        return format!("{b} B");
    }
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else if v >= 10.0 {
        format!("{v:.1} {}", UNITS[i])
    } else {
        format!("{v:.2} {}", UNITS[i])
    }
}

/// "500K", "20M", "1.5G", "2TB", "100MiB", "4096" -> bytes (1024-based).
pub fn parse_size(s: &str) -> Option<u64> {
    let lower = s.trim().to_ascii_lowercase();
    let t = lower.trim_end_matches("ib").trim_end_matches('b');
    let (num, mult) = match t.chars().last()? {
        'k' => (&t[..t.len() - 1], 1u64 << 10),
        'm' => (&t[..t.len() - 1], 1 << 20),
        'g' => (&t[..t.len() - 1], 1 << 30),
        't' => (&t[..t.len() - 1], 1 << 40),
        _ => (t, 1),
    };
    let v: f64 = num.trim().parse().ok()?;
    (v >= 0.0).then(|| (v * mult as f64) as u64)
}

pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Whole days since `t` (unix seconds); None when the timestamp is unknown.
pub fn age_days(now: i64, t: i64) -> Option<i64> {
    (t > 0).then(|| (now - t).max(0) / 86_400)
}

pub fn age_str(days: Option<i64>) -> String {
    match days {
        None => "-".into(),
        Some(d) if d < 1 => "today".into(),
        Some(d) if d < 60 => format!("{d}d"),
        Some(d) if d < 730 => format!("{}mo", d / 30),
        Some(d) => format!("{}y", d / 365),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("10M"), Some(10 << 20));
        assert_eq!(parse_size("10mb"), Some(10 << 20));
        assert_eq!(parse_size("10MiB"), Some(10 << 20));
        assert_eq!(parse_size("1.5g"), Some(3 << 29));
        assert_eq!(parse_size("4096"), Some(4096));
        assert_eq!(parse_size("x"), None);
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1536), "1.50 KB");
        assert_eq!(thousands(1234567), "1,234,567");
    }
}
