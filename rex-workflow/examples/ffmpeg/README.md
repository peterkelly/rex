# FFmpeg examples

These programs use `tools.ffmpeg` entirely through content-addressed media values. They cover
generated sources, transcoding, filtering, probing, stream inspection, muxing, concatenation,
image extraction, segmented output, HLS and DASH packages, and URLs.

Most examples accept one or more `Hash` values through `main` and wrap them as `FF.Media` values.
The hashes must identify blobs in workflow's content-addressable store. HLS and DASH functions
instead return `FF.MediaPackage`, whose hash identifies a CAS tree containing the manifest and all
segments.

`generate_test_video.rex` and `generate_sine_audio.rex` require no inputs and are useful for
smoke-testing an installation. `network_input.rex` requires network access. Every `.rex` file in
this directory is parsed and typechecked by the workflow test suite.

For multi-stage workflows that pass CAS-backed media directly between FFmpeg and ImageMagick, see
the [combined examples](../imagemagick_ffmpeg/README.md).
