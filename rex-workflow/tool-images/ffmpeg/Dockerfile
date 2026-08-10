# syntax=docker/dockerfile:1

ARG ALPINE_VERSION=3.24
FROM alpine:${ALPINE_VERSION}

ARG ALPINE_VERSION
LABEL org.opencontainers.image.title="Rex FFmpeg tool runtime" \
      org.opencontainers.image.description="FFmpeg and FFprobe for isolated Rex workflows" \
      org.opencontainers.image.source="https://github.com/peterkelly/rex" \
      ai.rex.tool-bundle="ffmpeg" \
      ai.rex.alpine-version="${ALPINE_VERSION}"

ENV HOME=/work/scratch \
    LANG=C \
    LC_ALL=C \
    TMPDIR=/work/scratch \
    TZ=UTC \
    XDG_CACHE_HOME=/work/scratch

RUN apk add --no-cache ffmpeg \
    && ffmpeg -version >/dev/null \
    && ffprobe -version >/dev/null

WORKDIR /work
