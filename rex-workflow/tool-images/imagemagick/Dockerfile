# syntax=docker/dockerfile:1

ARG ALPINE_VERSION=3.24
FROM alpine:${ALPINE_VERSION}

ARG ALPINE_VERSION
LABEL org.opencontainers.image.title="Rex ImageMagick tool runtime" \
      org.opencontainers.image.description="ImageMagick 7 and common headless coders for isolated Rex workflows" \
      org.opencontainers.image.source="https://github.com/peterkelly/rex" \
      ai.rex.tool-bundle="imagemagick" \
      ai.rex.alpine-version="${ALPINE_VERSION}"

ENV HOME=/work/scratch \
    LANG=C \
    LC_ALL=C \
    MAGICK_TEMPORARY_PATH=/work/scratch \
    TMPDIR=/work/scratch \
    TZ=UTC \
    XDG_CACHE_HOME=/work/scratch

RUN apk add --no-cache \
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
    && magick -version >/dev/null \
    && for command in mogrify identify compare composite montage; do \
        magick "$command" -version >/dev/null; \
    done

WORKDIR /work
