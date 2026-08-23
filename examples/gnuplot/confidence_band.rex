// Overlay a fitted curve on a filled confidence interval.
import tools.gnuplot as G;

let
    band = G.Band2D {
        data = G.BandPoints [
            G.BandPoint2D { x = 0.0, lower = 0.7, upper = 1.3 },
            G.BandPoint2D { x = 1.0, lower = 1.5, upper = 2.3 },
            G.BandPoint2D { x = 2.0, lower = 2.2, upper = 3.4 },
            G.BandPoint2D { x = 3.0, lower = 3.0, upper = 4.6 },
            G.BandPoint2D { x = 4.0, lower = 3.7, upper = 5.7 }
        ],
        title = Some "95% interval",
        fill = G.FillStyle {
            color = Some "#56b4e9",
            opacity = 0.30
        }
    },
    fit = G.Curve2D {
        data = G.NumericXY [
            (0.0, 1.0),
            (1.0, 1.9),
            (2.0, 2.8),
            (3.0, 3.8),
            (4.0, 4.7)
        ],
        title = Some "fit",
        line = G.LineStyle {
            color = Some "#0072b2",
            width = 2.0
        }
    },
    plot = G.Plot2D {
        title = Some "Confidence band",
        series = [
            G.BandSeries band,
            G.CurveSeries fit
        ]
    }
in
    G.render_svg
        (G.Figure { panels = [Some (G.Panel2D plot)] })
        G.SvgOptions {}
