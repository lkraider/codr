import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import plistlib
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))
import install as installer
import services

spec = importlib.util.spec_from_file_location("codr_setup", SCRIPTS / "setup.py")
setup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(setup)


class InstallerTests(unittest.TestCase):
    def test_release_installs_its_dedicated_connector_before_the_bridge(self):
        with tempfile.TemporaryDirectory() as temporary:
            with patch.dict(os.environ, {"HOME": temporary}), patch.object(installer, "version", return_value="0.6.0"), patch.object(installer, "platform_name", return_value="linux-x86_64"), patch.object(installer, "install_bundle") as install:
                self.assertEqual(installer.main(), 0)
            self.assertEqual(len(install.call_args_list), 2)
            connector, bridge = [call.args for call in install.call_args_list]
            self.assertIn("codr-wn-agent-v0.6.0-linux-x86_64.tar.gz", connector[0])
            self.assertEqual(connector[1], "wn-agent")
            self.assertEqual(connector[2], Path(temporary) / ".local/share/codr/bin/wn-agent-codr-v0.6.0")
            self.assertIn("codr-v0.6.0-linux-x86_64.tar.gz", bridge[0])
            self.assertEqual(bridge[1], "herdr-white-noise-bridge")

    def test_supported_architectures_and_rejection(self):
        self.assertEqual(installer.platform_name("Linux", "x86_64"), "linux-x86_64")
        self.assertEqual(installer.platform_name("Linux", "arm64"), "linux-aarch64")
        self.assertEqual(installer.platform_name("Darwin", "arm64"), "macos-aarch64")
        self.assertEqual(installer.platform_name("Darwin", "x86_64"), "macos-x86_64")
        self.assertEqual(installer.mdk_platform_name("Darwin", "arm64"), "darwin-aarch64")
        with self.assertRaises(installer.InstallError):
            installer.platform_name("Windows", "x86_64")

    def test_downloaded_archive_is_checked_before_installation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "source.tar.gz"
            binary = b"#!/bin/sh\nprintf 'test binary\\n'\n"
            with tarfile.open(archive, "w:gz") as bundle:
                entry = tarfile.TarInfo("herdr-white-noise-bridge")
                entry.size = len(binary)
                bundle.addfile(entry, io.BytesIO(binary))
            payload = archive.read_bytes()
            checksum = hashlib.sha256(payload).hexdigest().encode() + b"  source.tar.gz\n"

            def download(url, destination):
                destination.write_bytes(checksum if url.endswith(".sha256") else payload)

            target = root / "bin/bridge"
            with patch.object(installer, "download", side_effect=download):
                installer.install_bundle("https://github.com/example/release.tar.gz", "herdr-white-noise-bridge", target)
            self.assertEqual(target.read_bytes(), binary)
            self.assertEqual(target.stat().st_mode & 0o777, 0o755)
            checksum = b"0" * 64 + b"\n"
            with patch.object(installer, "download", side_effect=download):
                with self.assertRaises(installer.InstallError):
                    installer.install_bundle("https://github.com/example/release.tar.gz", "herdr-white-noise-bridge", target)
            self.assertEqual(target.read_bytes(), binary)

    def test_symlink_archive_member_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "source.tar.gz"
            with tarfile.open(archive, "w:gz") as bundle:
                entry = tarfile.TarInfo("bridge")
                entry.type = tarfile.SYMTYPE
                entry.linkname = "/etc/passwd"
                bundle.addfile(entry)
            payload = archive.read_bytes()

            def download(url, destination):
                destination.write_bytes(hashlib.sha256(payload).hexdigest().encode() if url.endswith(".sha256") else payload)

            with patch.object(installer, "download", side_effect=download):
                with self.assertRaises(installer.InstallError):
                    installer.install_bundle("https://github.com/example/release.tar.gz", "bridge", root / "installed")
            self.assertFalse((root / "installed").exists())


class ManifestTests(unittest.TestCase):
    def test_manifest_declares_both_host_platforms(self):
        manifest = setup.tomllib.loads((SCRIPTS.parent / "herdr-plugin.toml").read_text())
        self.assertIn("linux", manifest["platforms"])
        self.assertIn("macos", manifest["platforms"])
        self.assertEqual(manifest["version"], setup.tomllib.loads((SCRIPTS.parent / "Cargo.toml").read_text())["package"]["version"])


class SetupTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.home = Path(self.temporary.name)
        self.environment = patch.dict(os.environ, {"HOME": str(self.home), "XDG_CONFIG_HOME": str(self.home / "config space %"), "XDG_STATE_HOME": str(self.home / "state"), "CODR_OWNER_NPUB": "", "CODR_SERVICE_MANAGER": "systemd"}, clear=True)
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.addCleanup(self.temporary.cleanup)
        self.bridge = self.home / ".local/bin/herdr-white-noise-bridge"
        self.bridge.parent.mkdir(parents=True)
        self.bridge.write_text("fixture")
        self.socket = self.home / "named-herdr.sock"
        self.calls = []
        self.bootstrap_count = 0
        self.config = self.home / "config space %/herdr-white-noise-bridge/config.toml"

    def runner(self, argv):
        self.calls.append([str(a) for a in argv])
        if "bootstrap" in argv:
            self.bootstrap_count += 1
            return json.dumps({"account_id_hex": "ab" * 32, "npub": "npub1bridge", "label": "herdr-bridge", "key_package_published": True})
        if "--version" in argv:
            return f"wn-agent {setup.MDK_VERSION}"
        if "check-readers" in argv:
            return json.dumps({"checked_sessions": 0})
        return ""

    def mocks(self):
        return patch.multiple(setup,
            find_executable=lambda name, candidates=(): "/usr/bin/" + name,
            ensure_mdk=lambda runner: Path("/usr/bin/wn-agent"),
            read_snapshot=lambda path: {"agents": []},
            local_accounts=lambda path: [],
            configure_phonon=lambda *args: ["/usr/bin/python3", "/example/transcribe.py", "--socket", "/example/phonon.sock"])

    def test_connector_selection_uses_the_release_owned_path_not_a_shared_executable(self):
        dedicated = installer.connector_path(installer.version())
        def install(release):
            self.assertEqual(release, installer.version())
            dedicated.parent.mkdir(parents=True, exist_ok=True)
            dedicated.write_text("fixture")
        with patch.object(setup, "install_connector", side_effect=install) as download:
            self.assertEqual(setup.ensure_mdk(lambda argv: "wn-agent 0.13.0"), dedicated)
            self.assertEqual(setup.ensure_mdk(lambda argv: "wn-agent 0.13.0"), dedicated)
            self.assertEqual(download.call_count, 1)

    def test_managed_transport_upgrade_changes_only_the_executable(self):
        with self.mocks():
            setup.configure("npub1fixture", self.socket, self.runner)
        existing = setup.tomllib.loads(self.config.read_text())
        unit = self.config.parents[1] / "systemd/user/wn-agent-herdr.service"
        original = unit.read_text()
        binary = installer.connector_path(installer.version())
        before_config = self.config.read_bytes()
        with patch.object(setup, "ensure_mdk", return_value=binary):
            self.assertTrue(setup.upgrade_transport(existing, self.config.parents[1], self.runner))
        self.assertEqual(unit.read_text(), original.replace(setup.unit_argument("/usr/bin/wn-agent"), setup.unit_argument(binary), 1))
        self.assertEqual(self.config.read_bytes(), before_config)
        self.assertIn(["systemctl", "--user", "stop", "herdr-white-noise-bridge.service"], self.calls)
        self.assertIn(["systemctl", "--user", "start", "wn-agent-herdr.service"], self.calls)

    def test_failed_transport_upgrade_restores_the_unit_and_attempts_both_services(self):
        with self.mocks():
            setup.configure("npub1fixture", self.socket, self.runner)
        existing = setup.tomllib.loads(self.config.read_text())
        unit = self.config.parents[1] / "systemd/user/wn-agent-herdr.service"
        original = unit.read_bytes()
        failed = False
        def runner(argv):
            nonlocal failed
            self.calls.append(argv)
            if "--version" in argv:
                return f"wn-agent {setup.MDK_VERSION}"
            if argv == ["systemctl", "--user", "start", "wn-agent-herdr.service"] and not failed:
                failed = True
                raise setup.SetupError("fixture startup failure")
            return ""
        with patch.object(setup, "ensure_mdk", return_value=installer.connector_path(installer.version())):
            with self.assertRaises(setup.SetupError):
                setup.upgrade_transport(existing, self.config.parents[1], runner)
        self.assertEqual(unit.read_bytes(), original)
        self.assertEqual(self.calls[-2:], [["systemctl", "--user", "start", "wn-agent-herdr.service"], ["systemctl", "--user", "start", "herdr-white-noise-bridge.service"]])

    def test_custom_transport_overrides_are_not_replaced(self):
        with self.mocks():
            setup.configure("npub1fixture", self.socket, self.runner)
        unit = self.config.parents[1] / "systemd/user/wn-agent-herdr.service"
        overrides = unit.with_name(unit.name + ".d")
        overrides.mkdir()
        override = overrides / "custom.conf"
        override.write_text("[Service]\nEnvironment=KEEP_ME=1\n")
        self.calls.clear()
        with patch.object(setup, "ensure_mdk", return_value=installer.connector_path(installer.version())):
            with self.assertRaises(setup.SetupError):
                setup.upgrade_transport(setup.tomllib.loads(self.config.read_text()), self.config.parents[1], self.runner)
        self.assertEqual(override.read_text(), "[Service]\nEnvironment=KEEP_ME=1\n")
        self.assertFalse(self.calls)

    def test_inactive_services_after_successful_start_trigger_systemd_rollback(self):
        with self.mocks():
            setup.configure("npub1fixture", self.socket, self.runner)
        unit = self.config.parents[1] / "systemd/user/wn-agent-herdr.service"
        original = unit.read_bytes()
        def runner(argv):
            if "is-active" in argv:
                raise setup.SetupError("fixture activation failure")
            return self.runner(argv)
        with patch.object(setup, "ensure_mdk", return_value=installer.connector_path(installer.version())):
            with self.assertRaises(setup.SetupError):
                setup.upgrade_transport(setup.tomllib.loads(self.config.read_text()), self.config.parents[1], runner)
        self.assertEqual(unit.read_bytes(), original)
        self.assertEqual(self.calls[-2:], [["systemctl", "--user", "start", "wn-agent-herdr.service"], ["systemctl", "--user", "start", "herdr-white-noise-bridge.service"]])

    def test_failed_bridge_start_rolls_back_the_running_connector_not_just_its_unit(self):
        with self.mocks():
            setup.configure("npub1fixture", self.socket, self.runner)
        unit = self.config.parents[1] / "systemd/user/wn-agent-herdr.service"
        original = unit.read_bytes()
        running = {"transport": original}
        failed = False
        def runner(argv):
            nonlocal failed
            if "--version" in argv:
                return f"wn-agent {setup.MDK_VERSION}"
            if argv == ["systemctl", "--user", "stop", "wn-agent-herdr.service"]:
                running["transport"] = None
            elif argv == ["systemctl", "--user", "start", "wn-agent-herdr.service"] and running["transport"] is None:
                running["transport"] = unit.read_bytes()
            elif argv == ["systemctl", "--user", "start", "herdr-white-noise-bridge.service"] and not failed:
                failed = True
                self.assertNotEqual(running["transport"], original)
                raise setup.SetupError("bridge startup failed after transport activation")
            return ""
        with patch.object(setup, "ensure_mdk", return_value=installer.connector_path(installer.version())):
            with self.assertRaises(setup.SetupError):
                setup.upgrade_transport(setup.tomllib.loads(self.config.read_text()), self.config.parents[1], runner)
        self.assertEqual(unit.read_bytes(), original)
        self.assertEqual(running["transport"], original)

    def test_effective_runtime_and_system_dropins_are_refused_before_stopping_services(self):
        with self.mocks():
            setup.configure("npub1fixture", self.socket, self.runner)
        unit = self.config.parents[1] / "systemd/user/wn-agent-herdr.service"
        original = unit.read_bytes()
        for external in ("/run/user/123/systemd/user/wn-agent-herdr.service.d/runtime.conf", "/etc/systemd/user/wn-agent-herdr.service.d/system.conf"):
            calls = []
            def runner(argv):
                calls.append(argv)
                return external if "DropInPaths" in argv else ""
            with self.subTest(external=external), patch.object(setup, "ensure_mdk", return_value=installer.connector_path(installer.version())):
                with self.assertRaises(setup.SetupError):
                    setup.upgrade_transport(setup.tomllib.loads(self.config.read_text()), self.config.parents[1], runner)
                self.assertEqual(unit.read_bytes(), original)
                self.assertFalse(any("stop" in argv for argv in calls))

    def test_unknown_existing_connector_cohorts_are_not_downgraded(self):
        with self.mocks():
            setup.configure("npub1fixture", self.socket, self.runner)
        unit = self.config.parents[1] / "systemd/user/wn-agent-herdr.service"
        original = unit.read_bytes()
        calls = []
        def runner(argv):
            calls.append(argv)
            return "wn-agent 0.14.0" if "--version" in argv else ""
        with patch.object(setup, "ensure_mdk", return_value=installer.connector_path(installer.version())):
            with self.assertRaisesRegex(setup.SetupError, "supported MDK cohort"):
                setup.upgrade_transport(setup.tomllib.loads(self.config.read_text()), self.config.parents[1], runner)
        self.assertEqual(unit.read_bytes(), original)
        self.assertFalse(any("stop" in argv for argv in calls))

    def test_interrupted_upgrade_restores_services_before_propagating_interrupt(self):
        with self.mocks():
            setup.configure("npub1fixture", self.socket, self.runner)
        unit = self.config.parents[1] / "systemd/user/wn-agent-herdr.service"
        original = unit.read_bytes()
        calls = []
        interrupted = False
        def runner(argv):
            nonlocal interrupted
            calls.append(argv)
            if "--version" in argv:
                return f"wn-agent {setup.MDK_VERSION}"
            if argv == ["systemctl", "--user", "stop", "wn-agent-herdr.service"] and not interrupted:
                interrupted = True
                raise KeyboardInterrupt()
            return ""
        with patch.object(setup, "ensure_mdk", return_value=installer.connector_path(installer.version())):
            with self.assertRaises(KeyboardInterrupt):
                setup.upgrade_transport(setup.tomllib.loads(self.config.read_text()), self.config.parents[1], runner)
        self.assertEqual(unit.read_bytes(), original)
        self.assertEqual(calls[-2:], [["systemctl", "--user", "start", "wn-agent-herdr.service"], ["systemctl", "--user", "start", "herdr-white-noise-bridge.service"]])

    def test_fresh_setup_generates_private_portable_config_and_preserves_on_repeat(self):
        with self.mocks():
            result = setup.configure("npub1fixture", self.socket, self.runner)
            self.assertTrue(self.config.exists())
            self.assertEqual(self.config.stat().st_mode & 0o777, 0o600)
            config = setup.tomllib.loads(self.config.read_text())
            self.assertEqual(config["phone_npub"], "npub1fixture")
            self.assertEqual(config["herdr_socket"], str(self.socket.resolve()))
            self.assertEqual(config["account_id_hex"], "ab" * 32)
            unit = self.config.parents[1] / "systemd/user/herdr-white-noise-bridge.service"
            self.assertIn("%%", unit.read_text())
            self.assertIn("--config", unit.read_text())
            original = self.config.read_bytes()
            setup.configure("npub1fixture", self.socket, self.runner)
            self.assertEqual(self.config.read_bytes(), original)
            self.assertEqual(self.bootstrap_count, 1)
            self.assertNotIn("restart", [part for call in self.calls for part in call])

    def test_wrong_owner_cannot_change_an_existing_installation(self):
        with self.mocks():
            setup.configure("npub1fixture", self.socket, self.runner)
            original = self.config.read_bytes()
            with self.assertRaises(setup.SetupError):
                setup.configure("npub1different", self.socket, self.runner)
            self.assertEqual(self.config.read_bytes(), original)

    def test_enabling_phonon_on_an_existing_text_only_install_keeps_one_valid_command(self):
        with self.mocks():
            setup.configure("npub1fixture", self.socket, self.runner, enable_audio=False)
            setup.configure("npub1fixture", self.socket, self.runner, enable_audio=True)
        config = setup.tomllib.loads(self.config.read_text())
        self.assertEqual(config["phone_npub"], "npub1fixture")
        self.assertEqual(config["transcribe_command"][0], "/usr/bin/python3")
        self.assertEqual(self.bootstrap_count, 1)

    def test_invalid_owner_stops_before_services_or_identity_creation(self):
        def runner(argv):
            self.calls.append(argv)
            raise setup.SetupError("Invalid owner")
        with self.mocks():
            with self.assertRaises(setup.SetupError):
                setup.configure("npub1invalid", self.socket, runner)
        self.assertFalse(self.config.exists())
        self.assertFalse((self.home / ".marmot-agents/herdr").exists())
        self.assertTrue(all("systemctl" not in str(call) for call in self.calls))

    def test_reader_failure_stops_fresh_setup_before_transport_or_identity_creation(self):
        def runner(argv):
            if "check-readers" in argv:
                raise setup.SetupError("Session history check failed")
            return self.runner(argv)
        with self.mocks():
            with self.assertRaisesRegex(setup.SetupError, "history check"):
                setup.configure("npub1fixture", self.socket, runner)
        self.assertEqual(self.bootstrap_count, 0)
        self.assertFalse(self.config.exists())
        self.assertFalse(any(call[:3] in (["systemctl", "--user", "start"], ["systemctl", "--user", "stop"]) for call in self.calls))

    def test_existing_reader_failure_stops_before_connector_upgrade(self):
        with self.mocks():
            setup.configure("npub1fixture", self.socket, self.runner)
        self.calls.clear()
        def runner(argv):
            if "check-readers" in argv:
                raise setup.SetupError("Session history check failed")
            return self.runner(argv)
        with self.mocks(), patch.object(setup, "upgrade_transport") as upgrade:
            with self.assertRaisesRegex(setup.SetupError, "history check"):
                setup.configure("npub1fixture", self.socket, runner)
            upgrade.assert_not_called()

    def test_setup_check_reports_deferred_readers_without_changing_services_or_config(self):
        opencode = self.home / ".local/bin/opencode"
        opencode.write_text("fixture")
        opencode.chmod(0o700)
        def find(name, candidates=()):
            return str(opencode) if name == "opencode" else "/usr/bin/" + name
        with self.mocks(), patch.object(setup, "find_executable", side_effect=find):
            setup.configure("npub1fixture", self.socket, self.runner)
        before = self.config.read_bytes()
        self.calls.clear()
        with self.mocks(), patch.object(setup, "find_executable", side_effect=find), patch.object(setup, "upgrade_transport") as upgrade:
            result = setup.configure("npub1fixture", self.socket, self.runner, check=True)
            upgrade.assert_not_called()
        self.assertEqual(result["checked_session_readers"], 0)
        self.assertEqual(result["reader_validation"], "deferred: no live supported sessions")
        self.assertEqual(self.config.read_bytes(), before)
        self.assertFalse(any(call[0] in ("systemctl", "launchctl") or "bootstrap" in call for call in self.calls))

    def test_setup_reports_the_number_of_validated_live_readers(self):
        def runner(argv):
            if "check-readers" in argv:
                return json.dumps({"checked_sessions": 3})
            return self.runner(argv)
        with self.mocks():
            result = setup.configure("npub1fixture", self.socket, runner)
        self.assertEqual(result["checked_session_readers"], 3)
        self.assertEqual(result["reader_validation"], "passed")

    def test_resume_selects_existing_account_without_creating_another(self):
        with self.mocks(), patch.object(setup, "local_accounts", return_value=[{"label": "herdr-bridge", "account_id_hex": "ab" * 32, "local_signing": True}]):
            setup.configure("npub1fixture", self.socket, self.runner)
        bootstrap = next(call for call in self.calls if "bootstrap" in call)
        self.assertIn("--no-create", bootstrap)
        self.assertIn("--account-id-hex", bootstrap)

    def test_concurrent_setup_cannot_create_a_second_identity(self):
        with setup.setup_lock(self.config.parent), self.mocks():
            with self.assertRaises(setup.SetupError):
                setup.configure("npub1fixture", self.socket, self.runner)
        self.assertEqual(self.bootstrap_count, 0)

    def test_existing_install_cannot_silently_change_the_selected_server(self):
        with self.mocks():
            setup.configure("npub1fixture", self.socket, self.runner)
            with self.assertRaises(setup.SetupError):
                setup.configure("npub1fixture", self.home / "different.sock", self.runner)

    def test_unit_arguments_escape_literal_systemd_environment_references(self):
        self.assertEqual(setup.unit_argument("/config/${CODR_PROFILE}%"), '"/config/$${CODR_PROFILE}%%"')

    def test_retry_after_worker_start_failure_returns_the_same_bridge_identity(self):
        def fail_worker_start(argv):
            if list(argv) == ["systemctl", "--user", "start", "herdr-white-noise-bridge.service"]:
                raise setup.SetupError("Worker start failed")
            return self.runner(argv)
        with self.mocks():
            with self.assertRaises(setup.SetupError):
                setup.configure("npub1fixture", self.socket, fail_worker_start)
            self.assertTrue(self.config.exists())
            result = setup.configure("npub1fixture", self.socket, self.runner)
        self.assertEqual(result["bridge_npub"], "npub1bridge")
        self.assertEqual(self.bootstrap_count, 1)

    def test_codex_only_install_does_not_require_opencode(self):
        def find(name, candidates=()):
            if name in ("opencode", "claude"):
                raise setup.SetupError("Not installed")
            return "/usr/bin/" + name
        with self.mocks(), patch.object(setup, "find_executable", side_effect=find):
            setup.configure("npub1fixture", self.socket, self.runner)
            setup.configure("npub1fixture", self.socket, self.runner)
            setup.configure("npub1fixture", self.socket, self.runner, check=True)
            self.config.write_text(self.config.read_text().replace('opencode_bin = "opencode"', 'opencode_bin = "/removed/opencode"'))
            setup.configure("npub1fixture", self.socket, self.runner)
            setup.configure("npub1fixture", self.socket, self.runner, check=True)
        config = setup.tomllib.loads(self.config.read_text())
        self.assertEqual(config["opencode_bin"], "/removed/opencode")
        self.assertEqual(config["codex_home"], str((self.home / ".codex").resolve()))

    def test_claude_only_install_does_not_require_opencode_or_codex(self):
        def find(name, candidates=()):
            if name in ("opencode", "codex"):
                raise setup.SetupError("Not installed")
            return "/usr/bin/" + name
        with self.mocks(), patch.object(setup, "find_executable", side_effect=find):
            result = setup.configure("npub1fixture", self.socket, self.runner)
            setup.configure("npub1fixture", self.socket, self.runner)
            setup.configure("npub1fixture", self.socket, self.runner, check=True)
        config = setup.tomllib.loads(self.config.read_text())
        self.assertEqual(config["claude_home"], str((self.home / ".claude").resolve()))
        self.assertEqual(result["supported_agents"], ["claude"])

    def test_macos_setup_writes_launch_agents_and_uses_launchctl(self):
        agents = self.home / "LaunchAgents"
        with self.mocks(), patch.dict(os.environ, {"CODR_SERVICE_MANAGER": "launchd", "CODR_LAUNCH_AGENTS_DIR": str(agents)}), \
                patch.object(services, "_capture", return_value="state = running"):
            result = setup.configure("npub1fixture", self.socket, self.runner)
        self.assertEqual(result["bridge_npub"], "npub1bridge")
        self.assertTrue((agents / "io.github.tidley.codr.wn-agent-herdr.plist").exists())
        self.assertTrue((agents / "io.github.tidley.codr.herdr-white-noise-bridge.plist").exists())
        self.assertFalse((self.config.parents[1] / "systemd").exists())
        self.assertTrue(all("systemctl" not in " ".join(call) for call in self.calls))
        self.assertIn(["launchctl", "kickstart", f"gui/{os.getuid()}/io.github.tidley.codr.herdr-white-noise-bridge"], self.calls)
        plist = plistlib.loads((agents / "io.github.tidley.codr.herdr-white-noise-bridge.plist").read_bytes())
        self.assertIn("--config", plist["ProgramArguments"])
        self.assertEqual(plist["KeepAlive"], {"SuccessfulExit": False})

    def test_broken_unrelated_cli_does_not_block_an_observed_native_agent(self):
        def find(name, candidates=()):
            if name in ("codex", "claude"):
                raise setup.SetupError("Not on PATH")
            return "/usr/bin/" + name
        def runner(argv):
            if argv == ["/usr/bin/opencode", "--version"]:
                raise setup.SetupError("Broken unrelated installation")
            return self.runner(argv)
        with self.mocks(), patch.object(setup, "find_executable", side_effect=find), patch.object(setup,"read_snapshot", return_value={"agents":[{"agent":"claude","agent_session":{"agent":"claude"}}]}):
            result=setup.configure("npub1fixture", self.socket, runner)
        self.assertEqual(result["supported_agents"],["claude"])

    def test_launchd_transport_upgrade_preserves_arguments_and_rolls_back_running_jobs(self):
        for failure in (None, setup.SetupError, KeyboardInterrupt, "activation"):
            with self.subTest(failure=failure), patch.dict(os.environ, {"CODR_SERVICE_MANAGER": "launchd", "CODR_LAUNCH_AGENTS_DIR": str(self.home / "LaunchAgents")}):
                manager = services.LaunchdServices(self.runner, setup.private_write, self.config.parents[1])
                home = self.home / ".marmot-agents/herdr"
                argv = ["/usr/bin/wn-agent", "--home", str(home), "--socket", str(home / "dev/wn-agent.sock"), "--relay", "wss://fixture.invalid"]
                unit = manager.path("wn-agent-herdr")
                original = plistlib.dumps(manager.payload(services.Service("wn-agent-herdr", "fixture", argv, {"KEEP_ME": "yes"}))).decode()
                setup.private_write(unit, original)
                existing = {"wn_agent_socket": argv[4], "state_path": str(self.home / "state.json")}
                loaded = {"wn-agent-herdr": True, "herdr-white-noise-bridge": True}
                running = {"transport": original}
                failed = False
                def runner(args):
                    if "--version" in args:
                        return f"wn-agent {setup.MDK_VERSION}"
                    self.assertEqual(args[:2], ["launchctl", "bootout"])
                    name = args[2].split(services.LABEL_PREFIX + ".", 1)[1]
                    loaded[name] = False
                    if name == "wn-agent-herdr":
                        running["transport"] = None
                    return ""
                def start(name):
                    nonlocal failed
                    self.assertFalse(loaded[name])
                    loaded[name] = True
                    if name == "wn-agent-herdr":
                        running["transport"] = unit.read_text()
                    elif failure and failure != "activation" and not failed:
                        failed = True
                        raise failure("fixture interruption")
                binary = installer.connector_path(installer.version())
                with patch.object(setup, "ensure_mdk", return_value=binary), patch.object(setup.services, "for_host", return_value=manager), patch.object(manager, "loaded", side_effect=lambda name: loaded[name]), patch.object(manager, "domain", return_value="gui/123"), patch.object(manager, "start", side_effect=start), patch.object(manager, "is_active", side_effect=setup.SetupError("fixture activation failure") if failure == "activation" else None):
                    if failure:
                        with self.assertRaises(setup.SetupError if failure == "activation" else failure):
                            setup.upgrade_transport(existing, self.config.parents[1], runner)
                        self.assertEqual(unit.read_text(), original)
                        self.assertEqual(running["transport"], original)
                    else:
                        self.assertTrue(setup.upgrade_transport(existing, self.config.parents[1], runner))
                        payload = plistlib.loads(unit.read_bytes())
                        self.assertEqual(payload["ProgramArguments"], [str(binary), *argv[1:]])
                        self.assertEqual(payload["EnvironmentVariables"]["KEEP_ME"], "yes")
                        self.assertEqual(running["transport"], unit.read_text())
                    self.assertTrue(all(loaded.values()))

    def test_find_executable_keeps_the_path_entry_instead_of_the_symlink_target(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            real = root / "real-opencode"
            real.write_text("fixture")
            real.chmod(0o700)
            link = root / "opencode"
            link.symlink_to(real)
            with patch.dict(os.environ, {"PATH": str(root)}):
                self.assertEqual(setup.find_executable("opencode"), str(link))

    def test_existing_install_repairs_a_moved_opencode_executable(self):
        opencode = self.home / ".local/bin/opencode"
        opencode.write_text("fixture")
        opencode.chmod(0o700)
        def find(name, candidates=()):
            return str(opencode) if name == "opencode" else "/usr/bin/" + name
        with self.mocks(), patch.object(setup, "find_executable", side_effect=find):
            setup.configure("npub1fixture", self.socket, self.runner)
            self.assertEqual(setup.tomllib.loads(self.config.read_text())["opencode_bin"], str(opencode))
            text = self.config.read_text().replace(f'opencode_bin = "{opencode}"', 'opencode_bin = "/missing/opencode"')
            self.config.write_text(text)
            self.calls.clear()
            setup.configure("npub1fixture", self.socket, self.runner)
        self.assertEqual(setup.tomllib.loads(self.config.read_text())["opencode_bin"], str(opencode))
        self.assertIn(["systemctl", "--user", "restart", "herdr-white-noise-bridge.service"], self.calls)

    def test_check_reports_a_missing_opencode_executable_without_writing(self):
        with self.mocks():
            setup.configure("npub1fixture", self.socket, self.runner)
        text = self.config.read_text().replace('opencode_bin = "/usr/bin/opencode"', 'opencode_bin = "/missing/opencode"')
        self.config.write_text(text)
        with self.mocks(), self.assertRaises(setup.SetupError):
            setup.configure("npub1fixture", self.socket, self.runner, check=True)
        self.assertIn('"/missing/opencode"', self.config.read_text())

    def test_check_keeps_a_valid_opencode_path_command_without_repair(self):
        with self.mocks():
            setup.configure("npub1fixture", self.socket, self.runner)
        executable = self.home / ".local/bin/opencode"
        executable.write_text("fixture")
        executable.chmod(0o700)
        self.config.write_text(self.config.read_text().replace('opencode_bin = "/usr/bin/opencode"', 'opencode_bin = "opencode"'))
        before = self.config.read_bytes()
        self.calls.clear()
        with self.mocks(), patch.dict(os.environ, {"PATH": str(executable.parent)}), patch.object(setup, "find_executable") as find:
            setup.configure("npub1fixture", self.socket, self.runner, check=True)
            find.assert_not_called()
        self.assertEqual(self.config.read_bytes(), before)
        self.assertFalse(any(call[0] in ("systemctl", "launchctl") for call in self.calls))

    def test_failed_repair_reader_validation_preserves_config_and_retries_restart(self):
        executable = self.home / ".local/bin/opencode"
        executable.write_text("fixture")
        executable.chmod(0o700)
        with self.mocks():
            setup.configure("npub1fixture", self.socket, self.runner)
        self.config.write_text(self.config.read_text().replace('opencode_bin = "/usr/bin/opencode"', 'opencode_bin = "/missing/opencode"'))
        before = self.config.read_bytes()
        self.calls.clear()
        def find(name, candidates=()):
            return str(executable) if name == "opencode" else "/usr/bin/" + name
        def fail_reader(argv):
            if "check-readers" in argv:
                candidate = Path(argv[argv.index("--config") + 1])
                self.assertEqual(setup.tomllib.loads(candidate.read_text())["opencode_bin"], str(executable))
                raise setup.SetupError("Reader unavailable")
            return self.runner(argv)
        with self.mocks(), patch.object(setup, "find_executable", side_effect=find), patch.object(setup, "upgrade_transport") as upgrade:
            with self.assertRaisesRegex(setup.SetupError, "history check"):
                setup.configure("npub1fixture", self.socket, fail_reader)
            self.assertEqual(self.config.read_bytes(), before)
            self.assertFalse(any(call[0] in ("systemctl", "launchctl") for call in self.calls))
            upgrade.assert_not_called()
            setup.configure("npub1fixture", self.socket, self.runner)
        self.assertEqual(setup.tomllib.loads(self.config.read_text())["opencode_bin"], str(executable))
        self.assertIn(["systemctl", "--user", "restart", "herdr-white-noise-bridge.service"], self.calls)

    def test_failed_repair_restart_restores_config_so_retry_restarts_again(self):
        executable = self.home / ".local/bin/opencode"
        executable.write_text("fixture")
        executable.chmod(0o700)
        with self.mocks():
            setup.configure("npub1fixture", self.socket, self.runner)
        self.config.write_text(self.config.read_text().replace('opencode_bin = "/usr/bin/opencode"', 'opencode_bin = "/missing/opencode"'))
        before = self.config.read_bytes()
        restart = ["systemctl", "--user", "restart", "herdr-white-noise-bridge.service"]
        def runner(argv):
            if argv == restart:
                raise setup.SetupError("Restart failed")
            return self.runner(argv)
        with self.mocks(), patch.object(setup, "find_executable", return_value=str(executable)), patch.object(setup, "upgrade_transport"):
            with self.assertRaisesRegex(setup.SetupError, "Restart failed"):
                setup.configure("npub1fixture", self.socket, runner)
            self.assertEqual(self.config.read_bytes(), before)
            self.calls.clear()
            setup.configure("npub1fixture", self.socket, self.runner)
        self.assertIn(restart, self.calls)

    def test_unavailable_opencode_still_fails_a_required_live_reader(self):
        with self.mocks():
            setup.configure("npub1fixture", self.socket, self.runner)
        self.config.write_text(self.config.read_text().replace('opencode_bin = "/usr/bin/opencode"', 'opencode_bin = "/missing/opencode"'))
        before = self.config.read_bytes()
        self.calls.clear()
        def runner(argv):
            if "check-readers" in argv:
                raise setup.SetupError("OpenCode reader unavailable")
            return self.runner(argv)
        with self.mocks(), patch.object(setup, "find_executable", side_effect=setup.SetupError("Not installed")), patch.object(setup, "upgrade_transport") as upgrade:
            with self.assertRaisesRegex(setup.SetupError, "history check"):
                setup.configure("npub1fixture", self.socket, runner)
            upgrade.assert_not_called()
        self.assertEqual(self.config.read_bytes(), before)
        self.assertFalse(any(call[0] in ("systemctl", "launchctl") for call in self.calls))


class OpenRigSetupTests(unittest.TestCase):
    def install_home(self, manager):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.home = Path(temporary.name).resolve()
        self.agents = self.home / "Library/LaunchAgents"
        environment = patch.dict(os.environ, {"HOME": str(self.home), "XDG_CONFIG_HOME": str(self.home / "config"),
            "XDG_STATE_HOME": str(self.home / "state"), "CODR_SERVICE_MANAGER": manager,
            "CODR_LAUNCH_AGENTS_DIR": str(self.agents)}, clear=True)
        environment.start()
        self.addCleanup(environment.stop)
        bridge = self.home / ".local/bin/herdr-white-noise-bridge"
        bridge.parent.mkdir(parents=True)
        bridge.write_text("fixture")
        self.rig = self.home / "node/lib/node_modules/@openrig/cli/dist/bin-wrapper.js"
        self.rig.parent.mkdir(parents=True)
        self.rig.write_text("#!/usr/bin/env node\n")
        (self.home / "seat-bin").mkdir()
        (self.home / "seat-bin/rig").symlink_to(self.rig)
        self.config = self.home / "config/codr-openrig/config.toml"
        self.calls = []
        self.bootstrap_count = 0
        self.herdr = [self.home / "config/herdr-white-noise-bridge/config.toml",
                      self.home / "state/herdr-white-noise-bridge/state.json",
                      self.home / ".marmot-agents/herdr/codr-bootstrap.json",
                      self.agents / "io.github.tidley.codr.wn-agent-herdr.plist",
                      self.agents / "io.github.tidley.codr.herdr-white-noise-bridge.plist",
                      self.home / "config/systemd/user/wn-agent-herdr.service",
                      self.home / "config/systemd/user/herdr-white-noise-bridge.service"]
        for path in self.herdr:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(f"herdr fixture {path.name}")
            path.chmod(0)
        self.locked = {path.parent for path in self.herdr[:3]}
        for directory in self.locked:
            directory.chmod(0)
            self.addCleanup(directory.chmod, 0o700)

    def runner(self, argv):
        argv = [str(arg) for arg in argv]
        self.calls.append(argv)
        if "bootstrap" in argv:
            self.bootstrap_count += 1
            return json.dumps({"account_id_hex": "cd" * 32, "npub": "npub1openrig", "label": "openrig-bridge"})
        if "--version" in argv:
            return f"wn-agent {setup.MDK_VERSION}"
        if "check-readers" in argv:
            return json.dumps({"checked_sessions": 1})
        if "process.execPath" in argv:
            return "/opt/node/22/bin/node\n"
        return ""

    def mocks(self):
        def find(name, candidates=()):
            return str(self.home / "seat-bin/rig") if name == "rig" else "/usr/bin/" + name
        return patch.multiple(setup, find_executable=find, ensure_mdk=lambda runner: Path("/usr/bin/wn-agent"),
            local_accounts=lambda path: [])

    def assert_herdr_untouched(self):
        for directory in self.locked:
            self.assertEqual(directory.stat().st_mode & 0o777, 0, directory)
            directory.chmod(0o700)
        for path in self.herdr:
            self.assertEqual(path.stat().st_mode & 0o777, 0, path)
            path.chmod(0o600)
            self.assertEqual(path.read_text(), f"herdr fixture {path.name}")
        for call in self.calls:
            text = " ".join(call)
            for name in ("wn-agent-herdr", "herdr-white-noise-bridge.service", "codr.herdr-white-noise-bridge", ".marmot-agents/herdr"):
                self.assertNotIn(name, text)

    def test_openrig_setup_installs_its_own_bridge_and_reruns_keep_its_identity(self):
        for manager in (services.LAUNCHD, services.SYSTEMD):
            with self.subTest(manager=manager):
                self.install_home(manager)
                with self.mocks(), patch.object(services, "_capture", return_value="state = running"):
                    result = setup.configure("npub1fixture", runner=self.runner, seats=["operator-agent@kernel"])
                    config = setup.tomllib.loads(self.config.read_text())
                    self.assertEqual(config["backend"], "openrig")
                    self.assertEqual(config["openrig_seats"], ["operator-agent@kernel"])
                    self.assertIs(config["phone_replies_only"], True)
                    self.assertEqual(config["rig_bin"], str(self.rig.resolve()))
                    self.assertEqual(config["account_id_hex"], "cd" * 32)
                    self.assertEqual(config["wn_agent_socket"], str(self.home / ".marmot-agents/openrig/dev/wn-agent.sock"))
                    self.assertEqual(config["state_path"], str(self.home / "state/codr-openrig/state.json"))
                    self.assertEqual(self.config.stat().st_mode & 0o777, 0o600)
                    self.assertEqual(result["bridge_npub"], "npub1openrig")
                    if manager == services.LAUNCHD:
                        plist = plistlib.loads((self.agents / "io.github.tidley.codr.openrig-bridge.plist").read_bytes())
                        self.assertEqual(plist["ProgramArguments"][1:], ["--config", str(self.config), "run"])
                        self.assertTrue(plist["EnvironmentVariables"]["PATH"].startswith("/opt/node/22/bin" + os.pathsep))
                        self.assertEqual(plist["StandardErrorPath"], str(self.home / "Library/Logs/codr/openrig-bridge.err.log"))
                        self.assertTrue((self.agents / "io.github.tidley.codr.wn-agent-openrig.plist").exists())
                    else:
                        unit = (self.home / "config/systemd/user/openrig-bridge.service").read_text()
                        self.assertIn('Environment="PATH=/opt/node/22/bin' + os.pathsep, unit)
                        self.assertIn("Requires=wn-agent-openrig.service", unit)
                        self.assertIn(".marmot-agents/openrig", (self.home / "config/systemd/user/wn-agent-openrig.service").read_text())
                    original = self.config.read_bytes()
                    again = setup.configure(None, runner=self.runner, openrig=True)
                    self.assertEqual(self.config.read_bytes(), original)
                    self.assertEqual(again["bridge_npub"], "npub1openrig")
                    self.assertNotIn("restart", " ".join(" ".join(call) for call in self.calls))
                    self.assertNotIn("-k", [part for call in self.calls for part in call])
                    moved = setup.configure("npub1fixture", runner=self.runner, seats=["operator-agent@kernel", "dev-build@rig"])
                    self.assertEqual(setup.tomllib.loads(self.config.read_text())["openrig_seats"], ["operator-agent@kernel", "dev-build@rig"])
                    self.assertEqual(moved["openrig_seats"], ["operator-agent@kernel", "dev-build@rig"])
                    self.assertEqual(setup.tomllib.loads(self.config.read_text())["account_id_hex"], "cd" * 32)
                    restart = (["systemctl", "--user", "restart", "openrig-bridge.service"] if manager == services.SYSTEMD
                               else ["launchctl", "kickstart", "-k", f"gui/{os.getuid()}/io.github.tidley.codr.openrig-bridge"])
                    self.assertIn(restart, self.calls)
                    setup.configure(None, runner=self.runner, openrig=True, check=True)
                self.assertEqual(self.bootstrap_count, 1)
                self.assertNotIn("nsec", json.dumps([result, again, moved]))
                self.assert_herdr_untouched()

    def test_failed_seat_change_restart_restores_config_so_retry_restarts_again(self):
        self.install_home(services.SYSTEMD)
        restart = ["systemctl", "--user", "restart", "openrig-bridge.service"]
        def runner(argv):
            if [str(arg) for arg in argv] == restart:
                raise setup.SetupError("Restart failed")
            return self.runner(argv)
        with self.mocks(), patch.object(services, "_capture", return_value="state = running"):
            setup.configure("npub1fixture", runner=self.runner, seats=["a@kernel"])
            before = self.config.read_bytes()
            with self.assertRaisesRegex(setup.SetupError, "Restart failed"):
                setup.configure(None, runner=runner, seats=["a@kernel", "b@rig"])
            self.assertEqual(self.config.read_bytes(), before)
            self.calls.clear()
            setup.configure(None, runner=self.runner, seats=["a@kernel", "b@rig"])
        self.assertIn(restart, self.calls)
        self.assertEqual(setup.tomllib.loads(self.config.read_text())["openrig_seats"], ["a@kernel", "b@rig"])
        self.assert_herdr_untouched()

    def test_openrig_setup_requires_a_seat_and_keeps_the_herdr_options_out(self):
        self.install_home(services.LAUNCHD)
        with self.mocks():
            with self.assertRaisesRegex(setup.SetupError, "--seat"):
                setup.configure("npub1fixture", runner=self.runner, openrig=True)
            with self.assertRaises(setup.SetupError):
                setup.configure("npub1fixture", self.home / "herdr.sock", self.runner, seats=["a@b"])
            with self.assertRaises(setup.SetupError):
                setup.configure("npub1fixture", runner=self.runner, enable_audio=True, seats=["a@b"])
        self.assertFalse(self.config.exists())
        self.assertEqual(self.bootstrap_count, 0)
        self.assert_herdr_untouched()

    def test_systemd_units_keep_a_template_path_and_add_a_missing_one(self):
        with tempfile.TemporaryDirectory() as temporary:
            manager = services.SystemdServices(lambda argv: "", setup.private_write, Path(temporary))
            herdr = manager.install(services.Service("herdr-white-noise-bridge", "fixture", ["/bin/bridge", "run"], environment={"PATH": "/custom"}))
            self.assertNotIn("/custom", herdr.read_text())
            self.assertIn("Environment=PATH=%h/.local/bin", herdr.read_text())
            openrig = manager.install(services.Service("openrig-bridge", "fixture", ["/bin/bridge", "run"], environment={"PATH": "/node/bin:/usr/bin"}))
            self.assertIn('ExecStart="/bin/bridge" "run"\nEnvironment="PATH=/node/bin:/usr/bin"\n', openrig.read_text())


if __name__ == "__main__":
    unittest.main()
