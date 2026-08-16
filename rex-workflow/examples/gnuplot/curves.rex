// Render the supported two-dimensional curve modes from inline numeric points.
import tools.gnuplot as G;

fn curve (title: String) -> (mode: G.CurveMode) -> (offset: f64) -> G.Series2D =
    G.CurveSeries (G.Curve2D {
        data = G.NumericXY [
            (0.0, offset + 0.0),
            (1.0, offset + 1.0),
            (2.0, offset + 0.4),
            (3.0, offset + 1.4),
            (4.0, offset + 0.8)
        ],
        title = Some title,
        mode = mode
    });

let
    plot = G.Plot2D {
        title = Some "Curve modes",
        x_axis = G.Axis { label = Some "x" },
        y_axis = G.Axis { label = Some "y" },
        legend = G.Legend {
            position = G.LegendOutsideRight
        },
        series = [
            curve "lines" G.Lines 0.0,
            curve "points" G.Points 2.0,
            curve "lines + points" G.LinesPoints 4.0,
            curve "steps before" G.StepsBefore 6.0,
            curve "steps centered" G.StepsCentered 8.0,
            curve "steps after" G.StepsAfter 10.0,
            curve "impulses" G.Impulses 12.0
        ]
    },
    figure = G.Figure {
        panels = [Some (G.Panel2D plot)]
    }
in
    G.render_svg figure G.SvgOptions {}
