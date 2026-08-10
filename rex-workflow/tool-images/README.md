# Rex workflow tool images

These images package the external programs currently used by `rex-workflow`.
There is one image for each `ToolBundle` in the runtime catalog:

| Bundle | Programs | Local image |
|---|---|---|
| FFmpeg | `ffmpeg`, `ffprobe` | `rex-tool-ffmpeg:local` |
| ImageMagick | `magick` and its selected subcommands | `rex-tool-imagemagick:local` |
| QPDF | `qpdf` | `rex-tool-qpdf:local` |
| Poppler | `pdfinfo`, `pdftotext`, `pdftocairo`, `pdfimages` | `rex-tool-poppler:local` |

Keeping the projects in separate images avoids making unrelated executables
available to every tool invocation and lets each runtime be updated and
published independently. The current four-bundle list is explicit; it is not
intended to be the final registration mechanism once the tool catalog grows.

The images use Alpine Linux 3.24. Alpine publishes its base image and these
packages for both `linux/amd64` and `linux/arm64`. ImageMagick uses Alpine's
ImageMagick 7 package because the executor enters every ImageMagick operation
through `magick`. Its image also installs the packaged HEIC, JPEG 2000, JPEG,
JPEG XL, OpenEXR, Pango, PDF, raw, SVG, TIFF, and WebP coders.

None of the images has a tool `ENTRYPOINT`. `DockerToolExecutor` selects the
catalogued executable with Docker's `--entrypoint` option, so one image can
contain related programs without adding an image-specific wrapper or shell.
Locale, timezone, home, temporary-directory, and cache variables have stable
headless defaults. At runtime `/work` is replaced by the executor's sole bind
mount, and `/work/scratch` is its writable temporary area.

## Build and verify the native images

Run these commands from this directory:

```sh
docker buildx bake --load
./verify.sh
```

Omitting an explicit platform makes BuildKit select the daemon's native
platform. This is the preferred local path on both amd64 and arm64: it avoids
emulation and produces the four `:local` images listed above.

The verifier uses the same important runtime restrictions as the executor:
no network, a read-only image filesystem, no Linux capabilities,
`no-new-privileges`, and only one temporary workspace bind mount. It exercises
media generation and probing, image generation and identification, PDF repair,
and all four selected Poppler commands. The input PDF is generated inside the
temporary workspace, so the verifier needs no repository fixtures.

Configure an embedding host with the local image names as follows:

```rust,ignore
let images = DockerToolImages::new(
    "rex-tool-ffmpeg:local",
    "rex-tool-imagemagick:local",
    "rex-tool-qpdf:local",
    "rex-tool-poppler:local",
);
let state = State::docker(store, images);
```

The executor uses `--pull=never`. Building or pulling images is deliberately a
host provisioning step, never a side effect of evaluating a workflow.

## Build a multi-platform release

Set the prefix to a registry repository prefix and choose a release tag, then
build and push one manifest containing native amd64 and arm64 variants:

```sh
IMAGE_PREFIX=registry.example/rex-tool \
IMAGE_TAG=2026-08-10 \
docker buildx bake \
    --set '*.platform=linux/amd64,linux/arm64' \
    --push
```

This publishes names such as
`registry.example/rex-tool-ffmpeg:2026-08-10`. A multi-platform result cannot
be loaded into Docker's classic local image store as one image; it should be
pushed to a registry or exported as OCI artifacts. Each machine then pulls the
native variant selected by Docker.

Tags are useful release names but may be moved. Hosts that need to preserve an
exact provisioned environment should resolve the published manifest and put
its digest-qualified references in `DockerToolImages`. Workflows themselves do
not contain image references, so the person running a shared workflow remains
in control of tool provisioning and version policy.

The Alpine branch is intentionally fixed while package revisions are not.
Rebuilding picks up compatible bug and security fixes from that release
branch. Change `ALPINE_VERSION` deliberately when moving to a new Alpine
release, build both architectures, and run the smoke tests before publishing.
