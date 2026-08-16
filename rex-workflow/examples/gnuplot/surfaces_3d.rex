// Compare every supported representation of the same inline rectangular surface.
import tools.gnuplot as G;

fn panel (title: String) -> (mode: G.SurfaceMode) -> (show_colorbox: Bool) -> G.Panel =
    G.Panel3D (G.Plot3D {
        title = Some title,
        x_axis = G.Axis { label = Some "x" },
        y_axis = G.Axis { label = Some "y" },
        z_axis = G.Axis { label = Some "z" },
        color_axis = G.Axis {
            range = G.NumericRange (G.NumericBounds {
                minimum = -1.0,
                maximum = 1.0
            })
        },
        show_colorbox = show_colorbox,
        view = G.View3D {
            elevation_degrees = 55.0,
            azimuth_degrees = 35.0,
            scale = 0.9
        },
        series = [
            G.SurfaceSeries3D (G.Surface3D {
                grid = G.Grid2D {
                    x = [-2.0, -1.0, 0.0, 1.0, 2.0],
                    y = [-2.0, -1.0, 0.0, 1.0, 2.0],
                    values = [
                        [Some (-0.76), Some (-0.49), Some 0.0, Some 0.49, Some 0.76],
                        [Some (-0.49), Some (-0.45), Some 0.0, Some 0.45, Some 0.49],
                        [Some 0.0, Some 0.0, Some 0.0, Some 0.0, Some 0.0],
                        [Some 0.49, Some 0.45, Some 0.0, Some (-0.45), Some (-0.49)],
                        [Some 0.76, Some 0.49, Some 0.0, Some (-0.49), Some (-0.76)]
                    ]
                },
                title = None,
                mode = mode
            })
        ]
    });

let
    figure = G.Figure {
        title = Some "Surface representations",
        layout = G.GridLayout {
            columns = 2
        },
        panels = [
            Some (panel "Wireframe" G.WireframeSurface false),
            Some (panel "Colored surface" G.ColoredSurface true),
            Some (panel "Contour lines" G.ContourLines false),
            Some (panel "Filled contours" G.FilledContours true)
        ]
    }
in
    G.render_svg figure (G.SvgOptions {
        width_px = 1200,
        height_px = 900
    })
