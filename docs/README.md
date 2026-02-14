# Rex Documentation

This directory contains the documentation for Rex, built with [mdBook](https://rust-lang.github.io/mdBook/).

## Building the Documentation

### Prerequisites

Install mdBook:

```sh
cargo install mdbook
```

### Build

To build the documentation:

```sh
cd docs
mdbook build
```

The generated HTML will be in `docs/book/`.

### Serve Locally

To serve the documentation locally with auto-reload:

```sh
cd docs
mdbook serve
```

Then open http://localhost:3000 in your browser.

### Watch Mode

mdBook automatically rebuilds when source files change in serve mode. You can also use:

```sh
mdbook watch
```

## Directory Structure

- `book.toml` — mdBook configuration
- `src/` — Documentation source files (Markdown)
  - `SUMMARY.md` — Table of contents
  - `README.md` — Introduction/homepage
  - `tutorial/` — Tutorial chapters organized into 3 sections
  - Reference documents: `LANGUAGE.md`, `SPEC.md`, `ARCHITECTURE.md`, `MEMORY_MANAGEMENT.md`, `EMBEDDING.md`, `CONTRIBUTING.md`
- `book/` — Generated HTML output (gitignored)
- `.gitignore` — Git ignore rules

## Documentation Structure

The documentation is organized into:

- **Introduction** — Overview and getting started
- **Tutorial** — Three sections covering basics, advanced topics, and worked examples (36 chapters total)
- **Reference** — Language reference, specification, architecture, and contribution guidelines

## Syntax Highlighting

Rex code blocks are marked with ````rex` in the markdown files. mdBook displays them as formatted code blocks. Custom syntax highlighting for the Rex language can be added in the future by creating a highlight.js language definition and configuring it in the mdBook theme.

## Theme

The documentation uses a light theme by default with the Ayu dark theme available via the theme selector in the navigation bar. All mdBook themes are available for users to choose from.
