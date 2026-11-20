"""The job API for long meeting recordings (Nary batch 3), with the model and the decoder replaced."""
import os
import threading
import time
from types import SimpleNamespace

os.environ.setdefault("STT_SERVICE_TOKEN", "test-token")

import numpy as np
import pytest
from fastapi.testclient import TestClient

from app import main

AUTH = {"Authorization": "Bearer test-token"}
RATE = main.SAMPLE_RATE


class FakeModel:
    """Two segments per chunk; remembers the language each chunk was transcribed with."""

    def __init__(self, detected="fr"):
        self.detected = detected
        self.calls = []

    def transcribe(self, chunk, language=None, beam_size=1, vad_filter=False):
        assert vad_filter is True
        self.calls.append((len(chunk), language))
        n = len(self.calls)
        seconds = len(chunk) / RATE
        segs = [SimpleNamespace(text=f" part {n}a ", end=seconds / 2),
                SimpleNamespace(text=f" part {n}b", end=seconds)]
        return iter(segs), SimpleNamespace(language=language or self.detected, duration=seconds)


@pytest.fixture
def client(monkeypatch, tmp_path):
    model = FakeModel()
    monkeypatch.setattr(main, "TOKEN", "test-token")
    monkeypatch.setattr(main, "JOB_DIR", str(tmp_path / "jobs"))
    monkeypatch.setattr(main, "load_model", lambda: model)
    monkeypatch.setattr(main, "probe_seconds", lambda path: 30.0)
    # three chunks of 10 s of silence: 30 s in all
    monkeypatch.setattr(main, "iter_chunks", lambda path, secs: (np.zeros(RATE * 10, dtype=np.float32) for _ in range(3)))
    with TestClient(main.app) as c:
        c.model = model
        yield c
    main._jobs.clear()


def wait_for(c, job_id, statuses=("done", "failed"), timeout=5.0):
    end = time.time() + timeout
    while time.time() < end:
        r = c.get(f"/jobs/{job_id}", headers=AUTH)
        assert r.status_code == 200
        if r.json()["status"] in statuses:
            return r.json()
        time.sleep(0.02)
    raise AssertionError(f"job {job_id} did not finish: {r.json()}")


def submit(c, data=b"RIFF....", name="meeting.m4a", **form):
    return c.post("/jobs", headers=AUTH, files={"audio": (name, data, "audio/mp4")}, data=form)


def test_jobs_need_the_token(client):
    assert client.post("/jobs", files={"audio": ("m.mp3", b"ID3", "audio/mpeg")}).status_code == 401
    assert client.get("/jobs/abc").status_code == 401


def test_a_job_runs_to_done_with_text_language_duration_and_its_file_is_deleted(client, tmp_path):
    r = submit(client, language="fr")
    assert r.status_code == 202
    job_id = r.json()["job_id"]
    out = wait_for(client, job_id)
    assert out == {"job_id": job_id, "status": "done", "progress": 100,
                   "text": "part 1a\npart 1b\npart 2a\npart 2b\npart 3a\npart 3b",
                   "language": "fr", "duration": 30.0, "error": None}
    assert os.listdir(tmp_path / "jobs") == []
    assert [lang for _, lang in client.model.calls] == ["fr", "fr", "fr"]


def test_the_language_detected_on_the_first_chunk_is_kept(client):
    out = wait_for(client, submit(client).json()["job_id"])
    assert out["language"] == "fr"
    assert [lang for _, lang in client.model.calls] == [None, "fr", "fr"]


def test_progress_is_seconds_processed_over_the_length(client, monkeypatch):
    seen = []
    gate = threading.Event()

    def slow_chunks(path, secs):
        for i in range(3):
            if i == 1:
                gate.wait(5)
            yield np.zeros(RATE * 10, dtype=np.float32)

    monkeypatch.setattr(main, "iter_chunks", slow_chunks)
    job_id = submit(client).json()["job_id"]
    out = wait_for(client, job_id, statuses=("transcribing",))
    end = time.time() + 5
    while time.time() < end:
        out = client.get(f"/jobs/{job_id}", headers=AUTH).json()
        if out["progress"] >= 33:
            break
        time.sleep(0.02)
    seen.append(out["progress"])
    assert out["status"] == "transcribing" and out["text"] is None
    assert seen[0] == 33
    gate.set()
    assert wait_for(client, job_id)["progress"] == 100


def test_one_job_at_a_time_in_order(client, monkeypatch):
    running = []
    peak = []
    lock = threading.Lock()

    def tracked(path, language, limit, on_progress):
        with lock:
            running.append(path)
            peak.append(len(running))
        time.sleep(0.05)
        with lock:
            running.remove(path)
        return {"text": os.path.basename(path), "language": "en", "duration": 1.0}

    monkeypatch.setattr(main, "transcribe_file", tracked)
    ids = [submit(client).json()["job_id"] for _ in range(3)]
    outs = [wait_for(client, i) for i in ids]
    assert max(peak) == 1
    assert [o["status"] for o in outs] == ["done"] * 3


def test_too_long_is_refused_at_upload(client, monkeypatch, tmp_path):
    monkeypatch.setattr(main, "probe_seconds", lambda path: 5 * 3600.0)
    r = submit(client, max_seconds="14400")
    assert r.status_code == 413
    assert "300 minutes" in r.json()["detail"] and "240 minutes" in r.json()["detail"]
    assert os.listdir(tmp_path / "jobs") == []


def test_too_long_after_decoding_fails_the_job(client, monkeypatch):
    monkeypatch.setattr(main, "probe_seconds", lambda path: None)  # length unknown up front
    out = wait_for(client, submit(client, max_seconds="15").json()["job_id"])
    assert out["status"] == "failed" and out["error"].startswith("The recording is longer than")


def test_too_large_and_empty_are_refused(client, monkeypatch, tmp_path):
    monkeypatch.setattr(main, "JOB_MAX_BYTES", 1024)
    r = submit(client, data=b"x" * 5000)
    assert r.status_code == 413
    assert submit(client, data=b"").status_code == 422
    assert os.listdir(tmp_path / "jobs") == []


def test_an_unreadable_recording_fails_with_a_reason(client, monkeypatch):
    def broken(path, secs):
        raise main.JobError("The file has no sound track")
        yield  # pragma: no cover

    monkeypatch.setattr(main, "iter_chunks", broken)
    out = wait_for(client, submit(client).json()["job_id"])
    assert out["status"] == "failed" and out["error"] == "The file has no sound track"


def test_a_model_that_cannot_load_fails_the_job(client, monkeypatch):
    def nope():
        raise RuntimeError("no network")

    monkeypatch.setattr(main, "load_model", nope)
    out = wait_for(client, submit(client).json()["job_id"])
    assert out["status"] == "failed" and out["error"] == "The speech-to-text model is not available"


def test_unknown_job_is_404_and_finished_jobs_are_forgotten(client):
    assert client.get("/jobs/nope", headers=AUTH).status_code == 404
    job_id = submit(client).json()["job_id"]
    wait_for(client, job_id)
    main._prune(time.time() + main.JOB_KEEP_SECONDS + 1)
    assert client.get(f"/jobs/{job_id}", headers=AUTH).status_code == 404


def test_the_voice_note_route_is_unchanged(client, monkeypatch):
    monkeypatch.setattr(main, "run_transcription", lambda path, language: {"text": "hello", "language": "en", "duration": 2.0})
    r = client.post("/transcribe", headers=AUTH, files={"audio": ("v.ogg", b"OggS", "audio/ogg")})
    assert r.json() == {"text": "hello", "language": "en", "duration": 2.0}


def test_the_decoder_streams_16k_mono_chunks(monkeypatch):
    """iter_chunks against a fake PyAV: frames are resampled and cut into chunks of the asked length."""
    import sys

    class Frame:
        def __init__(self, n):
            self.n = n
            self.pts = 1

        def to_ndarray(self):
            return np.full((1, self.n), 16384, dtype=np.int16)

    class Resampler:
        def __init__(self, **kw):
            assert kw == {"format": "s16", "layout": "mono", "rate": RATE}

        def resample(self, frame):
            return [] if frame is None else [frame]

    class Container:
        streams = SimpleNamespace(audio=["a0"])

        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

        def decode(self, stream):
            assert stream == "a0"
            return (Frame(RATE) for _ in range(5))  # 5 s

    fake_av = SimpleNamespace(open=lambda path, metadata_errors=None: Container(),
                              audio=SimpleNamespace(resampler=SimpleNamespace(AudioResampler=Resampler)))
    monkeypatch.setitem(sys.modules, "av", fake_av)
    chunks = list(main.iter_chunks("x.mp3", 2))
    assert [len(c) for c in chunks] == [2 * RATE, 2 * RATE, RATE]
    assert all(c.dtype == np.float32 for c in chunks) and chunks[0][0] == pytest.approx(0.5)
