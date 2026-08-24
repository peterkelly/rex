// Render data-driven labels together with fixed semantic annotations.
import tools.gnuplot as G;

let
    labels = G.Labels2D {
        data = [
            G.LabelPoint2D { x = 1.0, y = 1.2, text = "alpha" },
            G.LabelPoint2D { x = 2.0, y = 2.4, text = "beta" },
            G.LabelPoint2D { x = 3.0, y = 1.8, text = "gamma" }
        ],
        title = Some "observations",
        color = Some "#0072b2",
        alignment = G.TextAlignment.Center
    },
    plot = G.Plot2D {
        title = Some "Labels and annotations",
        x_axis = G.Axis {
            range = G.AxisRange.Numeric (G.NumericBounds {
                minimum = 0.0,
                maximum = 4.0
            })
        },
        y_axis = G.Axis {
            range = G.AxisRange.Numeric (G.NumericBounds {
                minimum = 0.0,
                maximum = 3.5
            })
        },
        series = [G.Series2D.Label labels],
        annotations = [
            G.Annotation2D.Text (G.TextAnnotation2D {
                position = G.Position2D.Panel 0.03 0.92,
                text = "panel-relative note",
                font = G.Font { size_points = 11.0 },
                alignment = G.TextAlignment.Left
            }),
            G.Annotation2D.Arrow (G.ArrowAnnotation2D {
                from = G.Position2D.Data 0.4 0.5,
                to = G.Position2D.Data 1.0 1.2,
                line = G.LineStyle {
                    color = Some "#d55e00",
                    dash = G.DashPattern.Dashed
                },
                head = G.ArrowHead.Filled
            }),
            G.ReferenceLine (G.ReferenceLine2D {
                orientation = G.ReferenceOrientation.Horizontal,
                value = 2.0,
                label = Some "target",
                line = G.LineStyle {
                    color = Some "#009e73",
                    dash = G.DashPattern.Dotted
                }
            })
        ]
    }
in
    G.render_svg
        (G.Figure { panels = [Some (G.Panel.TwoDimensional plot)] })
        G.SvgOptions {}
