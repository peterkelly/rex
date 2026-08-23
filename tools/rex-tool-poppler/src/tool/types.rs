use blake3::Hash;
use rex::Rex;
pub use rex::modules::std::artifacts::Pdf;
use std::collections::BTreeMap;

/// One arbitrary file produced by a Poppler command-line utility.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct OutputFile {
    pub content: Hash,
}

/// UTF-8 text, XHTML, or TSV produced by `pdftotext` and stored in the CAS.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct TextFile {
    pub content: Hash,
    pub format: TextFormat,
}

/// A CAS tree containing heterogeneous image files produced by `pdfimages`.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct ExtractedImages {
    pub content: Hash,
}

/// Output representation selected for `pdftotext`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Rex)]
pub enum TextFormat {
    #[default]
    PlainText,
    PhysicalLayout,
    ContentStreamOrder,
    HtmlMetadata,
    BoundingBox,
    BoundingBoxLayout,
    TabSeparated,
}

/// Line-ending convention selected with `pdftotext -eol`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Rex)]
pub enum EndOfLine {
    #[default]
    EolUnix,
    EolDos,
    EolMac,
}

/// Pixel crop rectangle accepted by Poppler conversion utilities.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub struct PixelRectangle {
    pub x: u64,
    pub y: u64,
    pub width: u64,
    pub height: u64,
}

/// Options corresponding to common `pdftotext` command-line flags.
///
/// The default selects plain UTF-8 text with Unix line endings and otherwise leaves Poppler's
/// documented behavior unchanged.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct PdfToTextOptions {
    pub first_page: Option<u64>,
    pub last_page: Option<u64>,
    pub format: TextFormat,
    pub resolution: Option<f64>,
    pub crop: Option<PixelRectangle>,
    pub crop_box: bool,
    pub discard_diagonal_text: bool,
    pub column_spacing: Option<f64>,
    pub end_of_line: EndOfLine,
    pub no_page_breaks: bool,
    pub owner_password: Option<String>,
    pub user_password: Option<String>,
}

/// Page box coordinates in PDF points.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct PageBox {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

/// Size, rotation, and standard boxes reported by `pdfinfo` for one page.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct PageInfo {
    pub page: u64,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub rotation: Option<i64>,
    pub media_box: Option<PageBox>,
    pub crop_box: Option<PageBox>,
    pub bleed_box: Option<PageBox>,
    pub trim_box: Option<PageBox>,
    pub art_box: Option<PageBox>,
}

/// Page selection and passwords supplied to `pdfinfo`.
///
/// The default inspects every page without supplying a password.
#[derive(Clone, Debug, Default, Eq, PartialEq, Rex)]
pub struct PdfInfoOptions {
    pub first_page: Option<u64>,
    pub last_page: Option<u64>,
    pub owner_password: Option<String>,
    pub user_password: Option<String>,
}

/// Document information and page geometry parsed from `pdfinfo -box -isodates`.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct PdfInfo {
    pub title: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub author: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    pub creation_date: Option<String>,
    pub modification_date: Option<String>,
    pub custom_metadata: bool,
    pub metadata_stream: bool,
    pub tagged: bool,
    pub user_properties: bool,
    pub suspects: bool,
    pub form: String,
    pub javascript: bool,
    pub pages: u64,
    pub encrypted: bool,
    pub file_size: Option<u64>,
    pub linearized: bool,
    pub pdf_version: String,
    pub page_info: Vec<PageInfo>,
    pub other: BTreeMap<String, String>,
}

/// Output format selected for `pdftocairo`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum CairoFormat {
    CairoPng,
    CairoJpeg,
    CairoTiff,
    CairoPdf,
    CairoPostScript,
    CairoEncapsulatedPostScript,
    CairoSvg,
}

/// Odd/even page filtering accepted by `pdftocairo`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Rex)]
pub enum PageSelection {
    #[default]
    AllPages,
    OddPages,
    EvenPages,
}

/// Raster color mode accepted by `pdftocairo`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Rex)]
pub enum CairoColorMode {
    #[default]
    CairoColor,
    CairoGrayscale,
    CairoMonochrome,
}

/// Antialiasing mode accepted by `pdftocairo -antialias`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Rex)]
pub enum CairoAntialias {
    #[default]
    AntialiasDefault,
    AntialiasNone,
    AntialiasGray,
    AntialiasSubpixel,
    AntialiasFast,
    AntialiasGood,
    AntialiasBest,
}

/// Common rendering and page-selection options for `pdftocairo`.
///
/// The default renders all pages in color with Poppler's default antialiasing and no sizing,
/// cropping, format-specific, or password overrides.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct PdfToCairoOptions {
    pub first_page: Option<u64>,
    pub last_page: Option<u64>,
    pub page_selection: PageSelection,
    pub single_file: bool,
    pub resolution: Option<f64>,
    pub resolution_x: Option<f64>,
    pub resolution_y: Option<f64>,
    pub scale_to: Option<u64>,
    pub scale_to_x: Option<i64>,
    pub scale_to_y: Option<i64>,
    pub crop: Option<PixelRectangle>,
    pub crop_box: bool,
    pub color: CairoColorMode,
    pub transparent: bool,
    pub antialias: CairoAntialias,
    pub jpeg_options: Vec<String>,
    pub tiff_compression: Option<String>,
    pub owner_password: Option<String>,
    pub user_password: Option<String>,
}

/// One vector/single-page file or an ordered raster page sequence from `pdftocairo`.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum CairoOutput {
    CairoSingleFile(OutputFile),
    CairoPageFiles(Vec<OutputFile>),
}

/// Encoding policy selected for files extracted by `pdfimages`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Rex)]
pub enum PdfImagesFormat {
    #[default]
    ImagesDefault,
    ImagesPng,
    ImagesTiff,
    ImagesJpegNative,
    ImagesJpeg2000Native,
    ImagesJbig2Native,
    ImagesCcittNative,
    ImagesAll,
}

/// Page selection, naming, passwords, and output policy for `pdfimages`.
///
/// The default extracts every page using `pdfimages`' default PBM/PPM policy, without page-number
/// suffixes or passwords.
#[derive(Clone, Debug, Default, Eq, PartialEq, Rex)]
pub struct PdfImagesOptions {
    pub first_page: Option<u64>,
    pub last_page: Option<u64>,
    pub format: PdfImagesFormat,
    pub include_page_numbers: bool,
    pub owner_password: Option<String>,
    pub user_password: Option<String>,
}

/// One row parsed from `pdfimages -list`.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct PdfImageInfo {
    pub page: u64,
    pub index: u64,
    pub image_type: String,
    pub width: u64,
    pub height: u64,
    pub color: String,
    pub components: u64,
    pub bits_per_component: u64,
    pub encoding: String,
    pub interpolation: bool,
    pub object: u64,
    pub generation: u64,
    pub x_pixels_per_inch: u64,
    pub y_pixels_per_inch: u64,
    pub size: String,
    pub ratio: String,
}

/// Installed Poppler version as reported by `pdfinfo -v`.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct VersionInfo {
    pub version: String,
}

/// The class of an expected Poppler utility failure.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum PopplerErrorKind {
    InvalidRequest,
    ProcessFailed,
    UnexpectedOutput,
}

/// An expected invalid request, Poppler process failure, or unrecognized result.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct PopplerError {
    pub kind: PopplerErrorKind,
    pub exit_code: Option<i64>,
    pub message: String,
}
