//! Quotes from history: what a hire's landed tickets usually cost and take.
//! Pure; callers gather the samples.

use serde::{Deserialize, Serialize};

/// The most recent tickets a quote looks at.
pub const QUOTE_SAMPLES: usize = 20;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    pub usd: f64,
    pub minutes: f64,
    pub samples: usize,
}

/// Median spend and duration of past tickets, given as `(usd, duration_ms)`.
/// `None` without any samples.
pub fn quote(samples: &[(f64, u64)]) -> Option<Quote> {
    if samples.is_empty() {
        return None;
    }
    let usd = median(samples.iter().map(|s| s.0).collect());
    let ms = median(samples.iter().map(|s| s.1 as f64).collect());
    Some(Quote {
        usd,
        minutes: ms / 60_000.0,
        samples: samples.len(),
    })
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(|a, b| a.total_cmp(b));
    let mid = values.len() / 2;
    if values.len() % 2 == 1 {
        values[mid]
    } else {
        (values[mid - 1] + values[mid]) / 2.0
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn no_history_no_quote() {
        assert_eq!(quote(&[]), None);
    }

    #[test]
    fn one_sample_is_the_quote() {
        let q = quote(&[(0.5, 120_000)]).unwrap();
        assert_eq!(q.usd, 0.5);
        assert_eq!(q.minutes, 2.0);
        assert_eq!(q.samples, 1);
    }

    #[test]
    fn median_ignores_outliers() {
        let q = quote(&[(0.1, 60_000), (100.0, 6_000_000), (0.3, 180_000)]).unwrap();
        assert_eq!(q.usd, 0.3);
        assert_eq!(q.minutes, 3.0);
    }

    #[test]
    fn even_counts_average_the_middle() {
        let q = quote(&[(1.0, 60_000), (3.0, 180_000)]).unwrap();
        assert_eq!(q.usd, 2.0);
        assert_eq!(q.minutes, 2.0);
        assert_eq!(q.samples, 2);
    }
}
