# QPDF workflow examples

These programs use `tools.qpdf` with PDF blobs already imported into the workflow store. Import a
PDF and place the printed hash in `inputs.json`:

```sh
cargo run -p rex-workflow -- --store-path ./store store import document.pdf
cargo run -p rex-workflow -- --store-path ./store run \
  rex-workflow/examples/qpdf/check.rex --inputs inputs.json
```

For one-input examples use `{"input":"<hash>"}`. The merge example expects
`{"first":"<hash>","second":"<hash>"}`. Generated PDFs and JSON remain in the CAS; use the
workflow store commands or `storage` module to retrieve their hashes.

- `check.rex` checks PDF structure without rewriting it.
- `linearize.rex` performs a web-optimized deterministic rewrite.
- `merge_pages.rex` combines page ranges using QPDF's `--pages` model.
- `json.rex` exports selected QPDF JSON version 2 sections.
- `version.rex` reports the QPDF executable selected through `PATH`.
