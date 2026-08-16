# Rex workflow tool images

Rex can execute external workflow tools using either local processes or
isolated Docker containers. Workflows and `ToolExecutionPlan` values identify a
catalogued tool; they never select an executable path, image, mount, or Docker
option.

The current catalog has one image per `ToolBundle`:

| Bundle | Programs | Local development image |
|---|---|---|
| FFmpeg | `ffmpeg`, `ffprobe` | `rex-tool-ffmpeg:local` |
| Gnuplot | `gnuplot` | `rex-tool-gnuplot:local` |
| Graphviz | `dot` | `rex-tool-graphviz:local` |
| ImageMagick | `magick` and selected subcommands | `rex-tool-imagemagick:local` |
| QPDF | `qpdf` | `rex-tool-qpdf:local` |
| Poppler | `pdfinfo`, `pdftotext`, `pdftocairo`, `pdfimages` | `rex-tool-poppler:local` |

The list is deliberately closed today. It is not intended to be the final
registration mechanism once the catalog grows.

## Execution profiles

### Local processes

Local execution remains the default and uses tools installed on the host:

```sh
rex run workflow.rex
```

Use this profile for development when host tool discovery and the host
environment are acceptable.

### Docker with locally built images

Rex currently builds the six tool images on each machine and tags them
`rex-tool-*:local`. This gives amd64 and arm64 hosts native code without
requiring a public image registry. Image publication and a checked-in digest
lock are deliberately deferred until the tool catalog and Rex releases are
more mature.

Provision and diagnose a host once:

```sh
rex tools build
rex tools inspect
```

Then run a workflow:

```sh
rex run workflow.rex --tool-executor docker
```

Set `REX_TOOL_EXECUTOR=docker` to select the profile through the environment.
`rex tools build` uses the Dockerfiles embedded in the Rex binary and invokes
`docker buildx bake --load` for the Docker daemon's native architecture. The
executor always uses `--pull=never`, so evaluating a workflow never contacts a
registry or rebuilds an image as a side effect.

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
- uses a read-only image root, a PID limit, and the host's numeric UID/GID;
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

The normal one-command build works from any directory:

```sh
rex tools build
```

When developing the image definitions, build native images and run their smoke
suite from this directory:

```sh
docker buildx bake --load
./verify.sh
```

No platform override means BuildKit uses the daemon's native architecture.
The CLI uses these local tags whenever Docker execution is selected:

```sh
rex tools inspect
rex run workflow.rex --tool-executor docker
```

Embedders make the same policy explicit:

```rust,ignore
let images = DockerToolImages::development(
    "rex-tool-ffmpeg:local",
    "rex-tool-gnuplot:local",
    "rex-tool-graphviz:local",
    "rex-tool-imagemagick:local",
    "rex-tool-qpdf:local",
    "rex-tool-poppler:local",
);
let state = State::docker(store, images);
```

`DockerToolImages::development` makes the use of mutable local tags explicit at
the embedding boundary. `DockerToolImages::new` remains available for future
published or privately hosted images and requires digest-qualified references.
CLI image overrides also require digests unless `--allow-image-tags` is
supplied explicitly. The image override environment variables are:

- `REX_WORKFLOW_DOCKER_FFMPEG_IMAGE`
- `REX_WORKFLOW_DOCKER_GNUPLOT_IMAGE`
- `REX_WORKFLOW_DOCKER_GRAPHVIZ_IMAGE`
- `REX_WORKFLOW_DOCKER_IMAGEMAGICK_IMAGE`
- `REX_WORKFLOW_DOCKER_QPDF_IMAGE`
- `REX_WORKFLOW_DOCKER_POPPLER_IMAGE`

## Docker integration tests

The Rust Docker suite is opt-in so ordinary builds and CI jobs do not require a
daemon. With the native development images loaded, run:

```sh
REX_WORKFLOW_DOCKER_TESTS=1 \
cargo test -p rex-workflow --test docker_tools -- \
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
