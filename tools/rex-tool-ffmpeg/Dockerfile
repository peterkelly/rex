# syntax=docker/dockerfile:1

ARG ALPINE_IMAGE=alpine:3.24@sha256:28bd5fe8b56d1bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b
FROM ${ALPINE_IMAGE} AS builder

ARG FFMPEG_VERSION=8.1.2
ARG FFMPEG_SHA512=b3adc16fe426217bb607da01c4137cee0a9788fc08e077874336db185d2b7287746a7dc94cf0181ea92cd8afcdb06094ee9456e2986112354f84538cb9a5ed0b

RUN apk add --no-cache \
        build-base \
        bzip2-dev \
        ca-certificates \
        curl \
        dav1d-dev \
        fontconfig-dev \
        freetype-dev \
        fribidi-dev \
        harfbuzz-dev \
        lame-dev \
        libass-dev \
        libjxl-dev \
        libtheora-dev \
        libvorbis-dev \
        libvpx-dev \
        libwebp-dev \
        libxml2-dev \
        nasm \
        opus-dev \
        pkgconf \
        svt-av1-dev \
        x264-dev \
        x265-dev \
        xz \
        zlib-dev

WORKDIR /build
RUN curl --fail --location --retry 5 --retry-all-errors \
        "https://ffmpeg.org/releases/ffmpeg-${FFMPEG_VERSION}.tar.xz" \
        --output ffmpeg.tar.xz \
    && echo "${FFMPEG_SHA512}  ffmpeg.tar.xz" | sha512sum -c - \
    && tar -xf ffmpeg.tar.xz --strip-components=1 \
    && ./configure \
        --prefix=/usr \
        --disable-autodetect \
        --disable-debug \
        --disable-devices \
        --disable-doc \
        --disable-ffplay \
        --disable-hwaccels \
        --disable-lzma \
        --disable-network \
        --disable-static \
        --enable-avfilter \
        --enable-bzlib \
        --enable-gpl \
        --enable-indev=lavfi \
        --enable-libass \
        --enable-libdav1d \
        --enable-libfontconfig \
        --enable-libfreetype \
        --enable-libfribidi \
        --enable-libharfbuzz \
        --enable-libjxl \
        --enable-libmp3lame \
        --enable-libopus \
        --enable-libsvtav1 \
        --enable-libtheora \
        --enable-libvorbis \
        --enable-libvpx \
        --enable-libwebp \
        --enable-libx264 \
        --enable-libx265 \
        --enable-libxml2 \
        --enable-pic \
        --enable-pthreads \
        --enable-shared \
        --enable-version3 \
        --enable-zlib \
        --optflags="-O3" \
    && make -j"$(getconf _NPROCESSORS_ONLN)" \
    && make DESTDIR=/out install

FROM ${ALPINE_IMAGE} AS runtime

LABEL org.opencontainers.image.title="Rex FFmpeg tool runtime" \
      org.opencontainers.image.description="Headless FFmpeg and FFprobe for isolated Rex workflows" \
      org.opencontainers.image.source="https://github.com/peterkelly/rex" \
      ai.rex.tool-bundle="ffmpeg"

ENV HOME=/work/tmp \
    LANG=C \
    LC_ALL=C \
    TMPDIR=/work/tmp \
    TZ=UTC \
    XDG_CACHE_HOME=/work/tmp

RUN apk add --no-cache \
        libbz2 \
        font-dejavu \
        fontconfig \
        freetype \
        fribidi \
        harfbuzz \
        lame-libs \
        libass \
        libdav1d \
        libjxl \
        libtheora \
        libvorbis \
        libvpx \
        libwebp \
        libwebpmux \
        libxml2 \
        opus \
        libSvtAv1Enc \
        x264-libs \
        x265-libs \
    && mkdir -p /work/inputs /work/outputs /work/tmp

COPY --from=builder /out/usr /usr
WORKDIR /work

FROM runtime AS verification
RUN ffmpeg -version >/dev/null \
    && ffprobe -version >/dev/null \
    && ffmpeg -hide_banner -filters 2>/dev/null | grep -q subtitles \
    && ffmpeg -hide_banner -filters 2>/dev/null | grep -q drawtext \
    && ffmpeg -hide_banner -encoders 2>/dev/null | grep -q libx264 \
    && ffmpeg -hide_banner -encoders 2>/dev/null | grep -q libx265 \
    && ffmpeg -hide_banner -encoders 2>/dev/null | grep -q libsvtav1 \
    && ffmpeg -hide_banner -encoders 2>/dev/null | grep -q libvpx-vp9 \
    && ! ffmpeg -hide_banner -protocols 2>/dev/null | grep -Eq '^(  )?(http|https|tcp|udp)$' \
    && ! ffmpeg -hide_banner -devices 2>/dev/null | grep -Eq '(alsa|avfoundation|v4l2|x11grab)' \
    && ! ffmpeg -hide_banner -hwaccels 2>/dev/null | grep -Eq '^(cuda|drm|opencl|qsv|vaapi|vdpau|videotoolbox|vulkan)$' \
    && fc-match sans-serif | grep -q .

FROM verification AS final
