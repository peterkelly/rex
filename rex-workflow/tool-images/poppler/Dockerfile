# syntax=docker/dockerfile:1

ARG ALPINE_IMAGE=alpine:3.24@sha256:28bd5fe8b56d1bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b
FROM ${ALPINE_IMAGE} AS runtime

LABEL org.opencontainers.image.title="Rex Poppler tool runtime" \
      org.opencontainers.image.description="Poppler command-line tools for isolated Rex workflows" \
      org.opencontainers.image.source="https://github.com/peterkelly/rex" \
      ai.rex.tool-bundle="poppler"

ENV HOME=/work/tmp \
    LANG=C \
    LC_ALL=C \
    TMPDIR=/work/tmp \
    TZ=UTC \
    XDG_CACHE_HOME=/work/tmp

RUN apk add --no-cache \
        cairo \
        font-dejavu \
        fontconfig \
        libjpeg-turbo \
        libpng \
        poppler-utils \
        tiff \
    && mkdir -p /work/inputs /work/outputs /work/tmp

WORKDIR /work

FROM runtime AS verification
RUN pdfinfo -v 2>&1 | grep -q 'pdfinfo version' \
    && pdftotext -v 2>&1 | grep -q 'pdftotext version' \
    && pdftocairo -v 2>&1 | grep -q 'pdftocairo version' \
    && pdfimages -v 2>&1 | grep -q 'pdfimages version' \
    && fc-match sans-serif | grep -q .

FROM verification AS final
