# Poppler workflow examples

These programs use `tools.poppler` with a PDF already imported into the workflow store. Import a
PDF and place the printed hash in `inputs.json` as `{"input":"<hash>"}`, then run an example:

```sh
cargo run -p rex --bin rex -- --store-path ./store store import document.pdf
cargo run -p rex --bin rex -- --store-path ./store run \
  examples/poppler/pdfinfo.rex --inputs inputs.json
```

- `pdfinfo.rex` parses metadata and page geometry.
- `pdftotext.rex` produces geometry-preserving UTF-8 TSV in the CAS.
- `render_png.rex` renders pages to ordered PNG blobs at 144 DPI.
- `extract_images.rex` preserves extracted native images in a CAS tree.
- `version.rex` reports the Poppler version selected through `PATH`.

Use `std.storage.get_string` for text output, `std.storage.get_bytes` for rendered files, or
`std.storage.get_tree` to inspect extracted images in a larger workflow.
