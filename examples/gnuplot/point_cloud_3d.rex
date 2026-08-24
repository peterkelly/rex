// Render an inline three-dimensional point cloud.
import tools.gnuplot as G;

let
    cloud = G.PointCloud3D {
        data = [
            G.Point3D { x = 0.0, y = 0.0, z = 0.2 },
            G.Point3D { x = 1.0, y = 0.0, z = 0.8 },
            G.Point3D { x = 2.0, y = 0.0, z = 0.4 },
            G.Point3D { x = 0.0, y = 1.0, z = 1.1 },
            G.Point3D { x = 1.0, y = 1.0, z = 1.7 },
            G.Point3D { x = 2.0, y = 1.0, z = 1.3 },
            G.Point3D { x = 0.0, y = 2.0, z = 1.8 },
            G.Point3D { x = 1.0, y = 2.0, z = 2.5 },
            G.Point3D { x = 2.0, y = 2.0, z = 2.0 }
        ],
        title = Some "samples",
        points = G.PointStyle {
            color = Some "#cc79a7",
            shape = G.PointShape.FilledCircle,
            size = 1.5
        }
    },
    plot = G.Plot3D {
        title = Some "Point cloud",
        x_axis = G.Axis { label = Some "x" },
        y_axis = G.Axis { label = Some "y" },
        z_axis = G.Axis { label = Some "z" },
        view = G.View3D {
            elevation_degrees = 55.0,
            azimuth_degrees = 35.0,
            scale = 1.0
        },
        series = [G.Series3D.Point cloud]
    }
in
    G.render_svg
        (G.Figure { panels = [Some (G.Panel.ThreeDimensional plot)] })
        G.SvgOptions {}
