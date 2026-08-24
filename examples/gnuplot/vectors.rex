// Render an inline two-dimensional vector field.
import tools.gnuplot as G;

let
    vectors = G.Vectors2D {
        data = [
            G.Vector2D { x = 0.0, y = 0.0, dx = 0.8, dy = 0.2 },
            G.Vector2D { x = 1.0, y = 0.0, dx = 0.5, dy = 0.5 },
            G.Vector2D { x = 2.0, y = 0.0, dx = 0.2, dy = 0.8 },
            G.Vector2D { x = 0.0, y = 1.0, dx = 0.5, dy = -0.5 },
            G.Vector2D { x = 1.0, y = 1.0, dx = 0.0, dy = 0.8 },
            G.Vector2D { x = 2.0, y = 1.0, dx = -0.5, dy = 0.5 }
        ],
        title = Some "velocity",
        line = G.LineStyle {
            color = Some "#0072b2",
            width = 1.8
        },
        head = G.ArrowHead.Filled
    },
    plot = G.Plot2D {
        title = Some "Vector field",
        x_axis = G.Axis {
            label = Some "x",
            range = G.AxisRange.Numeric (G.NumericBounds {
                minimum = -0.25,
                maximum = 2.75
            })
        },
        y_axis = G.Axis {
            label = Some "y",
            range = G.AxisRange.Numeric (G.NumericBounds {
                minimum = -0.25,
                maximum = 2.0
            })
        },
        aspect_ratio = Some 1.0,
        series = [G.Series2D.Vector vectors]
    }
in
    G.render_svg
        (G.Figure { panels = [Some (G.Panel.TwoDimensional plot)] })
        G.SvgOptions {}
