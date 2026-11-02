//! The owner's clock. The studio keeps its times as milliseconds since the
//! epoch; the team pages say them as the machine's own clock does: "9:15" for
//! something today, the date for anything before today, and the greeting that
//! suits the hour.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long the machine's offset from UTC is believed before it is asked
/// again, so a change of daylight saving time is caught.
const OFFSET_FRESH: Duration = Duration::from_secs(600);

/// The machine's offset from UTC in seconds, from its own clock settings;
/// none (UTC) when they can't be read.
pub fn offset() -> i64 {
    static KNOWN: Mutex<Option<(Instant, i64)>> = Mutex::new(None);
    let Ok(mut known) = KNOWN.lock() else {
        return 0;
    };
    if let Some((at, offset)) = *known
        && at.elapsed() < OFFSET_FRESH
    {
        return offset;
    }
    let offset = std::process::Command::new("date")
        .arg("+%z")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|text| parse_offset(text.trim()))
        .unwrap_or(0);
    *known = Some((Instant::now(), offset));
    offset
}

/// Seconds from "+0530" or "-0800".
fn parse_offset(text: &str) -> Option<i64> {
    let (sign, digits) = match text.split_at_checked(1)? {
        ("+", digits) => (1, digits),
        ("-", digits) => (-1, digits),
        _ => return None,
    };
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: i64 = digits[..2].parse().ok()?;
    let minutes: i64 = digits[2..].parse().ok()?;
    Some(sign * (hours * 3600 + minutes * 60))
}

const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// A moment in the owner's day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Local {
    year: i64,
    month: usize,
    day: i64,
    /// 0 for Sunday.
    weekday: usize,
    hour: i64,
    minute: i64,
}

impl Local {
    /// `ms` (epoch milliseconds) as the clock `offset` seconds from UTC shows
    /// it.
    pub fn at(ms: u64, offset: i64) -> Local {
        let secs = (ms / 1000) as i64 + offset;
        let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
        // Days since 1970-01-01 to a calendar date (Hinnant's civil_from_days).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        Local {
            year: yoe + era * 400 + i64::from(month <= 2),
            month: (month - 1) as usize,
            day: doy - (153 * mp + 2) / 5 + 1,
            // 1970-01-01 was a Thursday.
            weekday: (days + 4).rem_euclid(7) as usize,
            hour: rest / 3600,
            minute: rest % 3600 / 60,
        }
    }

    /// "Monday, 2 November".
    pub fn date(&self) -> String {
        format!(
            "{}, {} {}",
            DAYS[self.weekday % 7],
            self.day,
            MONTHS[self.month % 12]
        )
    }

    /// "8:58".
    pub fn time(&self) -> String {
        format!("{}:{:02}", self.hour, self.minute)
    }

    /// What to say to the owner at this hour.
    pub fn greeting(&self) -> &'static str {
        match self.hour {
            0..12 => "Good morning",
            12..18 => "Good afternoon",
            _ => "Good evening",
        }
    }

    /// Whether both are in the same calendar day.
    fn same_day(&self, other: &Local) -> bool {
        (self.year, self.month, self.day) == (other.year, other.month, other.day)
    }

    /// "1 Nov", with the year when it is not the one `now` is in.
    fn short_date(&self, now: &Local) -> String {
        let month = &MONTHS[self.month % 12][..3];
        if self.year == now.year {
            format!("{} {month}", self.day)
        } else {
            format!("{} {month} {}", self.day, self.year)
        }
    }
}

/// When something happened, as a thread says it: "9:15" if it was today,
/// "1 Nov" if it was before today. `now` is the moment it is said at.
pub fn said(at_ms: u64, now_ms: u64) -> String {
    said_at(at_ms, now_ms, offset())
}

fn said_at(at_ms: u64, now_ms: u64, offset: i64) -> String {
    let (then, now) = (Local::at(at_ms, offset), Local::at(now_ms, offset));
    if then.same_day(&now) {
        then.time()
    } else {
        then.short_date(&now)
    }
}

/// The same moment in full, for a tooltip: how long ago, and when exactly,
/// such as "3h ago · Monday, 2 November, 9:15".
pub fn in_full(at_ms: u64, now_ms: u64) -> String {
    let then = Local::at(at_ms, offset());
    format!(
        "{} · {}, {}",
        crate::pages::timeline::relative(now_ms, at_ms),
        then.date(),
        then.time()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machines_offset_is_read_from_its_own_words() {
        assert_eq!(parse_offset("+0530"), Some(19_800));
        assert_eq!(parse_offset("-0800"), Some(-28_800));
        assert_eq!(parse_offset("+0000"), Some(0));
        for bad in ["", "0530", "+05", "+05:30", "UTC", "+ab30"] {
            assert_eq!(parse_offset(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_moment_reads_as_the_owners_clock_shows_it() {
        // Monday 2 November 2026, 03:28:20 UTC.
        let ms = 1_793_590_100_000;
        let utc = Local::at(ms, 0);
        assert_eq!(utc.date(), "Monday, 2 November");
        assert_eq!(utc.time(), "3:28");
        assert_eq!(utc.greeting(), "Good morning");
        // Five and a half hours on, it is still that Monday morning; eight
        // hours back it is Sunday evening.
        let india = Local::at(ms, 19_800);
        assert_eq!(
            (india.date(), india.time()),
            ("Monday, 2 November".into(), "8:58".into())
        );
        let west = Local::at(ms, -8 * 3600);
        assert_eq!(west.date(), "Sunday, 1 November");
        assert_eq!(west.greeting(), "Good evening");
        // Midnight and noon belong to the day that starts.
        assert_eq!(Local::at(1_793_577_600_000, 0).greeting(), "Good morning");
        assert_eq!(Local::at(1_793_620_800_000, 0).greeting(), "Good afternoon");
        assert_eq!(Local::at(1_793_642_400_000, 0).greeting(), "Good evening");
        // Leap day, and the epoch itself.
        assert_eq!(Local::at(1_709_251_201_000, 0).date(), "Friday, 1 March");
        assert_eq!(Local::at(0, 0).date(), "Thursday, 1 January");
    }

    #[test]
    fn today_is_a_clock_time_and_before_today_a_date() {
        // Monday 2 November 2026, 08:58 on a clock five and a half hours on.
        let now = 1_793_590_100_000;
        let india = 19_800;
        let minutes = |n: u64| n * 60_000;
        assert_eq!(said_at(now, now, india), "8:58");
        assert_eq!(said_at(now - minutes(43), now, india), "8:15");
        // Before the local midnight, which is 18:30 UTC the day before: the
        // date, and not the time.
        let midnight = now - minutes(8 * 60 + 58);
        assert_eq!(said_at(midnight, now, india), "0:00");
        assert_eq!(said_at(midnight - 60_000, now, india), "1 Nov");
        assert_eq!(said_at(now - 86_400_000 * 3, now, india), "30 Oct");
        // Another year says so.
        assert_eq!(said_at(now - 86_400_000 * 400, now, india), "28 Sep 2025");
    }

    #[test]
    fn a_tooltip_says_how_long_ago_and_when() {
        let now = 1_793_590_100_000;
        let full = in_full(now - 3 * 3_600_000, now);
        assert!(full.starts_with("3h ago · "), "{full}");
        assert!(full.contains(", "), "{full}");
    }
}
