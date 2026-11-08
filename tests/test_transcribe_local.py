from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch
import wave

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))

import transcribe_local


def write_wav(audio, ffmpeg, directory, seconds=1):
    wav = Path(directory) / "audio.wav"
    with wave.open(str(wav), "wb") as output:
        output.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
        output.writeframes(b"\0\0" * 16000 * seconds)
    return wav


class Segment:
    def __init__(self, text):
        self.text = text


class FakeModel:
    def __init__(self, texts=(" Olá, ", "deploy the build "), fail=False):
        self.texts, self.fail, self.calls = texts, fail, []

    def transcribe(self, path, **options):
        self.calls.append((path, options))
        if self.fail:
            raise RuntimeError("secret internal detail")
        return (Segment(text) for text in self.texts), object()


def run(engine, **arguments):
    loaded = []

    def loader(name, directory):
        loaded.append((name, directory))
        return engine

    with patch.object(transcribe_local, "normalise", write_wav):
        return transcribe_local.transcribe(Path("voice.m4a"), "ffmpeg", loader=loader, **arguments), loaded


class LocalTranscriptionTests(unittest.TestCase):
    def test_segments_are_joined_and_the_language_is_left_to_the_model(self):
        model = FakeModel()
        text, loaded = run(model)
        self.assertEqual(text, "Olá, deploy the build")
        self.assertEqual(loaded, [("medium", None)])
        path, options = model.calls[0]
        self.assertTrue(path.endswith("audio.wav"))
        self.assertNotIn("language", options)
        self.assertEqual(options["beam_size"], 1)

    def test_there_is_no_hint_unless_the_user_gives_one(self):
        model = FakeModel()
        run(model)
        self.assertIsNone(model.calls[0][1]["initial_prompt"])
        self.assertEqual(transcribe_local.HINT, "")
        run(model, hint="Transcrição em português.")
        self.assertEqual(model.calls[1][1]["initial_prompt"], "Transcrição em português.")

    def test_the_model_and_its_directory_are_configurable(self):
        _, loaded = run(FakeModel(), model="small", directory="/models")
        self.assertEqual(loaded, [("small", "/models")])

    def test_no_speech_is_an_error(self):
        with self.assertRaisesRegex(transcribe_local.TranscriptionError, "no text"):
            run(FakeModel(texts=(" ", "")))

    def test_a_model_failure_is_one_line_without_its_detail(self):
        with self.assertRaises(transcribe_local.TranscriptionError) as caught:
            run(FakeModel(fail=True))
        self.assertNotIn("secret", str(caught.exception))

    def test_a_huge_transcript_is_refused(self):
        with self.assertRaisesRegex(transcribe_local.TranscriptionError, "too large"):
            run(FakeModel(texts=("x" * 65537,)))

    def test_a_missing_package_says_how_to_set_up(self):
        with patch.dict(sys.modules, {"faster_whisper": None}):
            with self.assertRaisesRegex(transcribe_local.TranscriptionError, "not installed"):
                transcribe_local.load_model("medium")

    def test_audio_over_five_minutes_is_refused_by_the_shared_conversion(self):
        def fake_run(command, **kwargs):
            write_wav(None, None, Path(command[-1]).parent, seconds=301)

        loaded = []
        with patch.object(subprocess, "run", fake_run):
            with self.assertRaisesRegex(ValueError, "five minutes"):
                transcribe_local.transcribe(Path("v.m4a"), "ffmpeg", loader=lambda *a: loaded.append(a))
        self.assertEqual(loaded, [], "a refused file must not load the model")


if __name__ == "__main__":
    unittest.main()
