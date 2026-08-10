# Combined ImageMagick and FFmpeg examples

These workflows demonstrate direct, CAS-backed composition between `tools.imagemagick` and
`tools.ffmpeg`. An output hash from one module is wrapped in the semantic input type of the other;
Rex never observes or constructs an intermediate filename.

- `video_contact_sheet.rex` extracts frames with FFmpeg and arranges them with ImageMagick.
- `polished_thumbnail.rex` extracts a frame with FFmpeg and finishes it with ImageMagick.
- `watermarked_video.rex` prepares a watermark with ImageMagick and composites it with FFmpeg.
- `title_card_video.rex` generates a title image with ImageMagick and encodes video/audio with
  FFmpeg.
- `stylized_gif.rex` samples video with FFmpeg and renders an animated GIF with ImageMagick.

Because the two modules have distinct expected-error types, each example defines a small
`WorkflowError` ADT and preserves which external tool failed. Examples that use general FFmpeg or
multi-image output also reject an unexpected semantic output shape explicitly.

Every program has a top-level comment with its required inputs, exact workflow CLI invocation, and
output description. The complete directory is parsed and typechecked by the workflow test suite.
