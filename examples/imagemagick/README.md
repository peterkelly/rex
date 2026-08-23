# ImageMagick workflow examples

These examples use the semantic `tools.imagemagick` module. Rex programs pass BLAKE3 hashes for
stored image content and receive new hashes; temporary files and ImageMagick command lines remain
inside the workflow host.

The examples intentionally use a small set of consistent conventions:

- `Image { content = hash }` turns a CAS hash into a shared image artifact.
- `IM.StoredImage image IM.AllFrames []` reads all frames with default settings.
- operations are applied in list order.
- `IM.AdjoinFrames` produces one encoded file, which may contain multiple frames.
- `IM.SeparateFrames` produces `IM.MultipleImages`.
- expected ImageMagick failures are returned as `Err IM.ImageMagickError` values.

Run an example with the workflow CLI and a JSON input file, for example:

```sh
cargo run -p rex --bin rex -- run examples/imagemagick/resize.rex --inputs inputs.json
```

The examples cover generation, resizing, thumbnails, conversion, metadata, drawing, composition,
comparison, montage creation, frame extraction, batch processing, and raw pixels.

For multi-stage workflows that pass CAS-backed images directly between ImageMagick and FFmpeg, see
the [combined examples](../imagemagick_ffmpeg/README.md).
