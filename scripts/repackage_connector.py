#!/usr/bin/env python3
"""Reuse a published Linux connector only when its source and patch are unchanged."""
import hashlib
import io
import json
from pathlib import Path
import re
import sys
import tarfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    with Path(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def repackage(archive, output_dir):
    """Verify a prior bundle, preserve its executable, and record preview provenance."""
    archive, output_dir = Path(archive), Path(output_dir)
    match = re.fullmatch(r"codr-wn-agent-v[0-9]+\.[0-9]+\.[0-9]+-(linux-(?:x86_64|aarch64))\.tar\.gz", archive.name)
    if not match:
        raise ValueError("Expected a versioned Linux connector archive")
    checksum = archive.with_name(archive.name + ".sha256").read_text().split()
    actual = digest(archive)
    if not checksum or actual != checksum[0].lower():
        raise ValueError("Connector checksum mismatch")
    settings = tomllib.loads((ROOT / "connector.toml").read_text())
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    with tarfile.open(archive, "r:gz") as source:
        entries = source.getmembers()
        if (sorted(entry.name for entry in entries) != ["CODR-CONNECTOR.json", "LICENSE", "wn-agent"]
                or any(not entry.isfile() or not 0 < entry.size <= 256 * 1024 * 1024 for entry in entries)
                or source.getmember("CODR-CONNECTOR.json").size > 1024 * 1024):
            raise ValueError("Unexpected connector archive members")
        provenance = json.load(source.extractfile("CODR-CONNECTOR.json"))
        expected = dict(repository=settings["repository"], revision=settings["revision"],
                        upstream_version=settings["version"], patch_sha256=digest(ROOT / settings["patch"]))
        if any(provenance.get(key) != value for key, value in expected.items()):
            raise ValueError("Connector provenance differs; rebuild the connector")
        provenance.update(reused_from_codr_version=provenance["codr_version"],
                          source_archive_sha256=actual, codr_version=version)
        metadata = (json.dumps(provenance, indent=2) + "\n").encode()
        output_dir.mkdir(parents=True, exist_ok=True)
        output = output_dir / f"codr-wn-agent-v{version}-{match[1]}.tar.gz"
        with tarfile.open(output, "w:gz") as target:
            for entry in entries:
                if entry.name == "CODR-CONNECTOR.json":
                    entry.size = len(metadata)
                    target.addfile(entry, io.BytesIO(metadata))
                else:
                    target.addfile(entry, source.extractfile(entry))
    output.with_name(output.name + ".sha256").write_text(f"{digest(output)}  {output.name}\n")
    return output


if __name__ == "__main__":
    print(repackage(sys.argv[1], sys.argv[2]))
