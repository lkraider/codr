import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("repackage_connector", ROOT / "scripts/repackage_connector.py")
repack = importlib.util.module_from_spec(spec)
spec.loader.exec_module(repack)


class RepackageConnectorTests(unittest.TestCase):
    def provenance(self):
        settings = tomllib.loads((ROOT / "connector.toml").read_text())
        return dict(codr_version="0.6.0", repository=settings["repository"],
                    revision=settings["revision"], upstream_version=settings["version"],
                    patch_sha256=hashlib.sha256((ROOT / settings["patch"]).read_bytes()).hexdigest())

    def archive(self, directory, provenance=None, bad_member=None):
        archive = Path(directory) / "codr-wn-agent-v0.6.0-linux-x86_64.tar.gz"
        entries = [("wn-agent", b"unchanged executable"), ("LICENSE", b"original license"),
                   ("CODR-CONNECTOR.json", json.dumps(provenance or self.provenance()).encode())]
        with tarfile.open(archive, "w:gz") as bundle:
            for name, content in entries:
                entry = tarfile.TarInfo(name)
                entry.size = len(content)
                entry.mode = 0o755 if name == "wn-agent" else 0o644
                if bad_member == "symlink" and name == "wn-agent":
                    entry.type = tarfile.SYMTYPE
                    entry.linkname = "/unexpected"
                    entry.size = 0
                bundle.addfile(entry, io.BytesIO(content))
            if bad_member == "extra":
                bundle.addfile(tarfile.TarInfo("../unexpected"))
            if bad_member == "duplicate":
                bundle.addfile(tarfile.TarInfo("wn-agent"))
        archive.with_name(archive.name + ".sha256").write_text(hashlib.sha256(archive.read_bytes()).hexdigest() + "  " + archive.name + "\n")
        return archive

    def test_matching_connector_is_reused_with_explicit_provenance_and_checksum(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = self.archive(directory)
            output = repack.repackage(archive, Path(directory) / "dist")
            version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
            self.assertEqual(output.name, f"codr-wn-agent-v{version}-linux-x86_64.tar.gz")
            with tarfile.open(output) as bundle:
                self.assertEqual(sorted(bundle.getnames()), ["CODR-CONNECTOR.json", "LICENSE", "wn-agent"])
                self.assertEqual(bundle.extractfile("wn-agent").read(), b"unchanged executable")
                self.assertEqual(bundle.extractfile("LICENSE").read(), b"original license")
                provenance = json.load(bundle.extractfile("CODR-CONNECTOR.json"))
                self.assertEqual(provenance["codr_version"], version)
                self.assertEqual(provenance["reused_from_codr_version"], "0.6.0")
                self.assertEqual(provenance["source_archive_sha256"], hashlib.sha256(archive.read_bytes()).hexdigest())
                for key in ("repository", "revision", "upstream_version", "patch_sha256"):
                    self.assertEqual(provenance[key], self.provenance()[key])
            self.assertEqual(output.with_name(output.name + ".sha256").read_text().split()[0], hashlib.sha256(output.read_bytes()).hexdigest())

    def test_corrupt_archive_cannot_be_published(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = self.archive(directory)
            archive.write_bytes(archive.read_bytes() + b"corrupt")
            with self.assertRaisesRegex(ValueError, "checksum"):
                repack.repackage(archive, Path(directory) / "dist")
            self.assertFalse((Path(directory) / "dist").exists())

    def test_changed_connector_pin_or_patch_requires_a_rebuild(self):
        for field in ("revision", "repository", "upstream_version", "patch_sha256"):
            with self.subTest(field=field), tempfile.TemporaryDirectory() as directory:
                provenance = self.provenance()
                provenance[field] = "different"
                archive = self.archive(directory, provenance)
                with self.assertRaisesRegex(ValueError, "provenance"):
                    repack.repackage(archive, Path(directory) / "dist")
                self.assertFalse((Path(directory) / "dist").exists())

    def test_unexpected_duplicate_or_nonregular_members_are_rejected(self):
        for bad_member in ("symlink", "extra", "duplicate"):
            with self.subTest(member=bad_member), tempfile.TemporaryDirectory() as directory:
                archive = self.archive(directory, bad_member=bad_member)
                with self.assertRaisesRegex(ValueError, "members"):
                    repack.repackage(archive, Path(directory) / "dist")
                self.assertFalse((Path(directory) / "dist").exists())


if __name__ == "__main__":
    unittest.main()
