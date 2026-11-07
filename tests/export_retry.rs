// Adapted from Juraj Bednar's shared PR #17–#19 export-retry tests (afe4f38).
use herdr_white_noise_bridge::output;
use std::{os::unix::fs::PermissionsExt, path::Path};

static EXPORT_TEST: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn script(root: &Path, body: &str) -> std::path::PathBuf {
    let path = root.join("opencode");
    std::fs::write(
        &path,
        format!(
            r#"#!/bin/sh
count="$(dirname "$0")/attempts"
n=$(cat "$count" 2>/dev/null || echo 0)
n=$((n + 1))
printf '%s' "$n" > "$count"
{body}
"#
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn attempts(root: &Path) -> usize {
    std::fs::read_to_string(root.join("attempts"))
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test]
async fn incomplete_export_recovers_on_the_third_read() {
    let _guard = EXPORT_TEST.lock().await;
    let root = tempfile::tempdir().unwrap();
    let bin = script(
        root.path(),
        r#"
if [ "$n" -le 2 ]; then printf '%s' '{"messages":['; else printf '%s' '{"messages":[]}'; fi
"#,
    );
    output::export(&bin, "ses_1").await.unwrap();
    assert_eq!(attempts(root.path()), 3);
}

#[tokio::test]
async fn incomplete_export_is_bounded_per_api() {
    let _guard = EXPORT_TEST.lock().await;
    let root = tempfile::tempdir().unwrap();
    let bin = script(root.path(), "printf '%s' '{\"messages\":['");
    let error = output::export(&bin, "ses_1").await.unwrap_err();
    assert_eq!(attempts(root.path()), 6);
    assert_eq!(
        output::read_error_message(&error),
        "OpenCode export stayed incomplete after retries"
    );
}

#[tokio::test]
async fn permanent_parse_and_command_failures_are_not_retried() {
    let _guard = EXPORT_TEST.lock().await;
    for body in [
        "printf '%s' '{\"messages\":42}'",
        "printf '%s' '{broken}'",
        "exit 1",
    ] {
        let root = tempfile::tempdir().unwrap();
        let bin = script(root.path(), body);
        let error = output::export(&bin, "ses_1").await.unwrap_err();
        assert_eq!(attempts(root.path()), 2);
        assert_ne!(
            output::read_error_message(&error),
            "OpenCode export stayed incomplete after retries"
        );
    }
}

#[tokio::test]
async fn incomplete_evidence_survives_an_unsupported_fallback() {
    let _guard = EXPORT_TEST.lock().await;
    // Warm each preferred API so both fallback orders are covered without shared
    // environment variables or dependence on the global API cache's initial value.
    for supported in ["--pure", "session"] {
        let root = tempfile::tempdir().unwrap();
        let bin = script(
            root.path(),
            &format!(
                r#"
if [ "$1" != "{supported}" ]; then exit 1; fi
if [ -f "$(dirname "$0")/truncate" ]; then printf '%s' '{{"messages":['; else printf '%s' '{{"messages":[]}}'; fi
"#
            ),
        );
        output::export(&bin, "ses_1").await.unwrap();
        std::fs::write(root.path().join("attempts"), "0").unwrap();
        std::fs::write(root.path().join("truncate"), "").unwrap();
        let error = output::export(&bin, "ses_1").await.unwrap_err();
        assert_eq!(attempts(root.path()), 4);
        assert_eq!(
            output::read_error_message(&error),
            "OpenCode export stayed incomplete after retries"
        );
    }
}
