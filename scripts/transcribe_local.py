#!/usr/bin/env python3
"""Convert a downloaded voice message and transcribe it on this machine with faster-whisper."""
import argparse
import os
from pathlib import Path
import tempfile

from transcribe import normalise

MODEL = "medium"
# No default hint: the transcriber stays generic. The user may pass one with --hint.
HINT = ""


class TranscriptionError(Exception):
    pass


def load_model(name, directory=None):
    try:
        from faster_whisper import WhisperModel
    except ImportError:
        raise TranscriptionError("faster-whisper is not installed in this Python; see the local transcription setup") from None
    try:
        return WhisperModel(name, device="cpu", compute_type="int8", download_root=directory)
    except Exception:
        raise TranscriptionError(f"The {name} speech model could not be loaded; the first run needs network access to download it") from None


def transcribe(audio, ffmpeg, model=MODEL, directory=None, loader=load_model, hint=HINT):
    with tempfile.TemporaryDirectory(prefix="codr-audio-") as temporary:
        wav = normalise(audio, ffmpeg, temporary)
        # Load only after the file converted and fits the limit, so a bad file fails fast.
        engine = loader(model, directory)
        try:
            # The language is detected from the audio. Segments are generated lazily.
            segments, _ = engine.transcribe(str(wav), beam_size=1, vad_filter=True,
                                         initial_prompt=hint or None)
            text = " ".join(segment.text.strip() for segment in segments).strip()
        except Exception:
            raise TranscriptionError("The local speech model failed on the recording") from None
    if not text:
        raise TranscriptionError("The local speech model returned no text")
    if len(text.encode()) > 65536:
        raise TranscriptionError("The transcript is too large")
    return text


if __name__ == "__main__":
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--model", default=MODEL, help="faster-whisper model name, such as medium or small")
    parser.add_argument("--hint", default=HINT, help="optional text that tells the model about the language, such as the language and the kind of speech; none by default")
    parser.add_argument("--model-dir", help="where the model weights are kept")
    parser.add_argument("audio", type=Path)
    args = parser.parse_args()
    try:
        print(transcribe(args.audio, args.ffmpeg, args.model, args.model_dir, hint=args.hint))
    except TranscriptionError as error:
        raise SystemExit(str(error))
    except ValueError as error:
        raise SystemExit(str(error))
    except Exception:
        raise SystemExit("Audio conversion failed")
