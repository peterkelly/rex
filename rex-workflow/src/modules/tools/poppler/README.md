# Poppler tools for Rex

The workflow host exposes selected Poppler command-line utilities as `tools.poppler`. Function,
format, and option names follow the individual Poppler programs so agents can use the upstream
`pdfinfo`, `pdftotext`, `pdftocairo`, and `pdfimages` documentation directly.

```rex
P.Pdf { content = hash }
```

All programs are resolved through `PATH`. The host materializes CAS inputs in a private temporary
workspace and imports every declared output before returning. Rex code never supplies a host path.

## Functions

| Function | Poppler program | Result |
|---|---|---|
| `pdfinfo` | `pdfinfo -box -isodates` | Typed document metadata and page boxes |
| `pdftotext` | `pdftotext` | CAS-backed UTF-8 text, XHTML, bounding-box XHTML, or TSV |
| `pdftocairo` | `pdftocairo` | One vector file or ordered raster page files |
| `pdfimages` | `pdfimages` | CAS tree retaining mixed image filenames and extensions |
| `pdfimages_list` | `pdfimages -list` | Typed image-object inventory |
| `version` | `pdfinfo -v` | Installed Poppler version |

Raster `pdftocairo` output normally returns `CairoPageFiles`; `single_file` returns only the first
selected page as `CairoSingleFile`. PDF and PostScript are single artifacts. SVG and EPS require
one explicitly selected page. Crop, resolution, scaling, color, transparency, antialias, JPEG, and
TIFF settings correspond to Poppler flags.

`pdfimages` returns one CAS tree because a single invocation may create a heterogeneous collection
of JPEG, JPEG 2000, JBIG2, CCITT, PNG, TIFF, and mask files. Preserving the filenames and extensions
also preserves relationships between auxiliary files. Use `storage.get_tree` to inspect the tree.

Invalid requests and expected nonzero exits are returned as `Err PopplerError`. Missing binaries,
storage failures, and executor failures remain host evaluation errors.

Complete, typechecked workflows are in [`examples/poppler`](../../../../examples/poppler/README.md).
