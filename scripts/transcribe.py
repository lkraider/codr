#!/usr/bin/env python3
"""Convert a downloaded voice message and send it to the private Phonon worker."""
import argparse
from contextlib import closing
import http.client
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import uuid
import wave


class UnixHTTPConnection(http.client.HTTPConnection):
    def __init__(self, path, timeout=120):
        super().__init__("localhost", timeout=timeout)
        self.path = str(path)

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(self.timeout)
        self.sock.connect(self.path)


def normalise(audio, ffmpeg, directory):
    wav = Path(directory) / "audio.wav"
    subprocess.run([ffmpeg, "-nostdin", "-v", "error", "-y", "-i", str(audio),
                    "-t", "301", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", str(wav)],
                   stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                   check=True, timeout=45)
    with wave.open(str(wav), "rb") as recording:
        duration = recording.getnframes() / recording.getframerate()
        if duration <= 0 or duration > 300:
            raise ValueError("Audio must contain at most five minutes of speech")
    return wav


def transcribe(audio, socket_path, ffmpeg):
    with tempfile.TemporaryDirectory(prefix="codr-audio-") as temporary:
        wav = normalise(audio, ffmpeg, temporary)
        boundary = "codr-" + uuid.uuid4().hex
        body = (f'--{boundary}\r\nContent-Disposition: form-data; name="model"\r\n\r\nphonon-2\r\n'
                f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="audio.wav"\r\n'
                'Content-Type: audio/wav\r\n\r\n').encode() + wav.read_bytes() + f'\r\n--{boundary}--\r\n'.encode()
        with closing(UnixHTTPConnection(socket_path)) as connection:
            for attempt in range(40):
                try:
                    connection.connect()
                    break
                except (FileNotFoundError, ConnectionRefusedError):
                    connection.close()
                    if attempt == 39:
                        raise
                    time.sleep(0.25)
            connection.request("POST", "/v1/audio/transcriptions", body,
                               {"Content-Type": f"multipart/form-data; boundary={boundary}"})
            response = connection.getresponse()
            if response.status != 200:
                raise ValueError("The local speech worker refused the recording")
            payload = response.read(65537)
            if len(payload) > 65536:
                raise ValueError("The transcript is too large")
            result = json.loads(payload)
            text = result.get("text")
            if not isinstance(text, str) or not text.strip():
                raise ValueError("The speech worker returned no text")
            return text.strip()


if __name__ == "__main__":
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", required=True, type=Path)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("audio", type=Path)
    args = parser.parse_args()
    try:
        print(transcribe(args.audio, args.socket, args.ffmpeg))
    except (OSError, ValueError, subprocess.SubprocessError, http.client.HTTPException):
        raise SystemExit("Local audio transcription failed")
