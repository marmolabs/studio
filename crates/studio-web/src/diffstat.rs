//! Reading `git diff --stat`: the lines a change adds and removes.

/// The summary line, such as "2 files changed, 6 insertions(+), 1 deletion(-)".
fn summary(stat: &str) -> Option<&str> {
    stat.lines().rev().find(|l| l.contains(" changed"))
}

/// Lines added and removed, from the summary line.
pub fn totals(stat: &str) -> Option<(u64, u64)> {
    let summary = summary(stat)?;
    let count = |word: &str| {
        summary
            .split(", ")
            .find(|part| part.contains(word))
            .and_then(|part| part.split_whitespace().next()?.parse().ok())
            .unwrap_or(0)
    };
    Some((count("insertion"), count("deletion")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_gives_the_size() {
        let stat = " src/a.rs | 6 +++++-\n src/b.rs | 1 +\n 2 files changed, 6 insertions(+), 1 deletion(-)\n";
        assert_eq!(totals(stat), Some((6, 1)));
        assert_eq!(totals(" 1 file changed, 3 deletions(-)\n"), Some((0, 3)));
        assert_eq!(totals(""), None);
    }
}
