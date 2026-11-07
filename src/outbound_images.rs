//! Explicit reply images only: confined reads, private snapshots, encrypted MDK delivery.
use anyhow::{Result, ensure};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use rustix::fs::{Mode, OFlags, open, openat};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, Permissions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_BATCH_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub path: String,
    pub media_type: String,
    pub file_name: String,
}

pub struct Prepared {
    pub text: String,
    pub attachments: Vec<Attachment>,
}

pub fn spool(state_path: &Path) -> PathBuf {
    state_path.with_extension("outbound-media")
}

pub fn remove_snapshot(state_path: &Path, path: &str) {
    let path = Path::new(path);
    if path.parent() == Some(spool(state_path).as_path())
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("image-"))
    {
        let _ = std::fs::remove_file(path);
    }
}

pub fn prepare(text: &str, cwd: &Path, spool: &Path) -> Result<Prepared> {
    let mut prepared = Prepared {
        text: String::new(),
        attachments: vec![],
    };
    let mut previous = 0;
    let mut total_bytes = 0;
    let mut image = None;
    for (event, range) in Parser::new(text).into_offset_iter() {
        match event {
            Event::Start(Tag::Image { dest_url, .. }) => {
                image = Some((range.start, dest_url.to_string(), String::new()));
            }
            Event::Text(alt) | Event::Code(alt) if image.is_some() => {
                image.as_mut().unwrap().2.push_str(&alt);
            }
            Event::End(TagEnd::Image) => {
                let Some((start, destination, alt)) = image.take() else {
                    continue;
                };
                prepared.text.push_str(&text[previous..start]);
                let staged = if prepared.attachments.len() < 4 {
                    stage(
                        &destination,
                        cwd,
                        spool,
                        MAX_IMAGE_BYTES.min(MAX_BATCH_BYTES - total_bytes),
                    )
                } else {
                    Err(anyhow::anyhow!("Image count exceeded"))
                };
                match staged {
                    Ok((attachment, size)) => {
                        total_bytes += size;
                        prepared.attachments.push(attachment);
                        prepared.text.push_str(if alt.is_empty() { "Image attached." } else { &alt });
                    }
                    Err(_) => prepared.text.push_str("[Image not attached: missing, blocked, unsupported, or over the size/count limit.]"),
                }
                previous = range.end;
            }
            _ => {}
        }
    }
    prepared.text.push_str(&text[previous..]);
    Ok(prepared)
}

fn stage(destination: &str, cwd: &Path, spool: &Path, limit: u64) -> Result<(Attachment, u64)> {
    ensure!(
        !destination.contains("://") && !destination.starts_with("data:"),
        "Remote image blocked"
    );
    let path = Path::new(destination);
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    };
    let mut source = None;
    for root in [cwd, Path::new("/tmp/opencode")] {
        if !root.is_absolute() {
            continue;
        }
        if let Ok(relative) = path.strip_prefix(root)
            && let Ok(file) = read_beneath(root, relative)
        {
            source = Some(file);
            break;
        }
    }
    let file = source.ok_or_else(|| anyhow::anyhow!("Image outside approved folders"))?;
    ensure!(file.metadata()?.len() <= limit, "Image too large");
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "Image too large");
    let (media_type, extension) = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        ("image/png", ".png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        ("image/jpeg", ".jpg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        ("image/gif", ".gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        ("image/webp", ".webp")
    } else {
        anyhow::bail!("Unsupported image format")
    };
    if !spool.exists() {
        std::fs::DirBuilder::new().mode(0o700).create(spool)?;
    }
    let metadata = std::fs::symlink_metadata(spool)?;
    ensure!(
        metadata.is_dir() && metadata.permissions().mode() & 0o077 == 0,
        "Image spool must be a private directory"
    );
    let mut snapshot = tempfile::Builder::new()
        .prefix("image-")
        .suffix(extension)
        .tempfile_in(spool)?;
    snapshot
        .as_file()
        .set_permissions(Permissions::from_mode(0o600))?;
    snapshot.write_all(&bytes)?;
    snapshot.as_file().sync_all()?;
    let (_, saved) = snapshot.keep()?;
    File::open(spool)?.sync_all()?;
    Ok((
        Attachment {
            path: saved.to_string_lossy().into_owned(),
            media_type: media_type.into(),
            file_name: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("image")
                .into(),
        },
        bytes.len() as u64,
    ))
}

fn read_beneath(root: &Path, relative: &Path) -> Result<File> {
    let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    let mut directory = open(root, flags | OFlags::DIRECTORY, Mode::empty())?;
    let mut parts = relative.components().peekable();
    while let Some(component) = parts.next() {
        let Component::Normal(name) = component else {
            anyhow::bail!("Image traversal blocked")
        };
        let leaf = parts.peek().is_none();
        let fd = openat(
            &directory,
            name,
            flags
                | if leaf {
                    OFlags::NONBLOCK
                } else {
                    OFlags::DIRECTORY
                },
            Mode::empty(),
        )?;
        if leaf {
            let file = File::from(fd);
            ensure!(file.metadata()?.is_file(), "Image must be a regular file");
            return Ok(file);
        }
        directory = fd;
    }
    anyhow::bail!("Image path is empty")
}
