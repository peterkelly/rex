use chrono::{DateTime, Utc};
use rex::Rex;
pub use rex::modules::std::artifacts::{Image, Pdf};

/// Inclusive numeric bounds used by axes, errors, histograms, and color scales.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct NumericBounds {
    pub minimum: f64,
    pub maximum: f64,
}

/// Inclusive UTC time bounds used by timestamped axes.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct TimeBounds {
    pub minimum: DateTime<Utc>,
    pub maximum: DateTime<Utc>,
}

/// An automatically chosen, numeric, or timestamped axis range.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub enum AxisRange {
    #[default]
    AutoRange,
    NumericRange(NumericBounds),
    TimeRange(TimeBounds),
}

/// A linear scale or logarithmic scale with an explicit base.
#[derive(Clone, Copy, Debug, Default, PartialEq, Rex)]
pub enum AxisScale {
    #[default]
    LinearScale,
    LogScale(f64),
}

/// Automatic tick labels or a safely quoted numeric or UTC time format.
#[derive(Clone, Debug, Default, Eq, PartialEq, Rex)]
pub enum TickFormat {
    #[default]
    AutomaticTicks,
    NumericTicks(String),
    TimeTicks(String),
}

/// Final settings for one numeric or timestamped plot axis.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct Axis {
    pub label: Option<String>,
    pub range: AxisRange,
    pub scale: AxisScale,
    pub reversed: bool,
    pub tick_format: TickFormat,
}

/// A font used for plot text.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct Font {
    pub family: String,
    pub size_points: f64,
}

impl Default for Font {
    fn default() -> Self {
        Self {
            family: "DejaVu Sans".to_owned(),
            size_points: 10.0,
        }
    }
}

/// One position and color in a continuous palette.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct PaletteStop {
    pub position: f64,
    pub color: String,
}

/// A continuous color palette shared by color-mapped series in one panel.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct Palette {
    pub stops: Vec<PaletteStop>,
    pub reversed: bool,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            stops: vec![
                PaletteStop {
                    position: 0.0,
                    color: "#440154".to_owned(),
                },
                PaletteStop {
                    position: 0.25,
                    color: "#3b528b".to_owned(),
                },
                PaletteStop {
                    position: 0.5,
                    color: "#21918c".to_owned(),
                },
                PaletteStop {
                    position: 0.75,
                    color: "#5ec962".to_owned(),
                },
                PaletteStop {
                    position: 1.0,
                    color: "#fde725".to_owned(),
                },
            ],
            reversed: false,
        }
    }
}

/// Stable figure-wide colors, font, palette, and series color cycle.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct Theme {
    pub background_color: String,
    pub foreground_color: String,
    pub grid_color: String,
    pub font: Font,
    pub palette: Palette,
    pub color_cycle: Vec<String>,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            background_color: "white".to_owned(),
            foreground_color: "#202020".to_owned(),
            grid_color: "#d8d8d8".to_owned(),
            font: Font::default(),
            palette: Palette::default(),
            color_cycle: vec![
                "#0072b2".to_owned(),
                "#d55e00".to_owned(),
                "#009e73".to_owned(),
                "#cc79a7".to_owned(),
                "#e69f00".to_owned(),
                "#56b4e9".to_owned(),
                "#f0e442".to_owned(),
                "#000000".to_owned(),
            ],
        }
    }
}

/// The order used to fill a grid of figure panels.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Rex)]
pub enum GridFillOrder {
    #[default]
    FillRowsFirst,
    FillColumnsFirst,
}

/// A regular panel grid; omitted rows are inferred from the number of cells.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct GridLayout {
    pub columns: u64,
    pub rows: Option<u64>,
    pub fill_order: GridFillOrder,
    pub horizontal_spacing: f64,
    pub vertical_spacing: f64,
}

impl Default for GridLayout {
    fn default() -> Self {
        Self {
            columns: 1,
            rows: None,
            fill_order: GridFillOrder::FillRowsFirst,
            horizontal_spacing: 0.05,
            vertical_spacing: 0.08,
        }
    }
}

/// One immutable figure containing an ordered grid of optional panels.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct Figure {
    pub title: Option<String>,
    pub theme: Theme,
    pub layout: GridLayout,
    pub panels: Vec<Option<Panel>>,
}

/// A two-dimensional or three-dimensional figure panel.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum Panel {
    Panel2D(Plot2D),
    Panel3D(Plot3D),
}

/// Which axes a two-dimensional series uses.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Rex)]
pub enum AxisBinding {
    #[default]
    PrimaryAxes,
    SecondaryX,
    SecondaryY,
    SecondaryAxes,
}

/// Major and minor grid-line visibility for one panel.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct Grid {
    pub x: bool,
    pub y: bool,
    pub minor: bool,
}

impl Default for Grid {
    fn default() -> Self {
        Self {
            x: true,
            y: true,
            minor: false,
        }
    }
}

/// A standard legend location within or outside a panel.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Rex)]
pub enum LegendPosition {
    LegendTopLeft,
    #[default]
    LegendTopRight,
    LegendBottomLeft,
    LegendBottomRight,
    LegendOutsideRight,
    LegendBelow,
}

/// Final legend visibility, placement, orientation, and item ordering.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct Legend {
    pub visible: bool,
    pub position: LegendPosition,
    pub horizontal: bool,
    pub reversed: bool,
}

impl Default for Legend {
    fn default() -> Self {
        Self {
            visible: true,
            position: LegendPosition::LegendTopRight,
            horizontal: false,
            reversed: false,
        }
    }
}

/// A portable line dash pattern.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Rex)]
pub enum DashPattern {
    #[default]
    SolidLine,
    DashedLine,
    DottedLine,
    DashDotLine,
}

/// Final appearance of one line.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct LineStyle {
    pub color: Option<String>,
    pub width: f64,
    pub dash: DashPattern,
}

impl Default for LineStyle {
    fn default() -> Self {
        Self {
            color: None,
            width: 1.5,
            dash: DashPattern::SolidLine,
        }
    }
}

/// A portable point glyph.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Rex)]
pub enum PointShape {
    PointPlus,
    PointCross,
    PointStar,
    PointSquare,
    PointFilledSquare,
    #[default]
    PointCircle,
    PointFilledCircle,
    PointTriangle,
    PointFilledTriangle,
    PointDiamond,
    PointFilledDiamond,
}

/// Final appearance of point glyphs.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct PointStyle {
    pub color: Option<String>,
    pub size: f64,
    pub shape: PointShape,
}

impl Default for PointStyle {
    fn default() -> Self {
        Self {
            color: None,
            size: 1.0,
            shape: PointShape::PointCircle,
        }
    }
}

/// A solid or patterned filled region.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Rex)]
pub enum FillMode {
    #[default]
    SolidFill,
    PatternFill(i64),
}

/// Final appearance of a filled region.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct FillStyle {
    pub color: Option<String>,
    /// Alpha used by solid fills; pattern fills use terminal-defined transparency.
    pub opacity: f64,
    pub mode: FillMode,
    pub border: bool,
}

impl Default for FillStyle {
    fn default() -> Self {
        Self {
            color: None,
            opacity: 0.35,
            mode: FillMode::SolidFill,
            border: false,
        }
    }
}

/// Numeric or UTC-timestamped XY points, optionally split into explicit path segments.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum XYData {
    NumericXY(Vec<(f64, f64)>),
    NumericSegments(Vec<Vec<(f64, f64)>>),
    TimeXY(Vec<(DateTime<Utc>, f64)>),
    TimeSegments(Vec<Vec<(DateTime<Utc>, f64)>>),
}

impl Default for XYData {
    fn default() -> Self {
        Self::NumericXY(Vec::new())
    }
}

/// How an ordered XY dataset is drawn.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Rex)]
pub enum CurveMode {
    #[default]
    Lines,
    Points,
    LinesPoints,
    StepsBefore,
    StepsCentered,
    StepsAfter,
    Impulses,
}

/// One ordered two-dimensional curve or point series.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct Curve2D {
    pub data: XYData,
    pub title: Option<String>,
    pub mode: CurveMode,
    pub axes: AxisBinding,
    pub line: LineStyle,
    pub points: PointStyle,
}

/// A symmetric error magnitude or explicit lower and upper bounds.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub enum ErrorExtent {
    SymmetricError(f64),
    AbsoluteError(NumericBounds),
}

/// One numeric point with optional horizontal and vertical errors.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct ErrorPoint2D {
    pub x: f64,
    pub y: f64,
    pub x_error: Option<ErrorExtent>,
    pub y_error: Option<ErrorExtent>,
}

/// Numeric error bars, optionally connected through their central values.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct ErrorBars2D {
    pub data: Vec<ErrorPoint2D>,
    pub title: Option<String>,
    pub axes: AxisBinding,
    pub connected: bool,
    pub line: LineStyle,
    pub points: PointStyle,
}

/// One x coordinate and the lower and upper boundaries of a filled band.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct BandPoint2D {
    pub x: f64,
    pub lower: f64,
    pub upper: f64,
}

/// One band or a set of explicitly disconnected band segments.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum BandData {
    BandPoints(Vec<BandPoint2D>),
    BandSegments(Vec<Vec<BandPoint2D>>),
}

impl Default for BandData {
    fn default() -> Self {
        Self::BandPoints(Vec::new())
    }
}

/// A filled region between lower and upper y values.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct Band2D {
    pub data: BandData,
    pub title: Option<String>,
    pub axes: AxisBinding,
    pub fill: FillStyle,
}

/// One named sequence of categorical bar values.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct BarSeries {
    pub title: Option<String>,
    pub values: Vec<(String, f64)>,
    pub fill: FillStyle,
}

/// Whether multiple categorical bar series are clustered or stacked.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Rex)]
pub enum BarArrangement {
    #[default]
    ClusteredBars,
    StackedBars,
}

/// A categorical chart containing one or more aligned bar series.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct BarChart {
    pub series: Vec<BarSeries>,
    pub arrangement: BarArrangement,
    pub gap: f64,
}

impl Default for BarChart {
    fn default() -> Self {
        Self {
            series: Vec::new(),
            arrangement: BarArrangement::default(),
            gap: 1.0,
        }
    }
}

/// A fixed number of equal-width bins or an explicit bin width.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub enum HistogramBins {
    BinCount(u64),
    BinWidth(f64),
}

impl Default for HistogramBins {
    fn default() -> Self {
        Self::BinCount(20)
    }
}

/// How histogram bin values are normalized.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Rex)]
pub enum HistogramNormalization {
    #[default]
    HistogramCounts,
    HistogramProbability,
    HistogramDensity,
}

/// A statistical histogram computed from inline numeric samples.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct Histogram {
    pub samples: Vec<f64>,
    pub title: Option<String>,
    pub bins: HistogramBins,
    pub range: Option<NumericBounds>,
    pub normalization: HistogramNormalization,
    pub axes: AxisBinding,
    pub fill: FillStyle,
}

/// A rectangular numeric grid whose rows correspond to y and columns correspond to x.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct Grid2D {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub values: Vec<Vec<Option<f64>>>,
}

/// A color-mapped rectangular grid drawn in a two-dimensional panel.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct Heatmap2D {
    pub grid: Grid2D,
    pub title: Option<String>,
    pub axes: AxisBinding,
}

/// One vector anchored at x and y with the given displacement.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct Vector2D {
    pub x: f64,
    pub y: f64,
    pub dx: f64,
    pub dy: f64,
}

/// Whether vector arrows have no head, an open head, or a filled head.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Rex)]
pub enum ArrowHead {
    NoArrowHead,
    OpenArrowHead,
    #[default]
    FilledArrowHead,
}

/// A set of two-dimensional vectors with a shared appearance.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct Vectors2D {
    pub data: Vec<Vector2D>,
    pub title: Option<String>,
    pub axes: AxisBinding,
    pub line: LineStyle,
    pub head: ArrowHead,
}

/// Horizontal text alignment relative to an anchor point.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Rex)]
pub enum TextAlignment {
    AlignLeft,
    #[default]
    AlignCenter,
    AlignRight,
}

/// One text label anchored at numeric x and y coordinates.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct LabelPoint2D {
    pub x: f64,
    pub y: f64,
    pub text: String,
}

/// A set of data-driven labels in a two-dimensional panel.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct Labels2D {
    pub data: Vec<LabelPoint2D>,
    pub title: Option<String>,
    pub axes: AxisBinding,
    pub font: Font,
    pub color: Option<String>,
    pub alignment: TextAlignment,
}

/// One semantic two-dimensional plot layer.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum Series2D {
    CurveSeries(Curve2D),
    ErrorSeries(ErrorBars2D),
    BandSeries(Band2D),
    BarSeries2D(BarChart),
    HistogramSeries(Histogram),
    HeatmapSeries(Heatmap2D),
    VectorSeries(Vectors2D),
    LabelSeries(Labels2D),
}

/// A numeric point or a panel-relative point used by annotations.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub enum Position2D {
    DataPosition2D(f64, f64),
    PanelPosition2D(f64, f64),
}

/// One fixed text annotation in a two-dimensional panel.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct TextAnnotation2D {
    pub position: Position2D,
    pub text: String,
    pub font: Font,
    pub alignment: TextAlignment,
}

/// One fixed arrow annotation in a two-dimensional panel.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct ArrowAnnotation2D {
    pub from: Position2D,
    pub to: Position2D,
    pub line: LineStyle,
    pub head: ArrowHead,
}

/// The orientation of a numeric reference line.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum ReferenceOrientation {
    HorizontalReference,
    VerticalReference,
}

/// One horizontal or vertical reference line with an optional label.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct ReferenceLine2D {
    pub orientation: ReferenceOrientation,
    pub value: f64,
    pub label: Option<String>,
    pub line: LineStyle,
}

/// One fixed two-dimensional annotation.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum Annotation2D {
    TextAnnotation(TextAnnotation2D),
    ArrowAnnotation(ArrowAnnotation2D),
    ReferenceLine(ReferenceLine2D),
}

/// A complete two-dimensional Cartesian plot.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct Plot2D {
    pub title: Option<String>,
    pub x_axis: Axis,
    pub y_axis: Axis,
    pub x2_axis: Option<Axis>,
    pub y2_axis: Option<Axis>,
    pub color_axis: Axis,
    pub grid: Grid,
    pub legend: Legend,
    /// Optional panel override for the figure theme's palette.
    pub palette: Option<Palette>,
    pub show_colorbox: bool,
    pub aspect_ratio: Option<f64>,
    pub series: Vec<Series2D>,
    pub annotations: Vec<Annotation2D>,
}

/// One numeric point in three dimensions.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct Point3D {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// A three-dimensional point cloud.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct PointCloud3D {
    pub data: Vec<Point3D>,
    pub title: Option<String>,
    pub points: PointStyle,
}

/// One three-dimensional path or a set of explicitly disconnected path segments.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum PathData3D {
    PathPoints3D(Vec<Point3D>),
    PathSegments3D(Vec<Vec<Point3D>>),
}

impl Default for PathData3D {
    fn default() -> Self {
        Self::PathPoints3D(Vec::new())
    }
}

/// A connected path through three-dimensional points.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct Path3D {
    pub data: PathData3D,
    pub title: Option<String>,
    pub line: LineStyle,
    pub points: Option<PointStyle>,
}

/// How a gridded surface is represented.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Rex)]
pub enum SurfaceMode {
    #[default]
    WireframeSurface,
    ColoredSurface,
    ContourLines,
    FilledContours,
}

/// A gridded three-dimensional surface or contour plot.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct Surface3D {
    pub grid: Grid2D,
    pub title: Option<String>,
    pub mode: SurfaceMode,
    pub line: LineStyle,
}

/// One semantic three-dimensional plot layer.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum Series3D {
    PointSeries3D(PointCloud3D),
    PathSeries3D(Path3D),
    SurfaceSeries3D(Surface3D),
}

/// Camera angles and scale for a three-dimensional panel.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct View3D {
    pub elevation_degrees: f64,
    pub azimuth_degrees: f64,
    pub scale: f64,
}

impl Default for View3D {
    fn default() -> Self {
        Self {
            elevation_degrees: 60.0,
            azimuth_degrees: 30.0,
            scale: 1.0,
        }
    }
}

/// One fixed text annotation at numeric x, y, and z coordinates.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct TextAnnotation3D {
    pub position: Point3D,
    pub text: String,
    pub font: Font,
    pub alignment: TextAlignment,
}

/// One fixed arrow annotation between numeric three-dimensional positions.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct ArrowAnnotation3D {
    pub from: Point3D,
    pub to: Point3D,
    pub line: LineStyle,
    pub head: ArrowHead,
}

/// One fixed three-dimensional annotation.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum Annotation3D {
    TextAnnotation3DValue(TextAnnotation3D),
    ArrowAnnotation3DValue(ArrowAnnotation3D),
}

/// A complete three-dimensional Cartesian plot.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct Plot3D {
    pub title: Option<String>,
    pub x_axis: Axis,
    pub y_axis: Axis,
    pub z_axis: Axis,
    pub color_axis: Axis,
    pub grid: Grid,
    pub legend: Legend,
    /// Optional panel override for the figure theme's palette.
    pub palette: Option<Palette>,
    pub show_colorbox: bool,
    pub view: View3D,
    pub series: Vec<Series3D>,
    pub annotations: Vec<Annotation3D>,
}

/// Raster PNG output dimensions and transparency.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub struct PngOptions {
    pub width_px: u64,
    pub height_px: u64,
    pub transparent: bool,
}

impl Default for PngOptions {
    fn default() -> Self {
        Self {
            width_px: 960,
            height_px: 540,
            transparent: false,
        }
    }
}

/// SVG output dimensions in CSS pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub struct SvgOptions {
    pub width_px: u64,
    pub height_px: u64,
}

impl Default for SvgOptions {
    fn default() -> Self {
        Self {
            width_px: 960,
            height_px: 540,
        }
    }
}

/// PDF output dimensions in inches.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct PdfOptions {
    pub width_inches: f64,
    pub height_inches: f64,
}

impl Default for PdfOptions {
    fn default() -> Self {
        Self {
            width_inches: 10.0,
            height_inches: 5.625,
        }
    }
}

/// The class of an expected gnuplot request or process failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum GnuplotErrorKind {
    InvalidFigure,
    ProcessFailed,
    UnexpectedOutput,
}

/// An expected invalid figure, gnuplot process failure, or unrecognized result.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct GnuplotError {
    pub kind: GnuplotErrorKind,
    pub exit_code: Option<i64>,
    pub message: String,
}

/// The installed gnuplot version reported by `gnuplot --version`.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct VersionInfo {
    pub version: String,
}
