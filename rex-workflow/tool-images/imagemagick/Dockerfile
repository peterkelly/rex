# syntax=docker/dockerfile:1

ARG ALPINE_IMAGE=alpine:3.24@sha256:28bd5fe8b56d1bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b
FROM ${ALPINE_IMAGE} AS runtime

LABEL org.opencontainers.image.title="Rex ImageMagick tool runtime" \
      org.opencontainers.image.description="ImageMagick 7 and selected headless coders for isolated Rex workflows" \
      org.opencontainers.image.source="https://github.com/peterkelly/rex" \
      ai.rex.tool-bundle="imagemagick"

ENV HOME=/work/tmp \
    LANG=C \
    LC_ALL=C \
    MAGICK_TEMPORARY_PATH=/work/tmp \
    TMPDIR=/work/tmp \
    TZ=UTC \
    XDG_CACHE_HOME=/work/tmp

RUN apk add --no-cache \
        font-dejavu \
        fontconfig \
        imagemagick \
        imagemagick-heic \
        imagemagick-jp2 \
        imagemagick-jpeg \
        imagemagick-jxl \
        imagemagick-openexr \
        imagemagick-pango \
        imagemagick-pdf \
        imagemagick-raw \
        imagemagick-svg \
        imagemagick-tiff \
        imagemagick-webp \
    && mkdir -p /work/inputs /work/outputs /work/tmp

COPY policy.xml /etc/ImageMagick-7/policy.xml
WORKDIR /work

FROM runtime AS verification
RUN magick -version >/dev/null \
    && for command in mogrify identify compare composite montage; do \
        magick "$command" -version >/dev/null; \
    done \
    && magick identify -list format | grep -q WEBP \
    && fc-match sans-serif | grep -q .

FROM verification AS final
