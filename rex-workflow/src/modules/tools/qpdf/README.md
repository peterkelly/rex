# QPDF tools for Rex

The workflow host exposes QPDF as `tools.qpdf`. Names and behavior follow the QPDF command-line
documentation so an agent can apply existing QPDF knowledge without translating through a generic
PDF abstraction. The module owns command ordering, temporary paths, and content-addressed storage.

```rex
import artifacts (Pdf);

Pdf { content = hash }
```

`qpdf` is resolved through `PATH`. Input and output paths are never supplied by Rex code. Every
generated PDF and JSON document is imported into the CAS before the function returns.

## Functions

| Function | QPDF operation | Result |
|---|---|---|
| `check` | `--check` | Parsed clean/warning/error status and diagnostics |
| `show_npages` | `--show-npages` | Page count |
| `json` | `--json` version 2 | CAS-backed JSON plus warnings |
| `transform` | ordinary input/output rewrite | One CAS-backed PDF plus warnings |
| `pages` | `--pages ... --` | Selected, reordered, collated, or merged PDF |
| `split_pages` | `--split-pages` | Ordered CAS-backed PDF files |
| `overlay` | `--overlay` | One overlaid PDF |
| `underlay` | `--underlay` | One underlaid PDF |
| `version` | `--version` | Installed QPDF version |

`WriteOption` uses QPDF terminology for linearization, stream and object-stream policies,
compression, content normalization, rotation, annotation flattening, version selection, IDs,
decryption, restrictions, and AES encryption. Options remain ordered because QPDF option groups
have ordering rules. Passwords are dedicated fields rather than raw arguments.

`pages` accepts `PageSource` values whose `range` is QPDF page-range syntax, including forms such
as `1-5`, `z-1`, and `1-z:odd`. Its optional `collate` list maps to QPDF's `--collate=n[,m,...]`
page-group counts. `overlay` and `underlay` similarly retain QPDF's `to`, `from`, and `repeat` page
mappings. The wrapper validates these strings conservatively and does not expose raw shell or path
arguments.

QPDF exit status 3 is a successful write with recoverable warnings. Those warnings are preserved
on `PdfOutput`, `PdfSequenceOutput`, and `JsonOutput`. `check` treats QPDF's documented structural
error status as data. Invalid requests and other expected process failures are returned as
`Err QpdfError`; missing executables and CAS/executor failures remain host evaluation errors.

Complete, typechecked workflows are in [`examples/qpdf`](../../../../examples/qpdf/README.md).
