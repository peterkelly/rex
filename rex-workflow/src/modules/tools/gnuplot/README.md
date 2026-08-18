# Gnuplot tools for Rex

The workflow host exposes semantic plotting as `tools.gnuplot`. Rex programs construct immutable
figures from inline values and call `render_png`, `render_svg`, or `render_pdf`. The generated
gnuplot program is private: the API accepts no raw commands, column expressions, host paths, or
file-backed tables.

## Semantic model

- A `Figure` owns a theme and a regular grid of optional `Panel2D` and `Panel3D` values.
- A `Plot2D` owns axes, color scale, grid, legend, palette, annotations, and an ordered list of
  typed layers. Layers cover curves, error bars, uncertainty bands, categorical bars, statistical
  histograms, heatmaps, vectors, and data-driven labels.
- A `Plot3D` owns the corresponding three-dimensional settings and supports point clouds,
  segmented paths, wireframes, colored surfaces, contour lines, and filled contours.
- Numeric samples, categories, grids, errors, and labels are values inside their series. The API
  deliberately has no shared `std.artifacts.Table` input at this stage.
- Numeric and timestamped x data are distinct constructors, so their domains cannot be mixed
  accidentally. Disconnected paths and bands are represented explicitly as lists of segments.
- Output options describe the artifact, not gnuplot terminal syntax. Rendering returns the shared
  `std.artifacts.Image` or `std.artifacts.Pdf` type.

Defaults are registered for figure, panel-setting, series-setting, and output-option records. Rex
code can therefore specify only fields that differ from the semantic defaults. Color ranges are
panel-level `color_axis` settings because every color-mapped layer in a panel shares one scale.

## Validation and compilation

Before starting gnuplot, the host rejects empty datasets, non-finite coordinates, invalid ranges
and logarithmic bases, mismatched bar categories, malformed error bounds, non-rectangular grids,
invalid palette stops, incompatible numeric/timestamp domains, and unsupported series
combinations. Histograms are binned deterministically by Rex's host module rather than delegated
to mutable gnuplot state.

The compiler serializes data into private gnuplot data blocks, safely quotes all text, creates a
single multiplot program, and declares one output slot in the tool execution plan. Workflow code
cannot select an executable, image, terminal command, mount, or output path.

## Rendering

```rex
import tools.gnuplot as G;

G.render_svg
    (G.Figure {
        panels = [
            Some (G.Panel2D (G.Plot2D {
                series = [
                    G.CurveSeries (G.Curve2D {
                        data = G.NumericXY [(0.0, 0.0), (1.0, 1.0)],
                        mode = G.LinesPoints
                    })
                ]
            }))
        ]
    })
    G.SvgOptions {}
```

Rendering returns `Result Image GnuplotError` or `Result Pdf GnuplotError`. `InvalidFigure`
reports semantic validation failures, `ProcessFailed` carries gnuplot diagnostics, and
`UnexpectedOutput` reports an invalid runtime result. Storage and executor failures remain Rex
evaluation errors.

Complete, typechecked workflows covering every layer kind are in
[`examples/gnuplot`](../../../../examples/gnuplot/README.md).
