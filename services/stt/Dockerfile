FROM python:3.12-slim

WORKDIR /app
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# The model is downloaded once into /models (a volume) on first use, or baked in at build time:
#   docker compose build --build-arg STT_PRELOAD=small stt
ARG STT_PRELOAD=""
RUN mkdir -p /models && if [ -n "$STT_PRELOAD" ]; then \
      python -c "from faster_whisper import WhisperModel; WhisperModel('$STT_PRELOAD', device='cpu', compute_type='int8', download_root='/models')"; \
    fi

COPY app/ ./app/
COPY tests/ ./tests/

# Nothing here needs root.
RUN useradd --system --home /tmp stt && chown -R stt /models
USER stt

EXPOSE 3005
CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "3005", "--workers", "1"]
