# 🦖 Rex

[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![docs](https://img.shields.io/badge/docs-online-blue)](https://peterkelly.github.io/rex/)
[![crates.io](https://img.shields.io/crates/v/rex.svg)](https://crates.io/crates/rex)

<p align="center">
  <img src="logo.jpg" width="400" alt="Rex logo">
</p>

Rex (short for *[Rush](https://rush.cloud/) Expressions*) is a statically typed,
pure functional workflow language. It is designed for
scientific computing and data processing: work in Rex is expressed as pure
transformations over immutable values, while typed tool modules delegate
work to external programs for compute-intensive tasks.

The workflow runtime connects four ideas that are particularly useful when
processing scientific data:

- **A real functional language** makes control flow, data flow, reuse, and
  error handling part of one small, expressive language rather than a mixture
  of YAML, shell, and application-specific configuration.
- **Content-addressable storage** identifies every stored input and output artifact
  by its [BLAKE3](https://en.wikipedia.org/wiki/BLAKE_(hash_function)) hash. Files
  and directory trees are immutable values, so intermediate artifacts can be passed
  between tools without shared filenames or mutable working directories.
- **Typed tool APIs** expose domain concepts such as video codecs, PDF structure,
  image operations, and output formats. Rex programs construct valid tool
  requests rather than assembling shell command strings.
- **Mandatory OCI execution** runs every external tool in a fresh, locked-down
  container containing only its declared inputs. Docker is the supplied
  backend; a provider-neutral job protocol supports future remote runtimes.

Together these properties make workflow definitions concise, inspectable, and
amenable to parallel execution. They also create a clean boundary between the
logic of an analysis and the operating-system processes that carry it out.

Rex is also useful as a target for LLM-generated workflows: static types give
fast, high-signal feedback, pure code is easier to inspect, and each installed
tool's closed API sharply limits what generated programs can ask the host to execute.
See [LLM guidance](docs/src/LLMS.md) for syntax and validation advice.

> **Project status:** the `main` branch contains the work in progress toward
> Rex v4 and is currently versioned as 3.9.x. `rex-workflow` is new and under
> active development. The older production release of the core Rex language
> is available at [talo/rex](https://github.com/talo/rex).

## Why a functional language for workflows?

Many workflow systems begin with a directed acyclic graph and gradually grow
their own expression syntax, templates, conditionals, loops, and plugin model.
Rex starts with a small general-purpose language instead. It provides
Hindley–Milner type inference, algebraic data types, records, pattern matching,
parametric polymorphism, type classes, higher-order functions, recursion, and
modules.

That matters for scientific and data-processing work because real pipelines
rarely remain a static sequence of commands. They need to map an analysis over
a cohort, group observations, branch on metadata, preserve domain-specific
failure information, combine several tools, and package reusable methods.
Those operations are natural in a functional program:

```rex
let
    observations = [3.0, -1.0, 12.0, 7.0, 20.0],
    selected = filter (\value -> value >= 0.0) observations,
    normalized = map (\value -> value / 20.0) selected
in
    foldl (\total value -> total + value) 0.0 normalized
```

Rex uses strict evaluation, but expressions and functions are pure: their
meaning does not depend on hidden mutable state in the language. This gives
the evaluator freedom to run independent asynchronous calls concurrently
without making users manage threads, futures, locks, async/await syntax,
or callback graphs. Sequential dependencies are expressed by passing one
result into the next; independent work remains independent in the source.

Purity also improves reviewability. A function's arguments describe the data
it can use, its result type describes what it can produce, and an algebraic
data type can enumerate every expected outcome. Tool modules preserve this
model by returning ordinary typed values such as:

```rex
Result Media FF.FfmpegError
Result Q.PdfOutput Q.QpdfError
Result P.TextFile P.PopplerError
```

Expected invalid requests and tool-process failures can therefore be matched
and handled inside the workflow. Storage failures, executor failures, and
other infrastructure problems remain evaluation errors, keeping domain
failures distinct from failures of the runtime itself.

### Static types catch workflow wiring mistakes early

Tool options are represented by records and algebraic data types rather than
unstructured maps. Once a hash is wrapped in a semantic artifact type, an
`Image` cannot accidentally be supplied where a `Media` is expected; a
codec option cannot be confused with an image operation; and a multi-file
result must be handled as such. Raw imported hashes still have to be classified
correctly by the workflow, and a tool reports an error if the stored bytes are
not valid input. The compiler catches structural wiring errors before launching
an expensive external process.

Types are especially valuable when workflows are generated or modified by
software. An LLM or another program can propose a Rex workflow, run the parser
and type checker, and use precise diagnostics to repair it before any tool is
executed.

### Functional composition scales beyond a DAG file

Rex workflows can factor repeated logic into functions, define domain types,
transform collections with `map` and folds, use recursion for hierarchical
data, and preserve structured errors across tool boundaries. The result is a
program that can grow with an analysis instead of a configuration file that
eventually needs an external templating language.

## Content-addressable data

Scientific workflows are easier to reason about when artifacts are values,
not mutable locations. `rex::storage` provides the content-addressable store
used by `rex-workflow`, in which every object is named by the BLAKE3 hash of
its bytes.

The data model has two object kinds:

- A **blob** is an opaque byte sequence: an image, video, PDF, table, model,
  log, or any other file.
- A **tree** is a deterministically encoded map from names to blob or tree
  entries. Trees represent directories and multi-file datasets, extracted
  image collections, and nested results.

Each tree entry records its kind, hash, and size. Trees can contain other trees,
so one root hash identifies a complete immutable directory hierarchy. The sizes
are cumulative, meaning that an entry that refers to another tree includes the
total size of everything it contains.

```text
host file or directory
        |
        |  rex store import
        v
  BLAKE3 blob/tree hash
        |
        |  typed Rex values and tool calls
        v
  new blob/tree hashes
        |
        |  rex store export
        v
host file or directory
```

This model provides several useful properties:

- **Stable identity.** The same bytes always produce the same hash, regardless
  of their original filename or machine.
- **Immutability.** Existing inputs cannot be overwritten. A transformation
  creates a new object and returns a new hash.
- **Deduplication.** Writing content that is already present resolves to the
  existing address instead of creating a second logical object.
- **Unambiguous handoff.** A tool consumes an exact object and returns the
  exact identities of its outputs. There is no question about which revision
  of a path was read.
- **Natural composition.** An output hash from one tool is immediately usable
  by another tool without exporting and re-importing an intermediate file.
- **Portable storage.** The same API can use a local filesystem store, an
  in-memory store, or a cloud-hosted S3 bucket.

The CAS is deliberately aligned with the functional language: creating a new
artifact does not mutate an old artifact, and calling `put` with the same
content returns the same value. A directory update is represented by creating
new trees along the changed path, much like Git's object model.

Content addressing is an important foundation for caching and provenance, but
it is not magic. A hash identifies bytes; it does not by itself record which
workflow, parameters, tool version, or container produced them. Likewise, an
external tool may be nondeterministic even when its inputs are immutable. Rex
makes the artifact boundary explicit so that caching and provenance can be
built rigorously instead of inferred from mutable paths.

### Store operations available to Rex programs

The built-in `std.storage` module exposes immutable data directly:

```rex
import std.storage (*);

let
    report = put_string "analysis complete",
    files = dict_from_entries [("report.txt", (Blob, report))],
    result_tree = put_tree files
in
    result_tree
```

Programs can use `put_string`, `put_bytes`, `put_tree`, `get_string`,
`get_bytes`, and `get_tree`. The shared `std.artifacts` module wraps hashes with
semantic meaning through `Media`, `Image`, `Pdf`, and `JsonFile`; tool-specific
result types add operation metadata where needed.

```rex
import std.artifacts (Pdf);

fn as_pdf (content: Hash) -> Pdf = Pdf { content = content };
```

Constructing an artifact classifies a CAS blob but does not inspect its bytes.
The consuming tool remains responsible for validating that the stored content
has a supported representation.

## Tools are typed capabilities

Rex does not expose a general shell command to workflow programs. Instead, the
host registers modules whose functions and types describe supported
operations. The current workflow catalog contains:

| Rex module | Runtime programs | Selected capabilities |
|---|---|---|
| `tools.ffmpeg` | FFmpeg, FFprobe | Transcode and remux media, extract audio or frames, create thumbnails, concatenate, mux, segment, package HLS/DASH, probe metadata, inspect packets and frames, and query capabilities |
| `tools.gnuplot` | Gnuplot | Render typed figures from inline curves, error bars, bands, bars, histograms, heatmaps, vectors, labels, point clouds, paths, surfaces, and annotations |
| `tools.graphviz` | Graphviz `dot` | Render semantic directed or undirected graphs with typed attributes, declared nodes, binary edges, ports, labels, defaults, and nested subgraphs |
| `tools.imagemagick` | ImageMagick | Generate and transform images, batch-convert, identify, compare, composite, montage, extract pixels, and query formats and capabilities |
| `tools.qpdf` | QPDF | Check PDFs, count pages, export structured JSON, transform or linearize, merge/split pages, and apply overlays or underlays |
| `tools.poppler` | `pdfinfo`, `pdftotext`, `pdftocairo`, `pdfimages` | Parse PDF metadata, extract text and word geometry, render pages, extract images, and inspect embedded images |

The narrow functions cover common operations, while FFmpeg and ImageMagick
also provide typed general render APIs for complex filter graphs, multiple
inputs, ordered image operations, and multiple outputs.

Each module is implemented by an independently installed Rust binary named
`rex-tool-NAME`. `rex-workflow` reads its typed manifest when `tools.NAME` is
imported and invokes its `execute` command for calls. The bundled modules
compile requests into `ToolExecutionPlan` values. Plans can reference only:

- an OCI program selected by the installed Rust tool implementation;
- literal arguments generated by the typed module;
- declared CAS blob or tree inputs;
- declared output slots and output kinds; and
- optional standard input sourced from the CAS.

A Rex workflow cannot choose a host executable path, container image, host mount,
working directory, or arbitrary Docker option. Those remain host policy. This
separation is useful for untrusted or generated workflows because adding a new
operation is an explicit Rust API decision rather than an accidental extension
of shell access.

Tool outputs are also checked at the boundary. The executor knows whether to
expect a single file, a numbered sequence, a directory, or a tree. It rejects
symbolic links and special files before recursively importing output into the
CAS.

## OCI container execution

Every external tool invocation is an executor-neutral OCI job. Docker is the
only backend shipped in this repository and is selected automatically:

```sh
rex --store-path ./store --tool-dir /opt/rex/tools run workflow.rex \
    --inputs inputs.json
```

The supplied container executor is intentionally restrictive. For every
invocation it:

- materializes only the plan's declared CAS inputs;
- mounts `/work/inputs` read-only and `/work/outputs` read-write;
- uses a size-limited `noexec,nosuid,nodev` tmpfs for `/work/tmp`;
- does not mount the CAS, repository, current directory, home directory,
  Docker socket, system fonts, or arbitrary host paths;
- disables networking, Linux capabilities, privilege escalation, and health
  checks;
- uses a read-only image root, memory, CPU, PID, and temporary-storage limits,
  and a non-root numeric UID and GID;
- supplies a headless C locale, UTC timezone, temporary home, and controlled
  cache locations;
- bounds execution time and captured stdout/stderr; and
- explicitly removes the container on completion, timeout, or cancellation.

The bundled tool images further narrow the available surface. The FFmpeg API
does not expose network sources, capture devices, or hardware acceleration.
ImageMagick includes a defence-in-depth policy that disables network,
desktop/capture/print, indirect-path, MSL, and MVG coders while preserving the
documented headless formats.

Containers are useful for both isolation and repeatability. They make the tool
runtime, libraries, codecs, delegates, fonts, locale, and operating-system
environment an explicit deployment choice rather than an undocumented property
of a worker. `OciToolImages::new` requires digest-qualified image references,
and CLI image overrides require digests unless mutable tags are explicitly
allowed.

The repository currently builds development images locally with the tags
`rex-tool-ffmpeg:local`, `rex-tool-gnuplot:local`, `rex-tool-graphviz:local`, `rex-tool-imagemagick:local`,
`rex-tool-qpdf:local`, and `rex-tool-poppler:local`. The executor uses
`--pull=never`, so evaluating a workflow never contacts a registry or rebuilds
an image as a side effect. Published multi-platform images and a release digest
lock are future work; today, production users should supply and pin their own
images if they require a controlled image supply chain.

See [the tool image documentation](rex-workflow/tool-images/README.md) for the
full isolation contract, image overrides, build details, and integration-test
coverage.

## Quick start

You need a recent Rust toolchain and Docker with Buildx. Installed Rust tool
binaries run as protocol adapters on the host; the external programs they wrap
still run only inside OCI containers.

Build the workflow binary from the workspace root and create a local store:

```sh
cargo build -p rex-workflow \
  -p rex-tool-ffmpeg -p rex-tool-gnuplot -p rex-tool-graphviz \
  -p rex-tool-imagemagick -p rex-tool-poppler -p rex-tool-qpdf
mkdir -p store
```

Build and inspect the six native-architecture Docker tool images:

```sh
target/debug/rex tools build
target/debug/rex tools inspect
```

Run an input-free example that generates a PNG gradient with ImageMagick:

```sh
target/debug/rex --store-path ./store --tool-dir target/debug run \
    rex-workflow/examples/imagemagick/generate_gradient.rex
```

The JSON result contains the BLAKE3 `content` hash of the generated image.
Its shape is:

```json
{
  "Ok": {
    "SingleImage": [
      { "content": "<content-hash>" }
    ]
  }
}
```

Export that blob back to a conventional file when it needs to leave the
workflow system:

```sh
target/debug/rex --store-path ./store store export \
    <content-hash> gradient.png
```

To process an existing file, import it first:

```sh
target/debug/rex --store-path ./store store import photo.jpg
```

The command prints the content hash. Put that hash in `inputs.json`:

```json
{
  "input": "<photo-hash>"
}
```

Then run the resize example:

```sh
target/debug/rex --store-path ./store --tool-dir target/debug run \
    rex-workflow/examples/imagemagick/resize.rex \
    --inputs inputs.json
```

The CLI looks for tools beside its own executable by default. Set `REX_STORE`
and `REX_TOOL_DIR` to avoid passing the store path or to select another
installed-tool directory. OCI image
overrides use `--ffmpeg-image`, `--gnuplot-image`, `--graphviz-image`,
`--imagemagick-image`, `--qpdf-image`, and `--poppler-image`.

### CLI overview

The workflow binary is named `rex`:

```text
rex [--store-path PATH] store import PATH
rex [--store-path PATH] store export HASH PATH
rex [--store-path PATH] store cat HASH[/PATH]
rex [--store-path PATH] store ls HASH[/PATH]
rex [--store-path PATH] store resolve-path HASH[/PATH]

rex [--store-path PATH] [--tool-dir DIRECTORY] run FILE [--inputs JSON] [--raw-output]
    [--*-image OCI_REFERENCE] [--allow-image-tags]

rex tools build
rex tools inspect
rex tools cleanup [--include-running]
```

`run` reads a `.rex` program, parses and typechecks it, converts a flat JSON
object into the typed parameters of `main`, evaluates the workflow, and renders
the result as JSON. `--raw-output` prints a string result without JSON string
quoting, which is useful for report-generating workflows.

`store import` and `store export` work with both files and directory trees.
`cat`, `ls`, and `resolve-path` accept a root hash followed by slash-separated
tree components, allowing stored datasets to be inspected without exporting
the whole hierarchy.

## Workflow examples

The repository includes a broad set of examples. Every tool example is parsed
and typechecked by the `rex-workflow` test suite.

- [FFmpeg examples](rex-workflow/examples/ffmpeg/README.md): generated video
  and audio, transcoding, stream copying, probing, inspection, muxing,
  concatenation, filtering, frame extraction, segmentation, and HLS/DASH.
- [Gnuplot examples](rex-workflow/examples/gnuplot/README.md): curves, error
  bars, bands, categorical bars, histograms, heatmaps, vectors, labels, point
  clouds, paths, and surface representations built from inline values.
- [Graphviz examples](rex-workflow/examples/graphviz/README.md): typed DOT graph
  construction, subgraph composition, and SVG rendering.
- [ImageMagick examples](rex-workflow/examples/imagemagick/README.md): image
  generation, resizing, thumbnails, conversion, metadata, drawing,
  composition, comparison, montage, frame extraction, batch processing, and
  raw pixels.
- [QPDF examples](rex-workflow/examples/qpdf/README.md): validation, JSON
  inspection, linearization, and page merging.
- [Poppler examples](rex-workflow/examples/poppler/README.md): metadata, text
  and word geometry, page rendering, and image extraction.
- [Combined ImageMagick and FFmpeg
  examples](rex-workflow/examples/imagemagick_ffmpeg/README.md): multi-stage
  media workflows with direct CAS-backed handoff.
- [Storage examples](rex-workflow/examples/storage): recursive traversal and
  rendering of immutable directory trees.

Most example files contain their required input shape, exact CLI invocation,
and result description in a header comment.

## Execution architecture

A workflow run crosses several deliberately small boundaries:

1. **Parse.** `rex-parser` turns source into a `CompilationUnit`.
2. **Typecheck.** `rex-typesystem` infers and checks the program, its imports,
   `main` inputs, tool options, and result.
3. **Decode inputs.** The CLI converts JSON fields into the concrete Rex types
   declared by `main`.
4. **Evaluate.** `rex-engine` evaluates pure expressions and schedules injected
   asynchronous functions. The runtime state contains a CAS implementation and
   an OCI tool executor.
5. **Invoke the installed module.** `rex-workflow` converts arguments with the
   declared types and calls the module binary's `execute` command.
6. **Compile and build the OCI job.** The Rust tool converts semantic values
   into a `ToolExecutionPlan` with explicit input and output slots and resolves
   it to an image, fixed command,
   target platform, isolation policy, and resource limits.
7. **Execute.** The selected OCI backend transfers declared CAS inputs, runs the
   job, and returns only declared results plus execution provenance.
8. **Capture artifacts.** Declared outputs are validated, recursively imported
   into the CAS, and returned to Rex as hashes wrapped in semantic types.
9. **Encode the result.** The final typed Rex value is converted to JSON for the
   caller.

This separation makes policy replaceable. The workflow language describes
what to compute; the host decides where bytes live, which tool implementation
is trusted, how it is isolated, and which operational limits apply.

## Embedding workflows in Rust

`rex-workflow` is a library as well as a CLI. An embedding application chooses
its store, OCI images, and executor, then evaluates Rex source:

```rust,ignore
use rex_workflow::{
    run::eval_rex,
    state::State,
};
use rex::storage::Store;

use rex_workflow::modules::tools::executor::{OciPlatform, OciToolImages};

let store = Store::new_with_filesystem("./store".into());
let images = OciToolImages::new(
    OciPlatform::new("linux", "amd64", None)?,
    "registry.example/ffmpeg@sha256:<digest>",
    "registry.example/gnuplot@sha256:<digest>",
    "registry.example/graphviz@sha256:<digest>",
    "registry.example/imagemagick@sha256:<digest>",
    "registry.example/qpdf@sha256:<digest>",
    "registry.example/poppler@sha256:<digest>",
);
let state = State::docker(store, images);
let inputs = serde_json::json!({ "input": input_hash });
let result = eval_rex(source, Some(inputs), state).await?;
```

Stores can instead be in memory or backed by an `object_store::ObjectStore`.
Embedders can implement `OciJobExecutor` to route the same logical jobs to a
remote container service. The shared validator requires compatible platforms,
resource controls, isolation guarantees, CAS transfer, and provenance; it does
not permit a host-process implementation. See
[OCI executor protocol](docs/src/OCI_EXECUTORS.md).

Host applications can also use the core `rex` crate to
inject their own typed native modules. This is how domain-specific scientific
software can become a controlled Rex capability without becoming a shell
command.

## Rex as an embedded language

The workflow system is built on the general Rex embedding API. Outside
`rex-workflow`, a Rust application can parse and typecheck user programs,
inject synchronous or asynchronous native functions, and evaluate them with
application-defined state. The `#[derive(Rex)]`, `#[rex::export]`, and
`#[rex::module]` macros bridge documented Rust types and APIs into Rex.

This is useful when a workflow needs more than the built-in media and PDF
tools. A scientific host can expose typed operations for a simulator,
instrument, database, model runner, or cluster service while retaining Rex's
pure orchestration model. The host owns effects and policy; Rex owns the typed
composition of results.

For embedding patterns and the untrusted-code checklist, see
[Embedding Rex](docs/src/EMBEDDING.md).

## Language and tooling

Rex can also be used independently of `rex-workflow` for pure computation or
as an embedded scripting language. Try it in the
[browser playground](https://peterkelly.github.io/rex/) or run the standalone
language CLI:

```sh
cargo run -p rex-cli --bin rex_cli -- -c \
    'map (\n -> n * n) [1, 2, 3, 4]'
```

The repository includes:

- an LSP server and VS Code extension;
- a browser runtime compiled to WebAssembly;
- a standard prelude with collection operations, type classes, and common
  functional abstractions;
- a tutorial, language reference, formal semantics, architecture notes, and
  embedding guide; and
- fuzz targets and regression suites for the parser, type system, and runtime.

Documentation is available at
[peterkelly.github.io/rex](https://peterkelly.github.io/rex/). Useful starting
points include the [tutorial](docs/src/tutorial/README.md), [language
reference](docs/src/LANGUAGE.md), [semantics](docs/src/SPEC.md), and
[architecture](docs/src/ARCHITECTURE.md).

## Workspace crates

This repository is a Cargo workspace. Its main crates are:

- `rex-workflow`: workflow runtime, installable-tool protocol, OCI executor
  protocol, Docker backend,
  and the workflow `rex` CLI.
- `rex`: entry point for embedding the core language in Rust applications,
  including content-addressable storage with memory, filesystem, and
  `object_store` backends.
- `rex-parser`: parser producing a `CompilationUnit { decls, body }`.
- `rex-ast`: shared syntax tree nodes, symbols, and spans.
- `rex-typesystem`: Hindley–Milner inference, ADTs, higher-kinded types, and
  type classes.
- `rex-engine`: typed evaluator, asynchronous native-function injection, and
  the standard prelude.
- `rex-proc-macro`: `#[derive(Rex)]`, `#[rex::export]`, and `#[rex::module]`
  bridges between Rust and Rex.
- `rex-tool-ffmpeg`, `rex-tool-gnuplot`, `rex-tool-graphviz`,
  `rex-tool-imagemagick`, `rex-tool-poppler`, and `rex-tool-qpdf`: separately
  installable typed tool binaries.
- `rex-cli`: standalone language CLI.
- `rex-lsp` and `rex-vscode`: language server and editor extension.
- `rex-wasm` and `rex-mdbook`: browser runtime and interactive documentation.
- `rex-fuzz`: stdin-driven fuzz harnesses.
- `rex-util`: shared import, hashing, and module helpers.

## Building and testing

Run the workspace checks from the repository root:

```sh
cargo test
cargo fmt --check
cargo clippy --tests
```

The Docker integration suite is opt-in so normal builds do not require a
daemon. After building the local images, run:

```sh
cargo build -p rex-tool-ffmpeg -p rex-tool-gnuplot -p rex-tool-graphviz \
  -p rex-tool-imagemagick -p rex-tool-poppler -p rex-tool-qpdf
REX_WORKFLOW_DOCKER_TESTS=1 \
cargo test -p rex-workflow --test docker_tools -- \
    --nocapture --test-threads=1
```

The suite covers all six tool bundles, recursive blob/tree transfer, output
validation, read-only inputs and image roots, disabled networking, missing
images, tool and infrastructure failures, cancellation cleanup, concurrent
isolation, and container leaks.

## Design boundaries and current scope

Rex is intentionally not a shell wrapper. The set of installed tool modules is
open, but every module exposes a closed, typed API; host paths are absent from
tool APIs, and Docker policy is not controlled by workflow source. Installing a
tool binary is therefore an explicit trust decision rather than arbitrary
command execution from Rex code.

The current `rex-workflow` implementation supplies Docker and an in-memory fake
remote boundary for conformance testing. It is not yet a distributed scheduler
or cloud executor. Provider adapters must implement the OCI job protocol and
pass the complete conformance suite before they can be selected.

Made with ❤️ by [QDX](https://qdx.co/)
