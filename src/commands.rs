//! Command spelling: exact names, unambiguous prefixes, and bare words in the control chat.

pub(crate) enum Normalized {
    Text(String),
    Ambiguous(Vec<&'static str>),
}

const COMMANDS: &[&str] = &[
    "agent", "allow", "answer", "cancel", "help", "last", "model", "new", "queue", "recent",
    "reject", "status", "sync", "unmute", "update",
];

pub(crate) fn normalize(text: &str, control: bool, menu_open: bool) -> Normalized {
    let trimmed = text.trim();
    let body = match trimmed.strip_prefix('/') {
        Some(body) => body,
        None if control && !menu_open => trimmed,
        None => return Normalized::Text(text.to_owned()),
    };
    let first = body.split_whitespace().next().unwrap_or("");
    if first.is_empty() {
        return Normalized::Text(text.to_owned());
    }
    let lower = first.to_ascii_lowercase();
    for command in ["recent", "last"] {
        if let Some(number) = lower
            .strip_prefix(command)
            .filter(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Normalized::Text(format!(
                "/{command} {number}{}",
                body.strip_prefix(first).unwrap_or("")
            ));
        }
    }
    let matches: Vec<&'static str> =
        if let Some(exact) = COMMANDS.iter().copied().find(|command| *command == lower) {
            vec![exact]
        } else {
            COMMANDS
                .iter()
                .copied()
                .filter(|command| command.starts_with(&lower))
                .collect()
        };
    match matches.as_slice() {
        [command] => {
            let rest = body.strip_prefix(first).unwrap_or("").trim_start();
            Normalized::Text(if rest.is_empty() {
                format!("/{command}")
            } else {
                format!("/{command} {rest}")
            })
        }
        [] => Normalized::Text(text.to_owned()),
        _ => Normalized::Ambiguous(matches),
    }
}

#[cfg(test)]
mod tests {
    use super::{Normalized, normalize};

    fn text(value: &str, control: bool, menu_open: bool) -> String {
        match normalize(value, control, menu_open) {
            Normalized::Text(text) => text,
            Normalized::Ambiguous(options) => panic!("unexpected ambiguity: {options:?}"),
        }
    }

    #[test]
    fn exact_and_prefixed_commands_share_one_spelling() {
        assert_eq!(text("/new", false, false), "/new");
        assert_eq!(text("/n", false, false), "/new");
        assert_eq!(text("/ne", false, false), "/new");
        assert_eq!(text("/st", false, false), "/status");
        assert_eq!(text("/sy on", false, false), "/sync on");
        assert_eq!(text("/c", false, false), "/cancel");
        assert_eq!(text("/h", false, false), "/help");
        assert_eq!(text("/NEW", false, false), "/new");
        assert_eq!(text("/N", false, false), "/new");
        assert_eq!(text("/StAtUs", false, false), "/status");
        assert_eq!(text("NeW", true, false), "/new");
    }

    #[test]
    fn recent_hours_accept_compact_and_spaced_forms() {
        for control in [false, true] {
            for hours in ["2", "4", "24"] {
                let expected = format!("/recent {hours}");
                assert_eq!(text(&format!("/recent{hours}"), control, false), expected);
                assert_eq!(text(&format!("/recent {hours}"), control, false), expected);
            }
        }
        assert_eq!(text("recent24", true, false), "/recent 24");
        assert_eq!(text("/ReCeNt2", false, false), "/recent 2");
        assert_eq!(text("recent2", false, false), "recent2");
        assert_eq!(text("recent2", true, true), "recent2");
        assert_eq!(text("/recent2 extra", false, false), "/recent 2 extra");
        assert_eq!(text("/recently", false, false), "/recently");
    }

    #[test]
    fn last_counts_normalize_without_rewriting_menu_or_ordinary_input() {
        for command in ["/last4", "/last 4", "/LAST4", "/l 4"] {
            assert_eq!(text(command, false, false), "/last 4");
        }
        assert_eq!(text("last4", true, false), "/last 4");
        assert_eq!(text("l 4", true, false), "/last 4");
        assert_eq!(text("last4", false, false), "last4");
        assert_eq!(text("last4", true, true), "last4");
        assert_eq!(text("/lasting", false, false), "/lasting");
    }

    #[test]
    fn updater_prefixes_do_not_collide_with_unmute() {
        assert_eq!(text("/up", false, false), "/update");
        assert_eq!(text("update", true, false), "/update");
        assert_eq!(text("/un", false, false), "/unmute");
        match normalize("/u", true, false) {
            Normalized::Ambiguous(options) => assert_eq!(options, vec!["unmute", "update"]),
            _ => panic!("/u must not choose between unmute and update"),
        }
    }

    #[test]
    fn bare_words_are_commands_only_in_the_control_chat() {
        assert_eq!(text("new", true, false), "/new");
        assert_eq!(text("n", true, false), "/new");
        assert_eq!(text("status", true, false), "/status");
        assert_eq!(text("new", false, false), "new");
        assert_eq!(text("n", false, false), "n");
        assert_eq!(text("hello", true, false), "hello");
    }

    #[test]
    fn bare_words_stay_menu_input_while_a_launch_menu_is_open() {
        assert_eq!(text("new", true, true), "new");
        assert_eq!(text("n", true, true), "n");
        assert_eq!(text("/n", true, true), "/new");
    }

    #[test]
    fn ambiguous_prefixes_are_reported_and_unknown_text_is_untouched() {
        match normalize("/s", true, false) {
            Normalized::Ambiguous(options) => assert_eq!(options, vec!["status", "sync"]),
            _ => panic!("expected ambiguous /s"),
        }
        match normalize("r", true, false) {
            Normalized::Ambiguous(options) => assert_eq!(options, vec!["recent", "reject"]),
            _ => panic!("expected ambiguous r"),
        }
        assert_eq!(text("/clear-queue", false, false), "/clear-queue");
        assert_eq!(text("/nonsense", false, false), "/nonsense");
    }
}
