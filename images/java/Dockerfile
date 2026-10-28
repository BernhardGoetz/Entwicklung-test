# Runner-Image für Java-Abgaben des Code-Prüfers.
# Der Container wird von der App mit Netzwerk aus, read-only Root-FS und
# tmpfs unter /work und /tmp gestartet.
# Basis-Image per Digest (Manifest-Liste, multi-arch) gepinnt; der Tag dient
# nur der Lesbarkeit. Aktualisierung: Dependabot (docker) bzw.
# `docker buildx imagetools inspect eclipse-temurin:<tag>`.
FROM eclipse-temurin:21.0.12_8-jdk-noble@sha256:4d271cd5e0624598cf563342f47281b09cb364bc13acbbd7251f49f83470018d

ENV LANG=C.UTF-8 \
    LC_ALL=C.UTF-8

# Das Ubuntu-Basisimage bringt bereits einen Benutzer mit UID 1000 mit
# ("ubuntu"); er wird durch einen unprivilegierten "runner" ersetzt.
RUN userdel --remove ubuntu 2>/dev/null || true \
 && groupadd --gid 1000 runner \
 && useradd --uid 1000 --gid 1000 --no-create-home --home-dir /tmp --shell /usr/sbin/nologin runner \
 && mkdir -p /work && chown 1000:1000 /work

USER 1000:1000
WORKDIR /work
CMD ["sleep", "infinity"]
