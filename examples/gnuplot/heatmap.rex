// Render a rectangular grid with missing values and a semantic palette.
import tools.gnuplot as G;

let
    heatmap = G.Heatmap2D {
        grid = G.Grid2D {
            x = [0.0, 1.0, 2.0, 3.0],
            y = [0.0, 1.0, 2.0],
            values = [
                [Some 0.0, Some 1.0, Some 2.0, Some 3.0],
                [Some 1.0, Some 2.0, None, Some 4.0],
                [Some 2.0, Some 3.0, Some 4.0, Some 5.0]
            ]
        },
        title = Some "intensity"
    },
    plot = G.Plot2D {
        title = Some "Heatmap",
        palette = Some (G.Palette {
            stops = [
                G.PaletteStop { position = 0.0, color = "#313695" },
                G.PaletteStop { position = 0.5, color = "#ffffbf" },
                G.PaletteStop { position = 1.0, color = "#a50026" }
            ],
            reversed = false
        }),
        color_axis = G.Axis {
            label = Some "intensity",
            range = G.AxisRange.Numeric (G.NumericBounds {
                minimum = 0.0,
                maximum = 5.0
            })
        },
        show_colorbox = true,
        series = [G.Series2D.Heatmap heatmap]
    }
in
    G.render_svg
        (G.Figure { panels = [Some (G.Panel.TwoDimensional plot)] })
        G.SvgOptions {}
