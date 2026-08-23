// Bin inline samples into a probability histogram.
import tools.gnuplot as G;

let
    histogram = G.Histogram {
        samples = [
            0.2, 0.4, 0.5, 0.7, 0.8,
            1.0, 1.1, 1.2, 1.2, 1.4,
            1.5, 1.7, 1.8, 2.0, 2.3,
            2.4, 2.5, 2.7, 2.9, 3.0
        ],
        title = Some "samples",
        bins = G.BinCount 8,
        normalization = G.HistogramProbability,
        fill = G.FillStyle {
            color = Some "#009e73",
            opacity = 0.75,
            border = true
        }
    },
    plot = G.Plot2D {
        title = Some "Sample distribution",
        x_axis = G.Axis { label = Some "value" },
        y_axis = G.Axis { label = Some "probability" },
        series = [G.HistogramSeries histogram]
    }
in
    G.render_svg
        (G.Figure { panels = [Some (G.Panel2D plot)] })
        G.SvgOptions {}
