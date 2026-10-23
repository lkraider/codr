import base64
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch
import wave

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))

import transcribe_gemini

KEY = "test-key-123"


def write_wav(audio, ffmpeg, directory):
    wav = Path(directory) / "audio.wav"
    with wave.open(str(wav), "wb") as output:
        output.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
        output.writeframes(b"\0\0" * 16000)
    return wav


class FakeGemini:
    def __init__(self, status=200, response=None):
        self.requests = []
        owner = self
        body = json.dumps(response).encode() if response is not None else b""

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                payload = self.rfile.read(int(self.headers["Content-Length"]))
                owner.requests.append((self.path, self.headers, payload))
                self.send_response(status)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *args):
                pass

        self.server = HTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    @property
    def url(self):
        return f"http://127.0.0.1:{self.server.server_port}/v1beta/interactions"

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *args):
        self.server.shutdown()
        self.server.server_close()


def answer(*texts):
    return {"id": "interactions/1", "status": "completed", "steps": [
        {"type": "user_input", "content": [{"type": "text", "text": "ignored"}]},
        {"type": "model_output", "content": [{"type": "text", "text": text} for text in texts]}]}


class GeminiTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.home = Path(temporary.name)
        self.audio = self.home / "voice.m4a"
        self.audio.write_bytes(b"fixture")
        self.key = self.home / ".config/codr/gemini-api-key"
        self.key.parent.mkdir(parents=True)
        self.key.write_text(KEY + "\n")
        self.key.chmod(0o600)
        environment = patch.dict("os.environ", {"HOME": str(self.home)})
        environment.start()
        self.addCleanup(environment.stop)

    def run_transcribe(self, server):
        with patch.object(transcribe_gemini, "normalise", side_effect=write_wav):
            return transcribe_gemini.transcribe(self.audio, "ffmpeg", server.url)

    def test_wav_audio_goes_inline_with_the_key_in_the_header_and_returns_plain_text(self):
        with FakeGemini(response=answer("  Abra o arquivo ", "de configuração.\n")) as server:
            self.assertEqual(self.run_transcribe(server), "Abra o arquivo de configuração.")
        path, headers, payload = server.requests[0]
        self.assertEqual(path, "/v1beta/interactions")
        self.assertEqual(headers["x-goog-api-key"], KEY)
        self.assertEqual(headers["Content-Type"], "application/json")
        self.assertNotIn(KEY.encode(), payload)
        body = json.loads(payload)
        self.assertEqual(body["model"], "gemini-3.5-transcribe")
        self.assertEqual(len(body["input"]), 1)
        audio = body["input"][0]
        self.assertEqual((audio["type"], audio["mime_type"]), ("audio", "audio/wav"))
        self.assertEqual(base64.b64decode(audio["data"])[:4], b"RIFF")

    def test_a_key_file_with_group_or_other_access_is_refused_before_any_request(self):
        for mode in (0o640, 0o604, 0o620):
            self.key.chmod(mode)
            with FakeGemini(response=answer("text")) as server:
                with self.assertRaisesRegex(transcribe_gemini.TranscriptionError, "chmod 600"):
                    self.run_transcribe(server)
            self.assertEqual(server.requests, [])

    def test_a_missing_or_empty_key_file_is_refused_before_any_request(self):
        self.key.write_text("\n")
        with FakeGemini(response=answer("text")) as server:
            with self.assertRaisesRegex(transcribe_gemini.TranscriptionError, "empty"):
                self.run_transcribe(server)
            self.key.unlink()
            with self.assertRaisesRegex(transcribe_gemini.TranscriptionError, "missing"):
                self.run_transcribe(server)
        self.assertEqual(server.requests, [])

    def test_an_http_error_reports_the_status_without_the_key_or_the_body(self):
        with FakeGemini(status=403, response={"error": {"message": f"API key {KEY} not valid"}}) as server:
            with self.assertRaises(transcribe_gemini.TranscriptionError) as raised:
                self.run_transcribe(server)
        self.assertEqual(str(raised.exception), "Gemini refused the recording with HTTP 403")
        self.assertIsNone(raised.exception.__cause__)

    def test_an_empty_or_unreadable_transcript_is_an_error(self):
        for response, message in ((answer("  \n"), "no text"), (answer(), "no text"),
                                  ({"status": "completed"}, "no text"), ([], "unreadable"),
                                  ({"steps": "text"}, "unreadable")):
            with FakeGemini(response=response) as server:
                with self.assertRaisesRegex(transcribe_gemini.TranscriptionError, message):
                    self.run_transcribe(server)

    def test_an_unreachable_endpoint_is_an_error(self):
        with FakeGemini() as server:
            url = server.url
        with patch.object(transcribe_gemini, "normalise", side_effect=write_wav):
            with self.assertRaisesRegex(transcribe_gemini.TranscriptionError, "could not be reached"):
                transcribe_gemini.transcribe(self.audio, "ffmpeg", url)

    def test_the_command_exits_non_zero_with_a_short_message_and_no_stdout(self):
        self.key.chmod(0o644)
        result = subprocess.run([sys.executable, str(SCRIPTS / "transcribe_gemini.py"), str(self.audio)],
                                capture_output=True, text=True, timeout=30, env={**os.environ, "HOME": str(self.home)})
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        self.assertIn("allows group or other access", result.stderr)
        self.assertNotIn(KEY, result.stderr)


if __name__ == "__main__":
    unittest.main()
