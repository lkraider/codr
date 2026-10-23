#!/usr/bin/env python3
"""Convert a downloaded voice message and send it to the Gemini API for transcription."""
import argparse
import base64
import json
import os
from pathlib import Path
import stat
import tempfile
import urllib.error
import urllib.request

from transcribe import normalise

ENDPOINT = "https://generativelanguage.googleapis.com/v1beta/interactions"
MODEL = "gemini-3.5-transcribe"


class TranscriptionError(Exception):
    pass


def key_path():
    return Path.home() / ".config/codr/gemini-api-key"


def read_key(path):
    try:
        status = os.stat(path)
    except FileNotFoundError:
        raise TranscriptionError(f"The Gemini key file is missing: {path}") from None
    if not stat.S_ISREG(status.st_mode):
        raise TranscriptionError(f"The Gemini key file is not a regular file: {path}")
    if status.st_mode & 0o077:
        raise TranscriptionError(f"The Gemini key file allows group or other access; run chmod 600 {path}")
    try:
        key = path.read_text().strip()
    except (OSError, UnicodeError):
        raise TranscriptionError(f"The Gemini key file cannot be read: {path}") from None
    if not key:
        raise TranscriptionError(f"The Gemini key file is empty: {path}")
    return key


def transcript(payload):
    try:
        result = json.loads(payload)
        parts = [item["text"] for step in result.get("steps", []) if step.get("type") == "model_output"
                 for item in step.get("content", []) if item.get("type") == "text"]
    except (ValueError, TypeError, KeyError, AttributeError):
        raise TranscriptionError("Gemini returned an unreadable response") from None
    text = "".join(part for part in parts if isinstance(part, str)).strip()
    if not text:
        raise TranscriptionError("Gemini returned no text")
    if len(text.encode()) > 65536:
        raise TranscriptionError("The transcript is too large")
    return text


def transcribe(audio, ffmpeg, endpoint=ENDPOINT):
    key = read_key(key_path())
    with tempfile.TemporaryDirectory(prefix="codr-audio-") as temporary:
        wav = normalise(audio, ffmpeg, temporary)
        body = json.dumps({"model": MODEL, "input": [{
            "type": "audio", "mime_type": "audio/wav",
            "data": base64.b64encode(wav.read_bytes()).decode()}]}).encode()
    request = urllib.request.Request(endpoint, body, method="POST", headers={
        "Content-Type": "application/json", "x-goog-api-key": key})
    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            payload = response.read(1048577)
    except urllib.error.HTTPError as error:
        error.close()
        raise TranscriptionError(f"Gemini refused the recording with HTTP {error.code}") from None
    except (OSError, ValueError):
        raise TranscriptionError("Gemini could not be reached") from None
    if len(payload) > 1048576:
        raise TranscriptionError("The Gemini response is too large")
    return transcript(payload)


if __name__ == "__main__":
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("audio", type=Path)
    args = parser.parse_args()
    try:
        print(transcribe(args.audio, args.ffmpeg))
    except TranscriptionError as error:
        raise SystemExit(str(error))
    except ValueError as error:
        raise SystemExit(str(error))
    except Exception:
        raise SystemExit("Audio conversion failed")
