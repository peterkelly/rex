// Render aligned inline categories as a clustered bar chart.
import tools.gnuplot as G;

let
    chart = G.BarChart {
        arrangement = G.ClusteredBars,
        gap = 1.0,
        series = [
            G.BarSeries {
                title = Some "control",
                values = [
                    ("A", 4.0),
                    ("B", 6.0),
                    ("C", 5.0)
                ],
                fill = G.FillStyle {
                    color = Some "#0072b2",
                    opacity = 0.80
                }
            },
            G.BarSeries {
                title = Some "treated",
                values = [
                    ("A", 5.5),
                    ("B", 7.5),
                    ("C", 8.0)
                ],
                fill = G.FillStyle {
                    color = Some "#d55e00",
                    opacity = 0.80
                }
            }
        ]
    },
    plot = G.Plot2D {
        title = Some "Grouped observations",
        y_axis = G.Axis { label = Some "value" },
        series = [G.BarSeries2D chart]
    }
in
    G.render_svg
        (G.Figure { panels = [Some (G.Panel2D plot)] })
        G.SvgOptions {}
