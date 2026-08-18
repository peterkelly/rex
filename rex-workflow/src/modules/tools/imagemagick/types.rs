pub use crate::modules::std::artifacts::Image;
use blake3::Hash;
use rex::Rex;
use std::collections::BTreeMap;

/// The physical result of encoding an image sequence as one adjoined file or separate files.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum ImageOutput {
    SingleImage(Image),
    MultipleImages(Vec<Image>),
}

/// Image dimensions in pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub struct Size {
    pub width: u64,
    pub height: u64,
}

/// A pixel rectangle with dimensions and a signed top-left offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub struct Rectangle {
    pub width: u64,
    pub height: u64,
    pub x: i64,
    pub y: i64,
}

/// A two-dimensional coordinate used by drawing and image operators.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Horizontal and vertical image resolution, interpreted according to the active units setting.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct Resolution {
    pub x: f64,
    pub y: f64,
}

/// ImageMagick blur/sharpen geometry: kernel radius and Gaussian standard deviation.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct BlurGeometry {
    pub radius: f64,
    pub sigma: f64,
}

/// An ImageMagick color expression such as a name, hex value, or supported color syntax.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct Color {
    pub value: String,
}

/// An ImageMagick coder/format name without a leading dot.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct Format {
    pub name: String,
}

/// An ImageMagick define, rendered as `namespace:name=value`.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct Define {
    pub namespace: String,
    pub name: String,
    pub value: String,
}

/// Frames selected from a stored multi-frame image; indices are zero-based and inclusive in ranges.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum FrameSelection {
    AllFrames,
    Frame(u64),
    FrameRange(u64, u64),
    Frames(Vec<u64>),
}

/// ImageMagick gravity used to anchor geometry, text, montage tiles, and composition.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum Gravity {
    GravityNorthWest,
    GravityNorth,
    GravityNorthEast,
    GravityWest,
    GravityCenter,
    GravityEast,
    GravitySouthWest,
    GravitySouth,
    GravitySouthEast,
}

/// A reconstruction filter for resizing or related sampling operations.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum Filter {
    FilterPoint,
    FilterBox,
    FilterTriangle,
    FilterHermite,
    FilterHann,
    FilterHamming,
    FilterBlackman,
    FilterGaussian,
    FilterQuadratic,
    FilterCubic,
    FilterCatrom,
    FilterMitchell,
    FilterLanczos,
    FilterRobidoux,
    OtherFilter(String),
}

/// An ImageMagick colorspace name used for conversion, reading, or writing.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum Colorspace {
    ColorspaceSrgb,
    ColorspaceRgb,
    ColorspaceGray,
    ColorspaceCmyk,
    ColorspaceLab,
    ColorspaceLch,
    ColorspaceHsl,
    ColorspaceHsv,
    ColorspaceXyz,
    ColorspaceYuv,
    OtherColorspace(String),
}

/// The channel-intensity formula used by grayscale and related operations.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum IntensityMethod {
    IntensityAverage,
    IntensityBrightness,
    IntensityLightness,
    IntensityMean,
    IntensityMeanSquare,
    IntensityRec601Luma,
    IntensityRec601Luminance,
    IntensityRec709Luma,
    IntensityRec709Luminance,
    IntensityRootMeanSquare,
    OtherIntensityMethod(String),
}

/// One channel or channel group selected for subsequent channel-aware operations.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum Channel {
    ChannelRed,
    ChannelGreen,
    ChannelBlue,
    ChannelAlpha,
    ChannelBlack,
    ChannelCyan,
    ChannelMagenta,
    ChannelYellow,
    ChannelGray,
    ChannelRgb,
    ChannelRgba,
    ChannelCmyk,
    ChannelCmyka,
    ChannelAll,
    OtherChannel(String),
}

/// An ImageMagick alpha-channel operation mode.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum AlphaMode {
    AlphaActivate,
    AlphaDeactivate,
    AlphaSet,
    AlphaOpaque,
    AlphaTransparent,
    AlphaExtract,
    AlphaCopy,
    AlphaShape,
    AlphaBackground,
    OtherAlphaMode(String),
}

/// A built-in ImageMagick noise distribution.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum NoiseKind {
    NoiseGaussian,
    NoiseImpulse,
    NoiseLaplacian,
    NoiseMultiplicativeGaussian,
    NoisePoisson,
    NoiseRandom,
    NoiseUniform,
    OtherNoise(String),
}

/// An ImageMagick alpha-composition operator used when combining images.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum ComposeOperator {
    ComposeOver,
    ComposeAtop,
    ComposeIn,
    ComposeOut,
    ComposeXor,
    ComposeMultiply,
    ComposeScreen,
    ComposeOverlay,
    ComposeDarken,
    ComposeLighten,
    ComposeDifference,
    ComposeExclusion,
    ComposePlus,
    ComposeMinus,
    ComposeCopy,
    ComposeCopyAlpha,
    ComposeDstOver,
    ComposeSrc,
    ComposeDst,
    OtherCompose(String),
}

/// A coder compression method; support depends on the selected output format and delegates.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum Compression {
    CompressionNone,
    CompressionBZip,
    CompressionFax,
    CompressionGroup4,
    CompressionJpeg,
    CompressionLosslessJpeg,
    CompressionLzw,
    CompressionRle,
    CompressionZip,
    CompressionZstd,
    OtherCompression(String),
}

/// A format-specific scan/interlace scheme for encoded output.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum Interlace {
    InterlaceNone,
    InterlaceLine,
    InterlacePlane,
    InterlacePartition,
    InterlaceGif,
    InterlaceJpeg,
    InterlacePng,
    OtherInterlace(String),
}

/// Typed ImageMagick resize geometry with explicit fit, fill, exact, area, or percentage semantics.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum ResizeGeometry {
    FitWithin(Size),
    FillArea(Size),
    ExactSize(Size),
    ShrinkWithin(Size),
    EnlargeWithin(Size),
    PixelArea(u64),
    ResizePercentage(f64, f64),
}

/// A setting applied before a stored image is decoded.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum ReadOption {
    ReadDensity(Resolution),
    ReadColorspace(Colorspace),
    ReadDepth(u64),
    ReadPage(Rectangle),
    ReadSize(Size),
    ReadAlpha(AlphaMode),
    ReadBackground(Color),
    ReadProfile(Hash),
    ReadDefine(Define),
    ReadFormatHint(Format),
}

/// One bundled, headless ImageMagick pseudo-image.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum BuiltinImageKind {
    BuiltinLogo,
    BuiltinRose,
    BuiltinWizard,
    BuiltinGranite,
    BuiltinNetscape,
}

/// A stored image selection or a synthetic ImageMagick image source.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum ImageSource {
    StoredImage(Image, FrameSelection, Vec<ReadOption>),
    Canvas(Size, Color),
    LinearGradient(Size, Color, Color),
    RadialGradient(Size, Color, Color),
    Checkerboard(Size),
    NoiseImage(Size, NoiseKind),
    BuiltinImage(BuiltinImageKind),
}

/// A persistent ImageMagick setting that affects later reads or operations until changed.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum ImageSetting {
    SettingAntialias(bool),
    SettingAuthenticate(String),
    SettingBias(String),
    SettingBackground(Color),
    SettingBorderColor(Color),
    SettingFill(Color),
    SettingStroke(Color),
    SettingStrokeWidth(f64),
    SettingFont(Hash),
    SettingPointSize(f64),
    SettingGravity(Gravity),
    SettingFilter(Filter),
    SettingDensity(Resolution),
    SettingDepth(u64),
    SettingDirection(String),
    SettingDispose(String),
    SettingDither(String),
    SettingEndian(String),
    SettingIntent(String),
    SettingInterpolate(String),
    SettingLabel(String),
    SettingPage(Rectangle),
    SettingPrecision(u64),
    SettingQuality(u64),
    SettingSamplingFactor(String),
    SettingScene(u64),
    SettingSupport(f64),
    SettingUnits(String),
    SettingVirtualPixel(String),
    SettingSeed(u64),
    SettingFuzz(String),
    SettingDefine(Define),
}

/// A morphology method applied with an ImageMagick kernel specification.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum MorphologyMethod {
    MorphologyConvolve,
    MorphologyCorrelate,
    MorphologyErode,
    MorphologyDilate,
    MorphologyOpen,
    MorphologyClose,
    MorphologyEdgeIn,
    MorphologyEdgeOut,
    MorphologyEdge,
    MorphologyTopHat,
    MorphologyBottomHat,
    MorphologyHitAndMiss,
    MorphologyThinning,
    MorphologyThicken,
    OtherMorphology(String),
}

/// A geometric distortion method and its method-specific control points or coefficients.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum DistortMethod {
    DistortAffine,
    DistortAffineProjection,
    DistortScaleRotateTranslate,
    DistortPerspective,
    DistortPerspectiveProjection,
    DistortBilinearForward,
    DistortBilinearReverse,
    DistortPolynomial,
    DistortArc,
    DistortPolar,
    DistortDePolar,
    DistortBarrel,
    DistortBarrelInverse,
    OtherDistort(String),
}

/// A per-pixel arithmetic, threshold, or bitwise evaluate operator.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum EvaluateOperator {
    EvaluateAdd,
    EvaluateSubtract,
    EvaluateMultiply,
    EvaluateDivide,
    EvaluatePow,
    EvaluateLog,
    EvaluateMin,
    EvaluateMax,
    EvaluateSet,
    EvaluateThreshold,
    EvaluateAnd,
    EvaluateOr,
    EvaluateXor,
    OtherEvaluate(String),
}

/// A persistent drawing style applied to the accompanying drawing primitives.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum DrawStyle {
    DrawFill(Color),
    DrawNoFill,
    DrawStroke(Color),
    DrawNoStroke,
    DrawStrokeWidth(f64),
    DrawFont(Hash),
    DrawPointSize(f64),
    DrawGravity(Gravity),
}

/// An ImageMagick vector-drawing primitive; coordinates are in the current image coordinate space.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum DrawingPrimitive {
    DrawLine(Point, Point),
    DrawRectangle(Rectangle),
    DrawRoundedRectangle(Rectangle, f64, f64),
    DrawCircle(Point, Point),
    DrawEllipse(Point, f64, f64, f64, f64),
    DrawPolygon(Vec<Point>),
    DrawPolyline(Vec<Point>),
    DrawBezier(Vec<Point>),
    DrawText(Point, String),
    DrawPath(String),
}

/// An immediate image operator; operations are compiled and applied in list order.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum ImageOperation {
    AutoGamma,
    AutoLevel,
    AutoOrient,
    AutoThreshold(String),
    Resize(ResizeGeometry),
    AdaptiveResize(Size),
    InterpolativeResize(Size),
    Crop(Rectangle),
    Extent(Rectangle, Gravity, Color),
    Extract(Rectangle),
    Chop(Rectangle),
    Shave(Size),
    Trim,
    Rotate(f64),
    Shear(f64, f64),
    Roll(i64, i64),
    Flip,
    Flop,
    Transpose,
    Transverse,
    Scale(Size),
    Sample(Size),
    LiquidRescale(Size),
    Blur(BlurGeometry),
    BilateralBlur(String),
    AdaptiveBlur(BlurGeometry),
    GaussianBlur(BlurGeometry),
    MotionBlur(f64, f64, f64),
    RotationalBlur(f64),
    SelectiveBlur(f64, f64, String),
    Sharpen(BlurGeometry),
    AdaptiveSharpen(BlurGeometry),
    Median(f64),
    Despeckle,
    Enhance,
    Edge(f64),
    BlueShift(f64),
    Clahe(String),
    Clamp,
    Canny(f64, f64, String, String),
    Emboss(BlurGeometry),
    Charcoal(f64),
    Sketch(f64, f64, f64),
    AddNoise(NoiseKind, f64),
    ReduceNoise(f64),
    Gamma(f64),
    Level(String),
    BrightnessContrast(f64, f64),
    SigmoidalContrast(BoolValue, f64, f64),
    ContrastStretch(String),
    LinearStretch(String),
    Normalize,
    Equalize,
    Contrast(bool),
    Threshold(String),
    AdaptiveThreshold(Size, i64),
    BlackThreshold(String),
    WhiteThreshold(String),
    Colorize(String, Color),
    Modulate(f64, f64, f64),
    SepiaTone(String),
    Solarize(String),
    Negate,
    Grayscale(IntensityMethod),
    ConvertColorspace(Colorspace),
    ColorMatrix(Vec<f64>),
    ColorThreshold(String),
    Alpha(AlphaMode),
    SelectChannels(Vec<Channel>),
    SeparateChannels,
    CombineChannels(Colorspace),
    Transparent(Color, String),
    Opaque(Color, Color),
    FloodFill(Point, Color),
    Clut(Image),
    HaldClut(Image),
    ReadMask(Image),
    WriteMask(Image),
    Convolve(Vec<f64>),
    Morphology(MorphologyMethod, String),
    ForwardFft,
    InverseFft,
    ConnectedComponents(u64),
    HoughLines(String),
    Integral,
    Kmeans(String),
    Kuwahara(f64),
    LocalAdaptiveThreshold(String),
    LocalContrast(String),
    MeanShift(String),
    Distort(DistortMethod, Vec<f64>, bool),
    Implode(f64),
    Swirl(f64),
    Wave(f64, f64),
    Deskew(String),
    CycleColors(i64),
    Polaroid(f64),
    Posterize(u64, bool),
    Quantize(u64, Colorspace),
    Monochrome,
    OrderedDither(String),
    OilPaint(f64),
    Perceptible(f64),
    RandomThreshold(String),
    RangeThreshold(String),
    Raise(String, bool),
    Reshape(String),
    Segment(String),
    Shade(f64, f64),
    Spread(f64),
    Statistic(String, String),
    Unsharp(String),
    Vignette(String),
    WaveletDenoise(f64, f64),
    WhiteBalance,
    UniqueColors,
    Draw(Vec<DrawStyle>, Vec<DrawingPrimitive>),
    Annotate(Point, String),
    StripMetadata,
    SetProperty(String, String),
    DeleteProperty(String),
    ApplyProfile(Hash),
    ColorDecisionList(Hash),
    RemoveProfile(String),
    Evaluate(EvaluateOperator, f64),
    FxExpression(String),
    Shadow(String),
    Border(Size, Color),
    Frame(String, Color),
    OperationDefine(Define),
}

/// An explicit on/off value for ImageMagick operators whose polarity changes their spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum BoolValue {
    Enabled,
    Disabled,
}

/// An operation over the current image sequence rather than a single current image.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum SequenceOperation {
    AppendHorizontal,
    AppendVertical,
    CoalesceFrames,
    DeconstructFrames,
    FlattenLayers,
    MergeLayers,
    MosaicLayers,
    OptimizeFrames,
    OptimizeTransparency,
    ReverseSequence,
    DeleteFrames(FrameSelection),
    DuplicateFrames(FrameSelection, u64),
    SwapFrames(u64, u64),
}

/// One ordered instruction in the general ImageMagick `render` program.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum ImageInstruction {
    ReadImage(ImageSource),
    SetImageSetting(ImageSetting),
    ApplyImageOperation(ImageOperation),
    ApplyOperationGroup(Vec<ImageOperation>),
    ApplySequenceOperation(SequenceOperation),
}

/// Whether an image sequence is encoded into one file or one file per frame.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum OutputMode {
    AdjoinFrames,
    SeparateFrames,
}

/// An output setting applied while encoding an image or image sequence.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum WriteOption {
    WriteQuality(u64),
    WriteDepth(u64),
    WriteCompression(Compression),
    WriteColorspace(Colorspace),
    WriteInterlace(Interlace),
    WriteDensity(Resolution),
    WriteSamplingFactor(String),
    WriteStripMetadata,
    WriteDefine(Define),
}

/// Output coder, frame-adjoining mode, and ordered write settings.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct Encoding {
    pub format: Format,
    pub mode: OutputMode,
    pub options: Vec<WriteOption>,
}

/// An ImageMagick image-comparison metric; custom names must be supported by the host build.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum ComparisonMetric {
    MetricAbsoluteError,
    MetricFuzz,
    MetricMeanAbsoluteError,
    MetricMeanErrorPerPixel,
    MetricMeanSquaredError,
    MetricNormalizedCrossCorrelation,
    MetricPeakAbsoluteError,
    MetricPeakSignalToNoiseRatio,
    MetricRootMeanSquaredError,
    MetricStructuralSimilarity,
    MetricStructuralDissimilarity,
    OtherMetric(String),
}

/// An option controlling comparison fuzz, colors, composition, or selected channels.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum CompareOption {
    CompareFuzz(String),
    CompareHighlightColor(Color),
    CompareLowlightColor(Color),
    CompareCompose(ComposeOperator),
    CompareChannels(Vec<Channel>),
}

/// An option controlling image composition placement and blending behavior.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum CompositeOption {
    CompositeGravity(Gravity),
    CompositeGeometry(Rectangle),
    CompositeBlend(String),
    CompositeDissolve(String),
    CompositeTile,
    CompositeClamp,
    CompositeDefine(Define),
}

/// A contact-sheet tile layout; row and column counts must be positive when supplied.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum MontageLayout {
    MontageAutomatic,
    MontageColumns(u64),
    MontageRows(u64),
    MontageGrid(u64, u64),
}

/// A montage geometry, appearance, label, font, or spacing option.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum MontageOption {
    MontageGeometry(String),
    MontageGravity(Gravity),
    MontageBackground(Color),
    MontageBorder(u64, Color),
    MontageFrame(String),
    MontageShadow,
    MontageLabel(String),
    MontageFont(Hash),
    MontagePointSize(f64),
    MontageSpacing(Size),
}

/// An image-identification mode; feature distances are measured in pixels.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum IdentifyOption {
    IdentifyPing,
    IdentifyVerbose,
    IdentifyFeatures(u64),
    IdentifyMoments,
}

/// Typed metadata for one frame returned by ImageMagick `identify`.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct ImageInfo {
    pub format: String,
    pub mime_type: String,
    pub width: u64,
    pub height: u64,
    pub frame_index: u64,
    pub depth: u64,
    pub colorspace: String,
    pub has_alpha: bool,
    pub orientation: String,
    pub properties: BTreeMap<String, String>,
}

/// The semantic result of comparing two images with an ImageMagick metric.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct Comparison {
    pub equal: bool,
    pub metric: ComparisonMetric,
    pub distortion: f64,
    pub difference: Option<Image>,
}

/// The whole image or one pixel rectangle exported by `extract_pixels`.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum PixelRegion {
    WholeImage,
    PixelRectangle(Rectangle),
}

/// The numeric storage representation of samples in a raw pixel buffer.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum PixelStorageType {
    PixelsChar,
    PixelsShort,
    PixelsInteger,
    PixelsLong,
    PixelsFloat,
    PixelsDouble,
    PixelsQuantum,
}

/// Region, channel order, and sample representation requested from `extract_pixels`.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct PixelSpec {
    pub region: PixelRegion,
    pub channels: Vec<Channel>,
    pub storage_type: PixelStorageType,
}

/// Headerless pixel bytes stored in the CAS with the channel and sample layout needed to decode them.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct PixelBuffer {
    pub content: Hash,
    pub channels: Vec<Channel>,
    pub storage_type: PixelStorageType,
}

/// The class of an expected semantic ImageMagick failure.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum ImageMagickErrorKind {
    InvalidRequest,
    UnsupportedOperation,
    ProcessFailed,
    UnexpectedOutput,
}

/// An expected invalid request, unsupported operation, process failure, or unrecognized result.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct ImageMagickError {
    pub kind: ImageMagickErrorKind,
    pub exit_code: Option<i32>,
    pub message: String,
}

/// Installed ImageMagick version plus its enabled features and built-in delegates.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct VersionInfo {
    pub version: String,
    pub features: String,
    pub delegates: String,
}

/// A category accepted by ImageMagick's `-list` capability query.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum CapabilityDomain {
    CapabilityAlign,
    CapabilityAlpha,
    CapabilityChannel,
    CapabilityColorspace,
    CapabilityCommand,
    CapabilityCompose,
    CapabilityCompress,
    CapabilityDistort,
    CapabilityDither,
    CapabilityEvaluate,
    CapabilityFilter,
    CapabilityFont,
    CapabilityFormat,
    CapabilityGravity,
    CapabilityInterlace,
    CapabilityInterpolate,
    CapabilityKernel,
    CapabilityMetric,
    CapabilityMorphology,
    CapabilityNoise,
    CapabilityOrientation,
    CapabilityPolicy,
    CapabilityStorage,
    CapabilityTool,
    CapabilityType,
    CapabilityUnits,
    OtherCapability(String),
}
