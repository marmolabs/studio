//! How long ago something happened, and exactly when.

/// "just now", "3m ago", "2h ago", "4d ago".
pub fn relative(now_ms: u64, at_ms: u64) -> String {
    let secs = now_ms.saturating_sub(at_ms) / 1000;
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// "2026-10-30 14:05:09 UTC".
pub fn exact(at_ms: u64) -> String {
    let secs = at_ms / 1000;
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Days since 1970-01-01 to a calendar date (Hinnant's civil_from_days).
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_naturally() {
        assert_eq!(relative(1_000_000, 1_000_000 - 30_000), "just now");
        assert_eq!(relative(1_000_000, 1_000_000 - 180_000), "3m ago");
        assert_eq!(relative(10_000_000, 10_000_000 - 7_300_000), "2h ago");
        assert_eq!(exact(0), "1970-01-01 00:00:00 UTC");
        // One second after a leap day.
        assert_eq!(exact(1_709_251_201_000), "2024-03-01 00:00:01 UTC");
    }
}
