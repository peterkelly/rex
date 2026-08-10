# syntax=docker/dockerfile:1

ARG ALPINE_VERSION=3.24
FROM alpine:${ALPINE_VERSION}

ARG ALPINE_VERSION
LABEL org.opencontainers.image.title="Rex Poppler tool runtime" \
      org.opencontainers.image.description="Poppler command-line tools for isolated Rex workflows" \
      org.opencontainers.image.source="https://github.com/peterkelly/rex" \
      ai.rex.tool-bundle="poppler" \
      ai.rex.alpine-version="${ALPINE_VERSION}"

ENV HOME=/work/scratch \
    LANG=C \
    LC_ALL=C \
    TMPDIR=/work/scratch \
    TZ=UTC \
    XDG_CACHE_HOME=/work/scratch

RUN apk add --no-cache poppler-utils \
    && pdfinfo -v 2>&1 | grep -q 'pdfinfo version' \
    && pdftotext -v 2>&1 | grep -q 'pdftotext version' \
    && pdftocairo -v 2>&1 | grep -q 'pdftocairo version' \
    && pdfimages -v 2>&1 | grep -q 'pdfimages version'

WORKDIR /work
