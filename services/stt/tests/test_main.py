"""The service contract, with the model replaced (no download, no CPU work)."""
import os

os.environ.setdefault("STT_SERVICE_TOKEN", "test-token")

import pytest
from fastapi.testclient import TestClient

from app import main


@pytest.fixture
def client(monkeypatch):
    monkeypatch.setattr(main, "TOKEN", "test-token")
    monkeypatch.setattr(main, "run_transcription",
                        lambda path, language: {"text": "hello, what is the price", "language": language or "en", "duration": 4.2})
    monkeypatch.setattr(main, "probe_seconds", lambda path: 4.2)
    monkeypatch.setattr(main, "load_model", lambda: object())
    with TestClient(main.app) as c:
        yield c


AUTH = {"Authorization": "Bearer test-token"}


def test_health_needs_no_token(client):
    assert client.get("/health").json()["status"] == "ok"


def test_transcribe_needs_the_token(client):
    r = client.post("/transcribe", files={"audio": ("v.ogg", b"OggS....", "audio/ogg")})
    assert r.status_code == 401


def test_transcribe_returns_text_language_duration(client):
    r = client.post("/transcribe", headers=AUTH, files={"audio": ("v.ogg", b"OggS....", "audio/ogg")}, data={"language": "en"})
    assert r.status_code == 200
    assert r.json() == {"text": "hello, what is the price", "language": "en", "duration": 4.2}


def test_too_long_is_refused_before_transcribing(client, monkeypatch):
    monkeypatch.setattr(main, "probe_seconds", lambda path: 900)
    r = client.post("/transcribe", headers=AUTH, files={"audio": ("v.ogg", b"OggS....", "audio/ogg")}, data={"max_seconds": "600"})
    assert r.status_code == 413


def test_empty_is_refused(client):
    r = client.post("/transcribe", headers=AUTH, files={"audio": ("v.ogg", b"", "audio/ogg")})
    assert r.status_code == 422


def test_a_model_that_cannot_load_is_a_503_not_a_bad_recording(client, monkeypatch):
    def broken():
        raise RuntimeError("no network")
    monkeypatch.setattr(main, "load_model", broken)
    r = client.post("/transcribe", headers=AUTH, files={"audio": ("v.ogg", b"OggS....", "audio/ogg")})
    assert r.status_code == 503
