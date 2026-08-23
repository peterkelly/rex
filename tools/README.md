# Rex workflow tools

Each installable `rex-tool-*` binary translates its Rex-facing functions into
an OCI execution plan. The tool crate owns its executable names, image
selection, Dockerfile, and any runtime policy files; the `rex` crate provides
only the generic protocol, storage, and OCI execution infrastructure.

The repository's bundled development set has one image per tool:

| Tool | Programs | Local development image |
|---|---|---|
| FFmpeg | `ffmpeg`, `ffprobe` | `rex-tool-ffmpeg:local` |
| Gnuplot | `gnuplot` | `rex-tool-gnuplot:local` |
| Graphviz | `dot` | `rex-tool-graphviz:local` |
| ImageMagick | `magick` and selected subcommands | `rex-tool-imagemagick:local` |
| QPDF | `qpdf` | `rex-tool-qpdf:local` |
| Poppler | `pdfinfo`, `pdftotext`, `pdftocairo`, `pdfimages` | `rex-tool-poppler:local` |

This is the repository's bundled development set; the runtime discovers other
installed `rex-tool-*` binaries without a central catalog.

## Docker with locally built images

Rex currently builds the six tool images on each machine and tags them
`rex-tool-*:local`. This gives amd64 and arm64 hosts native code without
requiring a public image registry. Image publication and a checked-in digest
lock are deliberately deferred until the bundled tools and Rex releases are
more mature.

Build and verify the bundled development images from the repository:

```sh
cd tools
docker buildx bake --load
./verify-images.sh
```

Then run a workflow:

```sh
rex run workflow.rex
```

Each installable tool crate owns its Dockerfile and image policy. The top-level
`docker-bake.hcl` only coordinates development builds. The executor always uses
`--pull=never`, so evaluating a workflow never contacts a registry or rebuilds
an image as a side effect.

Interrupted runs normally remove their containers. To remove stopped
containers left by a process or daemon failure, run:

```sh
rex tools cleanup
```

`rex tools cleanup --include-running` also removes labelled containers Docker
still reports as running. That option may terminate a live Rex invocation, so
it is intentionally explicit.

## Isolation contract

Every invocation receives a fresh workspace and container. The executor:

- materializes only the plan's declared CAS blob and tree inputs;
- mounts `/work/inputs` read-only and `/work/outputs` read-write;
- supplies `/work/tmp` as a size-limited `noexec,nosuid,nodev` tmpfs;
- does not mount the CAS, repository, current directory, home directory,
  system fonts, Docker socket, or any caller-selected host path;
- disables networking, capabilities, privilege escalation, and health checks;
- uses a read-only image root and memory, CPU, PID, and temporary-storage
  limits, under a non-root numeric UID/GID;
- sets a headless C locale, UTC timezone, temporary home, and cache paths;
- labels containers for recovery without exposing container selection to Rex;
- separates Docker infrastructure failures from a tool's ordinary exit status;
- applies an execution timeout and bounded stdout/stderr capture; and
- explicitly inspects and removes the container after completion, with
  best-effort removal if the evaluation future is cancelled.

Output import rejects symbolic links and special files recursively before data
enters the CAS.

FFmpeg's workflow API does not expose network sources, capture devices, or
hardware acceleration. The images receive no host devices. ImageMagick has a
defence-in-depth policy disabling network, desktop/capture/print, indirect-path,
MSL, and MVG coders while preserving the documented headless formats.

## Build local images

The Dockerfiles use the digest-pinned Alpine 3.24 multi-platform base and
multi-stage capability checks. FFmpeg/Gnuplot/Graphviz/ImageMagick/Poppler
include DejaVu as a small fallback font set. ImageMagick installs its selected
codec delegates; QPDF remains minimal.

Build from the top-level `tools` directory:

```sh
docker buildx bake --load
```

When developing the image definitions, build native images and run their smoke
suite from this directory:

```sh
docker buildx bake --load
./verify-images.sh
```

No platform override means BuildKit uses the daemon's native architecture.
The installed tool binaries use these local tags by default:

```sh
rex run workflow.rex
```

Embedders make the same policy explicit:

```rust,ignore
let image = OciImage::new(
    "my-tool",
    "registry.example/my-tool@sha256:<digest>",
    OciPlatform::native_linux(),
);
let state = State::docker(store, image, false);
```

Each tool binary owns its image environment variable and local development
default. Embedders should use digest-qualified references and pass `false` for
the `allow_image_tags` argument. The bundled tool image variables are:

- `REX_WORKFLOW_FFMPEG_IMAGE`
- `REX_WORKFLOW_GNUPLOT_IMAGE`
- `REX_WORKFLOW_GRAPHVIZ_IMAGE`
- `REX_WORKFLOW_IMAGEMAGICK_IMAGE`
- `REX_WORKFLOW_QPDF_IMAGE`
- `REX_WORKFLOW_POPPLER_IMAGE`

## Docker integration tests

The Rust Docker suite is opt-in so ordinary builds and CI jobs do not require a
daemon. With the native development images loaded, run:

```sh
cargo build -p rex-tool-ffmpeg -p rex-tool-gnuplot -p rex-tool-graphviz \
  -p rex-tool-imagemagick -p rex-tool-poppler -p rex-tool-qpdf
REX_WORKFLOW_DOCKER_TESTS=1 \
cargo test -p rex-tools-integration --test docker_tools -- \
    --nocapture --test-threads=1
```

Unset the variable or use `0`, `false`, `off`, or `no` to disable the suite.
Use `1`, `true`, `on`, or `yes` to enable it. Other values are rejected rather
than silently skipping tests because of a typo.

The suite covers all six tools, all output kinds, recursive CAS input/output
transfer, read-only inputs and image root, an invisible host sentinel, disabled
networking, fallback and CAS-supplied fonts, tool versus infrastructure
failures, missing images, cancellation cleanup, concurrent isolation, and
container leaks.

The **Tool images** workflow builds, but does not publish, on native
`ubuntu-24.04` amd64 and
`ubuntu-24.04-arm` arm64 runners. Docker smoke and integration tests are
controlled by the manual `run_integration_tests` checkbox or the repository
variable `REX_WORKFLOW_DOCKER_TESTS=1`.

The workflow has read-only repository permissions and contains no registry
login, push, or manifest-publication steps. When Rex is ready to publish tool
images, publication and a platform-aware digest lock can be added without
changing the workflow language or `ToolExecutionPlan` boundary.
