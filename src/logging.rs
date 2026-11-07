//! Warning throttling adapted from Juraj Bednar's PR #19.
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

const REPEAT_AFTER: Duration = Duration::from_secs(600);
const MAX_LINES: usize = 256;

struct Seen {
    printed: Instant,
    suppressed: u64,
}

static SEEN: Mutex<Option<HashMap<String, Seen>>> = Mutex::new(None);

/// Print a distinct line once per ten minutes, counting suppressed repeats.
pub fn throttled(line: String) {
    let text = {
        let mut guard = SEEN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        admit(
            guard.get_or_insert_with(HashMap::new),
            &line,
            Instant::now(),
        )
    };
    if let Some(text) = text {
        eprintln!("{text}");
    }
}

fn admit(seen: &mut HashMap<String, Seen>, line: &str, at: Instant) -> Option<String> {
    if seen.len() >= MAX_LINES && !seen.contains_key(line) {
        seen.retain(|_, entry| at.duration_since(entry.printed) < REPEAT_AFTER);
        if seen.len() >= MAX_LINES {
            let oldest = seen
                .iter()
                .min_by_key(|(_, entry)| entry.printed)
                .map(|(line, _)| line.clone());
            if let Some(oldest) = oldest {
                seen.remove(&oldest);
            }
        }
    }
    match seen.get_mut(line) {
        Some(entry) if at.duration_since(entry.printed) < REPEAT_AFTER => {
            entry.suppressed = entry.suppressed.saturating_add(1);
            None
        }
        Some(entry) => {
            let text = if entry.suppressed == 0 {
                line.to_string()
            } else {
                format!("{line} (repeated {} more times)", entry.suppressed)
            };
            *entry = Seen {
                printed: at,
                suppressed: 0,
            };
            Some(text)
        }
        None => {
            seen.insert(
                line.to_string(),
                Seen {
                    printed: at,
                    suppressed: 0,
                },
            );
            Some(line.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_are_counted_until_the_window_expires() {
        let mut seen = HashMap::new();
        let at = Instant::now();
        assert_eq!(admit(&mut seen, "warning", at).as_deref(), Some("warning"));
        assert_eq!(
            admit(&mut seen, "warning", at + Duration::from_secs(1)),
            None
        );
        assert_eq!(
            admit(&mut seen, "other", at + Duration::from_secs(2)).as_deref(),
            Some("other")
        );
        assert_eq!(
            admit(&mut seen, "warning", at + Duration::from_secs(3)),
            None
        );
        assert_eq!(
            admit(&mut seen, "warning", at + REPEAT_AFTER).as_deref(),
            Some("warning (repeated 2 more times)")
        );
        assert_eq!(
            admit(
                &mut seen,
                "warning",
                at + REPEAT_AFTER + Duration::from_secs(1)
            ),
            None
        );
    }

    #[test]
    fn a_full_cache_keeps_new_errors_visible_and_remains_bounded() {
        let mut seen = HashMap::new();
        let at = Instant::now();
        for n in 0..MAX_LINES {
            let line = format!("Renamed group: {n}");
            assert_eq!(
                admit(&mut seen, &line, at + Duration::from_millis(n as u64)).as_deref(),
                Some(line.as_str())
            );
        }
        let later = at + Duration::from_secs(1);
        assert_eq!(
            admit(&mut seen, "MDK send unavailable", later).as_deref(),
            Some("MDK send unavailable")
        );
        assert_eq!(seen.len(), MAX_LINES);
        assert!(!seen.contains_key("Renamed group: 0"));
        assert_eq!(admit(&mut seen, "MDK send unavailable", later), None);
        assert_eq!(
            admit(&mut seen, "Renamed group: 0", later).as_deref(),
            Some("Renamed group: 0")
        );
        assert_eq!(seen.len(), MAX_LINES);
    }
}
