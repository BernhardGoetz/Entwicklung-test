# Runner-Image für Python-Abgaben des Code-Prüfers.
# Der Container wird von der App mit Netzwerk aus, read-only Root-FS und
# tmpfs unter /work und /tmp gestartet.
FROM python:3.12.15-slim-bookworm

ENV LANG=C.UTF-8 \
    LC_ALL=C.UTF-8 \
    PYTHONUNBUFFERED=1 \
    PYTHONIOENCODING=utf-8

RUN groupadd --gid 1000 runner \
 && useradd --uid 1000 --gid 1000 --no-create-home --home-dir /tmp --shell /usr/sbin/nologin runner \
 && mkdir -p /work && chown 1000:1000 /work

USER 1000:1000
WORKDIR /work
CMD ["sleep", "infinity"]
