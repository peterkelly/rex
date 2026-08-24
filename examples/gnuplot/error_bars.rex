// Render connected XY error bars from inline points.
import tools.gnuplot as G;

let
    errors = G.ErrorBars2D {
        data = [
            G.ErrorPoint2D {
                x = 1.0,
                y = 2.0,
                x_error = Some (G.ErrorExtent.Symmetric 0.10),
                y_error = Some (G.ErrorExtent.Symmetric 0.25)
            },
            G.ErrorPoint2D {
                x = 2.0,
                y = 2.7,
                x_error = Some (G.ErrorExtent.Absolute (G.NumericBounds {
                    minimum = 1.85,
                    maximum = 2.20
                })),
                y_error = Some (G.ErrorExtent.Absolute (G.NumericBounds {
                    minimum = 2.35,
                    maximum = 3.10
                }))
            },
            G.ErrorPoint2D {
                x = 3.0,
                y = 3.4,
                x_error = Some (G.ErrorExtent.Symmetric 0.15),
                y_error = Some (G.ErrorExtent.Symmetric 0.30)
            }
        ],
        title = Some "measurement",
        connected = true,
        points = G.PointStyle {
            shape = G.PointShape.FilledCircle,
            size = 1.2
        }
    },
    plot = G.Plot2D {
        title = Some "Measurement uncertainty",
        x_axis = G.Axis { label = Some "input" },
        y_axis = G.Axis { label = Some "response" },
        series = [G.Series2D.Error errors]
    }
in
    G.render_svg
        (G.Figure { panels = [Some (G.Panel.TwoDimensional plot)] })
        G.SvgOptions {}
