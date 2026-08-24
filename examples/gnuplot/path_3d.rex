// Render explicitly disconnected inline three-dimensional path segments.
import tools.gnuplot as G;

let
    path = G.Path3D {
        data = G.PathData3D.Segments [
            [
                G.Point3D { x = 0.0, y = 0.0, z = 0.0 },
                G.Point3D { x = 1.0, y = 0.4, z = 0.8 },
                G.Point3D { x = 2.0, y = 1.0, z = 1.2 }
            ],
            [
                G.Point3D { x = 0.0, y = 2.0, z = 0.5 },
                G.Point3D { x = 1.0, y = 1.7, z = 1.4 },
                G.Point3D { x = 2.0, y = 1.3, z = 2.2 }
            ]
        ],
        title = Some "trajectories",
        line = G.LineStyle {
            color = Some "#d55e00",
            width = 2.0
        },
        points = Some (G.PointStyle {
            shape = G.PointShape.FilledDiamond,
            size = 1.0
        })
    },
    plot = G.Plot3D {
        title = Some "Segmented paths",
        x_axis = G.Axis { label = Some "x" },
        y_axis = G.Axis { label = Some "y" },
        z_axis = G.Axis { label = Some "z" },
        series = [G.Series3D.Path path]
    }
in
    G.render_svg
        (G.Figure { panels = [Some (G.Panel.ThreeDimensional plot)] })
        G.SvgOptions {}
