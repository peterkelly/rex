# Gnuplot workflow examples

These workflows construct semantic figures from inline Rex values and render SVG artifacts through
`tools.gnuplot`. They never submit raw gnuplot commands, paths, column expressions, or table files.

- `curves.rex` covers lines, points, lines-and-points, steps, and impulses.
- `error_bars.rex` covers symmetric and explicit error bounds.
- `confidence_band.rex` covers a filled interval band overlaid by a curve.
- `bar_chart.rex` covers clustered categorical bar series.
- `histogram.rex` covers a statistical histogram of inline samples.
- `heatmap.rex` covers a rectangular grid with an explicit palette.
- `vectors.rex` covers a two-dimensional vector field.
- `labels.rex` covers data-driven labels and fixed text, arrow, and reference annotations.
- `point_cloud_3d.rex` covers a three-dimensional point cloud.
- `path_3d.rex` covers a segmented three-dimensional path.
- `surfaces_3d.rex` covers wireframe, colored, line-contour, and filled-contour surfaces.

Run an example from the workspace root:

```sh
cargo run -p rex-workflow -- --store-path ./store run \
  rex-workflow/examples/gnuplot/curves.rex
```

Replace the final path with any other example. The result is a `std.artifacts.Image` whose `content`
field is the CAS hash of the generated SVG.
