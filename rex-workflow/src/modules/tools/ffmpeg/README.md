# FFmpeg tools for Rex

The workflow host registers a semantic media API as `tools.ffmpeg`. It is designed for Rex programs
written by agents: programs describe media sources, stream processing, encodings, packages, and
inspection requests rather than executables, argument ordering, pipes, or temporary files.

The API is strictly headless: it does not expose playback or host capture devices. Known FFmpeg
device demuxer and muxer names are rejected even through generic format fields. FFmpeg and FFprobe
are resolved through `PATH`. No installation path is built into the
`rex-workflow` crate. External script files such as filter-complex scripts are not exposed; filter
graphs are represented directly as Rex values.

## Content-addressed media

An encoded media file is a shared CAS-backed artifact:

```rex
import artifacts (Media);

Media { content = hash }
```

FFmpeg inputs are exported to a private temporary workspace immediately before a process starts.
Every declared output is imported back into the content-addressable store before the Rex function
returns. Rex never observes a host path, extension, sequence pattern, playlist filename, or segment
filename.

HLS and DASH need more than an unordered collection of files because manifests refer to segments by
name. They therefore return `MediaPackage` values backed by CAS trees:

```rex
FF.MediaPackage { content = tree_hash, kind = FF.HlsPackage }
```

The fixed internal manifest names and all segments remain inside the tree. A package can be supplied
later as `StoredPackage` without unpacking it in Rex.

`MediaArtifact` represents the three output shapes of a general media program:

- `EncodedMedia Media` for a single container;
- `MediaSequence (List Media)` for image sequences or independent segments;
- `PackagedMedia MediaPackage` for HLS or DASH.

## Functions

| Function | Purpose | Program |
|---|---|---|
| `transcode` | Transform and encode one stored, generated, or network source | `ffmpeg` |
| `remux` | Change containers while copying all streams | `ffmpeg` |
| `extract_audio` | Select and encode the first audio stream | `ffmpeg` |
| `extract_frames` | Produce a CAS-backed image sequence | `ffmpeg` |
| `thumbnail` | Select and optionally resize one representative frame | `ffmpeg` |
| `concatenate` | Join media inputs with timestamp and format normalization | `ffmpeg` |
| `mux` | Select streams from several inputs and combine them | `ffmpeg` |
| `segment` | Produce independent, numbered media segments | `ffmpeg` |
| `package_hls` | Produce one HLS CAS tree | `ffmpeg` |
| `package_dash` | Produce one DASH CAS tree | `ffmpeg` |
| `render` | Execute a typed multi-input filter graph with multiple outputs | `ffmpeg` |
| `probe` | Return typed container, stream, chapter, and program metadata | `ffprobe` |
| `inspect` | Return selected frame or packet fields | `ffprobe` |
| `version` | Return parsed version, configuration, and library versions | `ffmpeg` |
| `capabilities` | Query codecs, formats, filters, devices, protocols, and related domains | `ffmpeg` |

Expected process failures are returned as `Result` errors containing `FfmpegError`. Missing
executables, store failures, and executor failures remain host evaluation errors because the media
operation itself did not run to a meaningful completion state.

## Sources

`MediaSource` separates the origin of a stream from its input options:

- `StoredMedia` reads one CAS blob;
- `StoredPackage` materializes an HLS or DASH CAS tree;
- `TestVideo` and `SolidVideo` use FFmpeg's generated video sources;
- `SineAudio` and `SilenceAudio` generate audio;

The generated sources use typed size, frame-rate, sample-rate, duration, color, and pattern fields.
`MediaInput` adds seek ranges, looping, format hints, hardware acceleration, decoder selection, and
demuxer or protocol options for the general `render` API.

Network sources are intentionally explicit because they have different reproducibility properties
from CAS content. All media produced from them is still imported into the store before being
returned.

## Common transcoding

`transcode source operations encoding` is the preferred API for one input. `MediaOperation`
separates video operations, audio operations, time ranges, stream selection, dropped media kinds,
and output metadata. `Encoding` independently specifies the container and optional video, audio,
and subtitle encoders.

Common video filters include scaling, cropping, padding, frame-rate conversion, pixel and aspect
formats, rotation, transposition, deinterlacing, denoising, blurring, sharpening, equalization,
chroma keying, fades, overlays, text, subtitle rendering, frame selection, timestamp expressions,
reversal, and loops.

Common audio filters include gain, loudness normalization, resampling, channel layouts, fades,
delay, tempo, equalization, high- and low-pass filtering, compression, limiting, gating, echo,
silence removal, and reversal.

Encoder records expose semantic rate control, quality, presets, profiles, levels, pixel formats,
sample formats, channel layouts, GOP structure, threading, and codec-specific options. Named common
codecs have constructors; `OtherVideoCodec`, `OtherAudioCodec`, and `OtherSubtitleCodec` allow an
installed encoder to be selected without waiting for a new Rex release.

## General filter graphs

`render` accepts a `MediaProgram` with inputs, a `FilterGraph`, and outputs. Use it when streams from
several inputs must meet in one graph, filter outputs feed later chains, or one invocation must
produce several output formats.

Each `FilterChain` has:

1. input pads referring to an input stream or an earlier named link;
2. an ordered list of typed `MediaFilter` values;
3. named output pads.

Each `MediaOutput` maps input streams or named filter outputs to `OutputStream` records. A stream
record chooses copy or encoding behavior, metadata, and dispositions. Output modes describe single
containers, numbered files, time-based segments, HLS trees, or DASH trees.

`CustomFilter`, `CustomVideoFilter`, and `CustomAudioFilter` are semantic FFmpeg escape hatches. They
accept a filter name and named or positional filter options, but they do not accept raw command-line
arguments. This preserves graph structure and lets the executor continue to own all path handling.
Path-bearing operations such as fonts and subtitle files have dedicated CAS-backed fields rather
than string filenames.

Likewise, codec-specific, muxer, demuxer, and protocol options are scoped to the object they
configure. There is deliberately no `RawArgument`, shell command, filter script, or arbitrary
output filename constructor.

## Encoding and output shapes

`VideoEncoding`, `AudioEncoding`, and `SubtitleEncoding` select codecs independently. `None` in a
simple `Encoding` means that media kind is omitted. General outputs instead use explicit
`OutputStream` mappings and may mix copied and encoded streams.

Image encodings separate the image extension from the internal `image2` muxer. `extract_frames`
returns the ordered hashes of the produced files, and `thumbnail` returns one `Media` value.
`AtTimes` performs one accurate seek and extraction per timestamp instead of relying on fragile
floating-point equality inside a select-filter expression.

`segment` also returns an ordered list of independent files. HLS and DASH are not flattened because
doing so would destroy manifest relationships; both return a tree-backed `MediaPackage`.

## Inspection

`probe` invokes FFprobe's JSON writer and converts it into stable Rex records. `MediaInfo` contains:

- optional container information and tags;
- typed stream kinds plus codec, geometry, timing, rate, channel, disposition, and tag fields;
- chapter boundaries and tags;
- program identifiers and stream membership.

`inspect` handles potentially large frame and packet reports. Callers select the media kind, stream,
interval, and exact field names they need. Each result is an `InspectionRecord` dictionary so FFmpeg
can expose codec-specific fields without destabilizing the core metadata schema.

## Agent guidance

Prefer the narrowest function that expresses the task:

1. Use `transcode` for one source with ordinary video or audio edits.
2. Use `remux` when codecs must remain unchanged.
3. Use `extract_audio`, `extract_frames`, or `thumbnail` for those specific outputs.
4. Use `concatenate` or `mux` instead of manually constructing their filter graphs.
5. Use `segment`, `package_hls`, or `package_dash` according to the semantic output shape.
6. Use `render` only for multi-input graphs, named filter links, or multiple outputs.
7. Prefer named codecs and filters; use custom constructors only for installed functionality not yet
   represented by a dedicated constructor.
8. Probe unfamiliar media before choosing stream indices, layouts, or encoders.

See [`examples/ffmpeg`](examples/ffmpeg/README.md) for complete programs. Every example is parsed and
typechecked in the workflow test suite.
Cross-tool pipelines are demonstrated in the
[`examples/imagemagick_ffmpeg`](examples/imagemagick_ffmpeg/README.md) directory.

## Host architecture

The FFmpeg compiler lowers semantic values into a `ToolExecutionPlan`. A plan contains symbolic CAS
inputs, symbolic output slots, and argument fragments whose paths are resolved only by a
`ToolExecutor`. Joined argument fragments support path-bearing filter values without leaking the
materialized path into Rex or into the semantic compiler.

`LocalToolExecutor` currently creates a temporary workspace, exports blobs or trees, invokes the
program without a shell, imports files, sequences, or trees, and removes the workspace. The trait is
the boundary for replacing local execution with an isolated runner later; none of the Rex-visible
FFmpeg types depend on the local implementation.
