# syntax=docker/dockerfile:1

ARG ALPINE_IMAGE=alpine:3.24@sha256:28bd5fe8b56d1bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b
FROM ${ALPINE_IMAGE} AS runtime

LABEL org.opencontainers.image.title="Rex gnuplot tool runtime" \
      org.opencontainers.image.description="Headless gnuplot rendering for isolated Rex workflows" \
      org.opencontainers.image.source="https://github.com/peterkelly/rex" \
      ai.rex.tool-bundle="gnuplot"

ENV HOME=/work/tmp \
    LANG=C \
    LC_ALL=C \
    TMPDIR=/work/tmp \
    TZ=UTC \
    XDG_CACHE_HOME=/work/tmp

RUN apk add --no-cache \
        font-dejavu \
        fontconfig \
        gnuplot \
    && mkdir -p /work/inputs /work/outputs /work/tmp

WORKDIR /work

FROM runtime AS verification
RUN gnuplot --version | grep -qi '^gnuplot ' \
    && printf '%s\n' \
        'set terminal svg size 64,64' \
        'set output "/tmp/check.svg"' \
        'plot x with lines' \
        | gnuplot \
    && grep -q '<svg' /tmp/check.svg \
    && fc-match sans-serif | grep -q .

FROM verification AS final
