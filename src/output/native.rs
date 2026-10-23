use anyhow::{Result, ensure};
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_RECORD_BYTES: u64 = 16 * 1024 * 1024;

pub fn session_id(value: &str) -> Result<String> {
    Ok(uuid::Uuid::parse_str(value)
        .map_err(|_| anyhow::anyhow!("Invalid native session UUID"))?
        .hyphenated()
        .to_string())
}

pub fn find(roots: &[(PathBuf, usize)], matches: impl Fn(&str) -> bool) -> Result<PathBuf> {
    let mut found = Vec::new();
    for (root, depth) in roots {
        scan(root, *depth, &matches, &mut found)?;
    }
    ensure!(
        found.len() == 1,
        "Native transcript is missing or ambiguous"
    );
    Ok(found.remove(0))
}

fn scan(
    root: &Path,
    depth: usize,
    matches: &impl Fn(&str) -> bool,
    found: &mut Vec<PathBuf>,
) -> Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(anyhow::anyhow!("Cannot scan native transcript directory")),
    };
    for entry in entries {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_file() && entry.file_name().to_str().is_some_and(matches) {
            found.push(entry.path());
            ensure!(found.len() <= 1, "Native transcript is ambiguous");
        } else if kind.is_dir() && depth > 0 {
            scan(&entry.path(), depth - 1, matches, found)?;
        }
    }
    Ok(())
}

pub fn records(path: &Path, visit: impl FnMut(Value) -> Result<()>) -> Result<()> {
    ensure!(
        fs::metadata(path)?.len() <= MAX_FILE_BYTES,
        "Native transcript exceeds 128 MiB"
    );
    records_from(path, 0, visit).map(drop)
}

/// Visits the complete records that start at `offset` and returns the offset after the last one.
pub fn records_from(
    path: &Path,
    offset: u64,
    mut visit: impl FnMut(Value) -> Result<()>,
) -> Result<u64> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::new(file.take(length.saturating_sub(offset)));
    let mut end = offset;
    loop {
        let mut line = Vec::new();
        let bytes = reader
            .by_ref()
            .take(MAX_RECORD_BYTES + 1)
            .read_until(b'\n', &mut line)?;
        if bytes == 0 {
            break;
        }
        ensure!(
            bytes as u64 <= MAX_RECORD_BYTES,
            "Native transcript record exceeds 16 MiB"
        );
        // Writers append asynchronously. An incomplete last record becomes visible on the next read.
        if line.last() != Some(&b'\n') {
            break;
        }
        end += bytes as u64;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let row = serde_json::from_slice(&line)
            .map_err(|_| anyhow::anyhow!("Invalid native transcript record"))?;
        visit(row)?;
    }
    Ok(end)
}
