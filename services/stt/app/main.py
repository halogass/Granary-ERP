"""
Speech-to-text service (Nary): customer WhatsApp voice notes and the Owner's debriefs, transcribed on
the ERP server with faster-whisper (CTranslate2, int8 on the CPU). Audio never leaves the server.

  GET  /health
  POST /transcribe   multipart: audio (file), language? (en | fr | ...), max_seconds?
                     → {text, language, duration}
  POST /jobs         multipart: audio (file), language?, max_seconds?  → 202 {job_id, status}
                     a long meeting recording, transcribed in the background (one job at a time)
  GET  /jobs/{id}    → {job_id, status (queued | transcribing | done | failed), progress (0-100),
                        text, language, duration, error}

Every route but /health needs `Authorization: Bearer <STT_SERVICE_TOKEN>`. One transcription at a
time (STT_CONCURRENCY), and the container's CPU share is capped in docker-compose (STT_CPUS), so a
long recording can't starve the rest of the ERP. The model loads once, on the first request.

Jobs (Nary batch 3, meeting recordings of up to a few hours): the upload is copied to a file under
STT_JOB_DIR in 1 MB pieces (never held in memory), queued, and transcribed by one background worker, one
job at a time. The audio is decoded in chunks of STT_JOB_CHUNK_SECONDS (16 kHz mono), so memory stays
small whatever the length; progress is the seconds processed over the recording's length. The file is
deleted when the job ends; a finished job is forgotten after STT_JOB_KEEP_SECONDS. Jobs live in memory: a
restart forgets them (GET answers 404 and the ERP sends the recording again).
"""
from __future__ import annotations

import asyncio
import logging
import os
import tempfile
import time
import uuid
from contextlib import asynccontextmanager

from fastapi import FastAPI, File, Form, Header, HTTPException, UploadFile

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s %(message)s")
log = logging.getLogger("stt")

TOKEN = os.environ.get("STT_SERVICE_TOKEN") or os.environ.get("SERVICE_TOKEN") or ""
MODEL = os.environ.get("STT_MODEL", "small")              # tiny | base | small | medium (CPU: small)
THREADS = int(os.environ.get("STT_THREADS", "1"))         # match the compose CPU cap
MAX_BYTES = int(os.environ.get("STT_MAX_BYTES", str(40 * 1024 * 1024)))
BEAM = int(os.environ.get("STT_BEAM_SIZE", "1"))

# Jobs: hard caps of the service (the ERP's own limits, from its settings, are lower and sent as max_seconds).
JOB_DIR = os.environ.get("STT_JOB_DIR", os.path.join(tempfile.gettempdir(), "stt-jobs"))
JOB_MAX_BYTES = int(os.environ.get("STT_JOB_MAX_BYTES", str(600 * 1024 * 1024)))
JOB_MAX_SECONDS = int(os.environ.get("STT_JOB_MAX_SECONDS", str(6 * 3600)))
JOB_CHUNK_SECONDS = int(os.environ.get("STT_JOB_CHUNK_SECONDS", "600"))
JOB_KEEP_SECONDS = int(os.environ.get("STT_JOB_KEEP_SECONDS", str(24 * 3600)))
JOB_QUEUE_MAX = int(os.environ.get("STT_JOB_QUEUE_MAX", "20"))
SAMPLE_RATE = 16000

_slots = asyncio.Semaphore(int(os.environ.get("STT_CONCURRENCY", "1")))
_model = None
_jobs: dict[str, dict] = {}
_queue: asyncio.Queue | None = None
_worker_task: asyncio.Task | None = None


class JobError(Exception):
    """A job failed for a reason the ERP can tell the Owner (the message is shown as is)."""


def load_model():
    """The faster-whisper model, loaded once. Kept separate so tests can replace it."""
    global _model
    if _model is None:
        from faster_whisper import WhisperModel
        _model = WhisperModel(MODEL, device="cpu", compute_type="int8", cpu_threads=THREADS,
                              download_root=os.environ.get("STT_MODEL_DIR", "/models"))
        log.info("stt model loaded (%s, %d thread(s))", MODEL, THREADS)
    return _model


def probe_seconds(path: str) -> float | None:
    """The recording's length, without decoding it all (PyAV ships with faster-whisper)."""
    try:
        import av
        with av.open(path) as container:
            if container.duration:
                return container.duration / 1_000_000
    except Exception:  # noqa: BLE001 — unknown length: the limit is enforced after decoding
        return None
    return None


def run_transcription(path: str, language: str | None) -> dict:
    model = load_model()
    segments, info = model.transcribe(path, language=language or None, beam_size=BEAM, vad_filter=True)
    text = " ".join(s.text.strip() for s in segments).strip()
    return {"text": text, "language": info.language, "duration": round(info.duration or 0, 2)}


def iter_chunks(path: str, chunk_seconds: int):
    """The recording's sound as float32 16 kHz mono arrays of at most `chunk_seconds` each, decoded as it
    goes (PyAV), so a four-hour meeting never sits in memory at once."""
    import av
    import numpy as np

    size = SAMPLE_RATE * max(1, chunk_seconds)
    parts: list = []
    have = 0

    def take(arrays, have):
        data = np.concatenate(arrays) if len(arrays) > 1 else arrays[0]
        return data[:size], data[size:]

    try:
        container = av.open(path, metadata_errors="ignore")
    except Exception as e:  # noqa: BLE001
        raise JobError("The recording could not be read") from e
    with container:
        if not container.streams.audio:
            raise JobError("The file has no sound track")
        stream = container.streams.audio[0]
        resampler = av.audio.resampler.AudioResampler(format="s16", layout="mono", rate=SAMPLE_RATE)

        def frames():
            for frame in container.decode(stream):
                frame.pts = None
                yield from resampler.resample(frame)
            yield from resampler.resample(None)

        for out in frames():
            arr = out.to_ndarray().reshape(-1)
            if not arr.size:
                continue
            parts.append(arr)
            have += arr.size
            while have >= size:
                chunk, rest = take(parts, have)
                yield chunk.astype(np.float32) / 32768.0
                parts = [rest] if rest.size else []
                have = rest.size
    if have:
        chunk, _ = take(parts, have)
        yield chunk.astype(np.float32) / 32768.0


def transcribe_file(path: str, language: str | None, limit_seconds: float, on_progress) -> dict:
    """A long recording, chunk by chunk, segments streamed: {text, language, duration}. The language of the
    first chunk (given or detected) is kept for the rest, so a mixed call stays in one language."""
    model = load_model()
    total = probe_seconds(path) or 0.0
    lines: list[str] = []
    offset = 0.0
    lang = language or None
    for chunk in iter_chunks(path, JOB_CHUNK_SECONDS):
        seconds = len(chunk) / SAMPLE_RATE
        if offset + seconds > limit_seconds + 1:
            raise JobError(f"The recording is longer than {limit_seconds / 60:.0f} minutes")
        segments, info = model.transcribe(chunk, language=lang, beam_size=BEAM, vad_filter=True)
        if not lang:
            lang = info.language
        for seg in segments:
            text = (seg.text or "").strip()
            if text:
                lines.append(text)
            if total:
                on_progress(min(99, int((offset + seg.end) * 100 / total)))
        offset += seconds
        if total:
            on_progress(min(99, int(offset * 100 / total)))
    return {"text": "\n".join(lines).strip(), "language": lang, "duration": round(offset, 2)}


def _remove(path: str | None) -> None:
    if path:
        try:
            os.remove(path)
        except FileNotFoundError:
            pass


def _prune(now: float | None = None) -> None:
    now = now or time.time()
    for job_id, job in list(_jobs.items()):
        if job.get("finished_at") and now - job["finished_at"] > JOB_KEEP_SECONDS:
            _jobs.pop(job_id, None)


async def _run_job(job_id: str) -> None:
    job = _jobs.get(job_id)
    if not job or job["status"] != "queued":
        return
    job["status"] = "transcribing"
    job["started_at"] = time.time()
    try:
        try:
            await asyncio.to_thread(load_model)
        except Exception as e:  # noqa: BLE001: the service's problem, not the recording's
            log.error("model could not be loaded: %s", e)
            raise JobError("The speech-to-text model is not available") from e

        def progress(p: int) -> None:
            job["progress"] = max(job["progress"], int(p))

        out = await asyncio.to_thread(transcribe_file, job["path"], job["language"], job["limit"], progress)
        job.update(status="done", progress=100, text=out["text"], language=out["language"],
                   duration=out["duration"])
    except JobError as e:
        job.update(status="failed", error=str(e))
    except Exception as e:  # noqa: BLE001
        log.warning("job %s failed: %s", job_id, e)
        job.update(status="failed", error="The recording could not be read")
    finally:
        _remove(job.pop("path", None))
        job["finished_at"] = time.time()
        log.info("job %s %s", job_id, job["status"])


async def _worker() -> None:
    """One job at a time, in the order they came."""
    assert _queue is not None
    while True:
        job_id = await _queue.get()
        try:
            await _run_job(job_id)
        finally:
            _queue.task_done()
            _prune()


@asynccontextmanager
async def _lifespan(app: FastAPI):
    global _queue, _worker_task
    if not TOKEN:
        raise RuntimeError("STT_SERVICE_TOKEN is not set: refusing to start without auth")
    # files of jobs a restart forgot
    os.makedirs(JOB_DIR, exist_ok=True)
    for name in os.listdir(JOB_DIR):
        _remove(os.path.join(JOB_DIR, name))
    _queue = asyncio.Queue()
    _worker_task = asyncio.create_task(_worker())
    log.info("stt service ready (model=%s, loads on first use)", MODEL)
    try:
        yield
    finally:
        _worker_task.cancel()
        _worker_task = None
        _queue = None


app = FastAPI(title="Granary speech-to-text", docs_url=None, redoc_url=None, lifespan=_lifespan)


def _auth(authorization: str | None) -> None:
    import hmac
    if not TOKEN or not hmac.compare_digest((authorization or "").encode(), f"Bearer {TOKEN}".encode()):
        raise HTTPException(status_code=401, detail="Unauthorized")


@app.get("/health")
async def health():
    return {"status": "ok", "model": MODEL, "loaded": _model is not None}


@app.post("/transcribe")
async def transcribe(audio: UploadFile = File(...), language: str | None = Form(None),
                     max_seconds: int | None = Form(None), authorization: str | None = Header(None)):
    _auth(authorization)
    data = await audio.read()
    if not data:
        raise HTTPException(status_code=422, detail="Empty recording")
    if len(data) > MAX_BYTES:
        raise HTTPException(status_code=413, detail="Recording too large")
    suffix = os.path.splitext(audio.filename or "")[1] or ".ogg"
    with tempfile.NamedTemporaryFile(suffix=suffix) as tmp:
        tmp.write(data)
        tmp.flush()
        seconds = probe_seconds(tmp.name)
        if max_seconds and seconds and seconds > max_seconds:
            raise HTTPException(status_code=413, detail=f"Recording is {seconds:.0f}s; the limit is {max_seconds}s")
        async with _slots:
            try:
                await asyncio.to_thread(load_model)
            except Exception as e:  # noqa: BLE001 — the service's problem, not the recording's
                log.error("model could not be loaded: %s", e)
                raise HTTPException(status_code=503, detail="The speech-to-text model is not available")
            try:
                out = await asyncio.to_thread(run_transcription, tmp.name, language)
            except Exception as e:  # noqa: BLE001
                log.warning("transcription failed: %s", e)
                raise HTTPException(status_code=422, detail="The recording could not be read")
    if max_seconds and out["duration"] and out["duration"] > max_seconds:
        raise HTTPException(status_code=413, detail="Recording too long")
    return out


def _job_view(job_id: str, job: dict) -> dict:
    return {"job_id": job_id, "status": job["status"], "progress": job["progress"],
            "text": job.get("text") if job["status"] == "done" else None,
            "language": job.get("language"), "duration": job.get("duration"), "error": job.get("error")}


@app.post("/jobs", status_code=202)
async def create_job(audio: UploadFile = File(...), language: str | None = Form(None),
                     max_seconds: int | None = Form(None), authorization: str | None = Header(None)):
    _auth(authorization)
    if _queue is None:
        raise HTTPException(status_code=503, detail="The job queue is not running")
    waiting = sum(1 for j in _jobs.values() if j["status"] in ("queued", "transcribing"))
    if waiting >= JOB_QUEUE_MAX:
        raise HTTPException(status_code=503, detail="Too many recordings are waiting; try again later")
    os.makedirs(JOB_DIR, exist_ok=True)
    job_id = uuid.uuid4().hex
    ext = os.path.splitext(audio.filename or "")[1].lower()
    suffix = ext if ext.isascii() and 1 < len(ext) <= 6 and ext[1:].isalnum() else ".bin"
    path = os.path.join(JOB_DIR, job_id + suffix)
    size = 0
    try:
        with open(path, "wb") as out:
            while True:
                piece = await audio.read(1024 * 1024)
                if not piece:
                    break
                size += len(piece)
                if size > JOB_MAX_BYTES:
                    raise HTTPException(status_code=413,
                                        detail=f"Recording too large ({JOB_MAX_BYTES // (1024 * 1024)} MB at most)")
                out.write(piece)
    except BaseException:
        _remove(path)
        raise
    if size == 0:
        _remove(path)
        raise HTTPException(status_code=422, detail="Empty recording")
    limit = min(max_seconds or JOB_MAX_SECONDS, JOB_MAX_SECONDS)
    seconds = probe_seconds(path)
    if seconds and seconds > limit:
        _remove(path)
        raise HTTPException(status_code=413,
                            detail=f"The recording is {seconds / 60:.0f} minutes; the limit is {limit / 60:.0f} minutes")
    _jobs[job_id] = {"status": "queued", "progress": 0, "path": path, "language": language or None,
                     "limit": limit, "duration": round(seconds, 2) if seconds else None,
                     "created_at": time.time()}
    await _queue.put(job_id)
    return {"job_id": job_id, "status": "queued"}


@app.get("/jobs/{job_id}")
async def get_job(job_id: str, authorization: str | None = Header(None)):
    _auth(authorization)
    job = _jobs.get(job_id)
    if job is None:
        raise HTTPException(status_code=404, detail="No such job")
    return _job_view(job_id, job)
