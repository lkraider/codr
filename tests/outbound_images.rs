use herdr_white_noise_bridge::outbound_images;
use std::{
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
};

fn prepare(text: &str, repo: &Path, spool: &Path) -> outbound_images::Prepared {
    outbound_images::prepare(text, repo, spool).unwrap()
}

#[test]
fn only_explicit_markdown_images_are_attached_and_bytes_are_staged_privately() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    std::fs::write(repo.join("screen.png"), b"\x89PNG\r\n\x1a\nfixture").unwrap();
    let spool = dir.path().join("spool");
    let result = prepare(
        "![Screen](screen.png)\n[Download](screen.png)\n`![code](screen.png)`\n\n```md\n![example](screen.png)\n```",
        &repo,
        &spool,
    );
    assert_eq!(result.attachments.len(), 1);
    let file = &result.attachments[0];
    assert_eq!(file.media_type, "image/png");
    assert_eq!(file.file_name, "screen.png");
    assert!(Path::new(&file.path).starts_with(&spool));
    assert_eq!(
        std::fs::read(&file.path).unwrap(),
        b"\x89PNG\r\n\x1a\nfixture"
    );
    assert_eq!(
        std::fs::metadata(&file.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(result.text.contains("[Download](screen.png)"));
    assert!(result.text.contains("`![code](screen.png)`"));
}

#[test]
fn outside_paths_symlinks_nonimages_and_missing_files_get_a_visible_notice() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let secret = dir.path().join("secret.png");
    std::fs::write(&secret, b"\x89PNG\r\n\x1a\nprivate").unwrap();
    symlink(&secret, repo.join("escape.png")).unwrap();
    symlink(dir.path(), repo.join("linked-dir")).unwrap();
    std::fs::write(repo.join("fake.png"), "not an image").unwrap();
    for path in [
        "../secret.png",
        "escape.png",
        "linked-dir/secret.png",
        "fake.png",
        "missing.png",
        "/etc/passwd",
    ] {
        let result = prepare(
            &format!("Text\n![Image]({path})"),
            &repo,
            &dir.path().join("spool"),
        );
        assert!(result.attachments.is_empty(), "{path}");
        assert!(result.text.contains("Image not attached"), "{path}");
        assert!(result.text.starts_with("Text"));
    }
}

#[test]
fn remote_images_are_not_fetched_and_regular_links_are_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let text = "![Remote](https://example.com/image.png)\n[File](/etc/passwd)";
    let result = prepare(text, dir.path(), &dir.path().join("spool"));
    assert!(result.attachments.is_empty());
    assert!(result.text.contains("Image not attached"));
    assert!(result.text.contains("[File](/etc/passwd)"));
}

#[test]
fn image_count_and_size_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("screen.png"), b"\x89PNG\r\n\x1a\nfixture").unwrap();
    let text = "![Screen](screen.png)\n".repeat(5);
    let result = prepare(&text, dir.path(), &dir.path().join("spool"));
    assert_eq!(result.attachments.len(), 4);
    assert!(result.text.contains("Image not attached"));
    let big = std::fs::File::create(dir.path().join("big.png")).unwrap();
    big.set_len(10 * 1024 * 1024 + 1).unwrap();
    assert!(
        prepare("![Big](big.png)", dir.path(), &dir.path().join("spool"))
            .attachments
            .is_empty()
    );
}

#[test]
fn approved_temporary_images_work_without_broadening_repository_access() {
    let root = Path::new("/tmp/opencode");
    std::fs::DirBuilder::new()
        .recursive(true)
        .create(root)
        .unwrap();
    let temporary = tempfile::tempdir_in(root).unwrap();
    let repo = tempfile::tempdir().unwrap();
    let image = temporary.path().join("screenshot.png");
    std::fs::write(&image, b"\x89PNG\r\n\x1a\nfixture").unwrap();
    let result = prepare(
        &format!("![Screen]({})", image.display()),
        repo.path(),
        &repo.path().join("spool"),
    );
    assert_eq!(result.attachments.len(), 1);
}

#[test]
fn snapshot_cleanup_cannot_delete_original_files() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state.json");
    let original = dir.path().join("image-original.png");
    std::fs::write(&original, "original").unwrap();
    outbound_images::remove_snapshot(&state, original.to_str().unwrap());
    assert!(original.exists());
}
