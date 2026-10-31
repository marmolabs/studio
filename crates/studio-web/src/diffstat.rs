//! Reading `git diff --stat`: the lines a change adds and removes, and how
//! many files it touches.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Mutex;
use studio_core::ticket::Ticket;
use studiod::api::Request;

use crate::client;

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

/// The number of files changed, from the summary line.
pub fn files(stat: &str) -> Option<u64> {
    summary(stat)?.split_whitespace().next()?.parse().ok()
}

/// Lines added and removed by each of `tickets`, by id; one without a
/// worktree is left out. A ticket's diff only changes with a new attempt,
/// so git is asked once per attempt, and for every new one in one request.
pub async fn sizes(socket: &Path, tickets: &[&Ticket]) -> HashMap<String, (u64, u64)> {
    static SEEN: Mutex<BTreeMap<(String, u32), (u64, u64)>> = Mutex::new(BTreeMap::new());
    let mut sizes = HashMap::new();
    let mut asked = Vec::new();
    if let Ok(seen) = SEEN.lock() {
        for ticket in tickets {
            let id = ticket.id.to_string();
            match seen.get(&(id.clone(), ticket.attempts)) {
                Some(size) => {
                    sizes.insert(id, *size);
                }
                None => asked.push(id),
            }
        }
    }
    if asked.is_empty() {
        return sizes;
    }
    let stats: HashMap<String, Option<String>> =
        client::call(socket, Request::Diffstats { tickets: asked })
            .await
            .unwrap_or_default();
    let found = stats
        .into_iter()
        .filter_map(|(id, stat)| Some((id, totals(&stat?)?)));
    for (id, size) in found {
        let attempts = tickets.iter().find(|t| t.id.to_string() == id);
        if let (Some(ticket), Ok(mut seen)) = (attempts, SEEN.lock()) {
            seen.insert((id.clone(), ticket.attempts), size);
        }
        sizes.insert(id, size);
    }
    sizes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_gives_the_size_and_the_files() {
        let stat = " src/a.rs | 6 +++++-\n src/b.rs | 1 +\n 2 files changed, 6 insertions(+), 1 deletion(-)\n";
        assert_eq!(totals(stat), Some((6, 1)));
        assert_eq!(files(stat), Some(2));
        assert_eq!(totals(" 1 file changed, 3 deletions(-)\n"), Some((0, 3)));
        assert_eq!(totals(""), None);
        assert_eq!(files(""), None);
    }
}
