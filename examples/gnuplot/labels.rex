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
        alignment = G.AlignCenter
    },
    plot = G.Plot2D {
        title = Some "Labels and annotations",
        x_axis = G.Axis {
            range = G.NumericRange (G.NumericBounds {
                minimum = 0.0,
                maximum = 4.0
            })
        },
        y_axis = G.Axis {
            range = G.NumericRange (G.NumericBounds {
                minimum = 0.0,
                maximum = 3.5
            })
        },
        series = [G.LabelSeries labels],
        annotations = [
            G.TextAnnotation (G.TextAnnotation2D {
                position = G.PanelPosition2D 0.03 0.92,
                text = "panel-relative note",
                font = G.Font { size_points = 11.0 },
                alignment = G.AlignLeft
            }),
            G.ArrowAnnotation (G.ArrowAnnotation2D {
                from = G.DataPosition2D 0.4 0.5,
                to = G.DataPosition2D 1.0 1.2,
                line = G.LineStyle {
                    color = Some "#d55e00",
                    dash = G.DashedLine
                },
                head = G.FilledArrowHead
            }),
            G.ReferenceLine (G.ReferenceLine2D {
                orientation = G.HorizontalReference,
                value = 2.0,
                label = Some "target",
                line = G.LineStyle {
                    color = Some "#009e73",
                    dash = G.DottedLine
                }
            })
        ]
    }
in
    G.render_svg
        (G.Figure { panels = [Some (G.Panel2D plot)] })
        G.SvgOptions {}
