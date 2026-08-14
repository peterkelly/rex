use blake3::Hash;
use rex::Rex;

/// One PDF file stored as a content-addressed blob.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct Pdf {
    pub content: Hash,
}

/// QPDF JSON output stored as a UTF-8 content-addressed blob.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct JsonFile {
    pub content: Hash,
}

/// One QPDF-produced PDF plus any recoverable warnings reported while writing it.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct PdfOutput {
    pub pdf: Pdf,
    pub warnings: String,
}

/// Numbered PDFs produced by `--split-pages` plus any recoverable QPDF warnings.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct PdfSequenceOutput {
    pub pdfs: Vec<Pdf>,
    pub warnings: String,
}

/// QPDF JSON output plus any recoverable warnings reported while reading the PDF.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct JsonOutput {
    pub json: JsonFile,
    pub warnings: String,
}

/// One PDF and page range supplied to QPDF's `--pages` operation.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct PageSource {
    pub pdf: Pdf,
    pub range: String,
    pub password: Option<String>,
}

/// Page mapping options used by QPDF's `--overlay` and `--underlay` operations.
///
/// The default supplies no password or page-range overrides, so QPDF maps overlay pages in
/// sequence onto corresponding output pages.
#[derive(Clone, Debug, Default, Eq, PartialEq, Rex)]
pub struct OverlaySpec {
    pub password: Option<String>,
    pub to: Option<String>,
    pub from: Option<String>,
    pub repeat: Option<String>,
}

/// Absolute or relative page rotation accepted by QPDF's `--rotate` option.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum Rotation {
    AbsoluteRotation(i64),
    RelativeRotation(i64),
}

/// A QPDF rotation and the optional page range to which it applies.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct RotationSpec {
    pub rotation: Rotation,
    pub pages: Option<String>,
}

/// QPDF's `--object-streams` mode.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum ObjectStreamMode {
    ObjectStreamsPreserve,
    ObjectStreamsDisable,
    ObjectStreamsGenerate,
}

/// QPDF's `--stream-data` mode.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum StreamDataMode {
    StreamDataCompress,
    StreamDataPreserve,
    StreamDataUncompress,
}

/// QPDF's stream decoding level used by transformations and JSON output.
#[derive(Clone, Debug, Default, Eq, PartialEq, Rex)]
pub enum DecodeLevel {
    DecodeNone,
    #[default]
    DecodeGeneralized,
    DecodeSpecialized,
    DecodeAll,
}

/// QPDF's `--remove-unreferenced-resources` policy.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum RemoveUnreferencedResourcesMode {
    RemoveResourcesAuto,
    RemoveResourcesYes,
    RemoveResourcesNo,
}

/// Annotation appearances retained by QPDF's `--flatten-annotations` option.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum FlattenAnnotationsMode {
    FlattenAllAnnotations,
    FlattenPrintAnnotations,
    FlattenScreenAnnotations,
}

/// Printing permission stored in a 128-bit or 256-bit encrypted PDF.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum PrintPermission {
    PrintNone,
    PrintLowResolution,
    PrintFullResolution,
}

/// Modification permission stored in a 128-bit or 256-bit encrypted PDF.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum ModifyPermission {
    ModifyNone,
    ModifyAssembly,
    ModifyForms,
    ModifyAnnotations,
    ModifyAll,
}

/// Modern encryption algorithms exposed by this QPDF wrapper.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum EncryptionMethod {
    EncryptionAes128,
    EncryptionAes256,
}

/// Passwords and permissions supplied to QPDF's `--encrypt` option group.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct EncryptionSpec {
    pub user_password: Option<String>,
    pub owner_password: Option<String>,
    pub method: EncryptionMethod,
    pub print: PrintPermission,
    pub modify: ModifyPermission,
    pub extract: bool,
    pub accessibility: bool,
    pub annotate: bool,
    pub assemble: bool,
    pub form: bool,
    pub cleartext_metadata: bool,
}

/// One output transformation using QPDF option names and semantics.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum WriteOption {
    Linearize,
    CompressStreams(bool),
    RecompressFlate,
    CompressionLevel(u64),
    ObjectStreams(ObjectStreamMode),
    StreamData(StreamDataMode),
    DecodeStreams(DecodeLevel),
    NormalizeContent(bool),
    PreserveUnreferenced,
    RemoveUnreferencedResources(RemoveUnreferencedResourcesMode),
    CoalesceContents,
    ExternalizeInlineImages,
    FlattenRotation,
    Rotate(RotationSpec),
    GenerateAppearances,
    FlattenAnnotations(FlattenAnnotationsMode),
    RemovePageLabels,
    DeterministicId,
    StaticId,
    NoOriginalObjectIds,
    MinimumVersion(String),
    ForceVersion(String),
    Decrypt,
    RemoveRestrictions,
    Encrypt(EncryptionSpec),
}

/// A top-level key accepted by QPDF's `--json-key` option.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum JsonKey {
    JsonAcroform,
    JsonAttachments,
    JsonEncrypt,
    JsonObjectInfo,
    JsonObjects,
    JsonOutlines,
    JsonPageLabels,
    JsonPages,
    JsonQpdf,
}

/// How QPDF includes stream data in JSON output.
#[derive(Clone, Debug, Default, Eq, PartialEq, Rex)]
pub enum JsonStreamData {
    #[default]
    JsonStreamDataNone,
    JsonStreamDataInline,
}

/// Selection and stream options for QPDF JSON version 2 output.
///
/// The default includes all keys and objects, omits stream data, and uses generalized decoding,
/// matching QPDF's documented `--json` defaults.
#[derive(Clone, Debug, Default, Eq, PartialEq, Rex)]
pub struct JsonOptions {
    pub keys: Vec<JsonKey>,
    pub objects: Vec<String>,
    pub stream_data: JsonStreamData,
    pub decode_level: DecodeLevel,
}

/// Outcome category reported by `qpdf --check`.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum CheckStatus {
    CheckClean,
    CheckWarnings,
    CheckErrors,
}

/// QPDF syntax-check status and its complete diagnostic output.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct CheckReport {
    pub status: CheckStatus,
    pub diagnostics: String,
}

/// Installed QPDF version string.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct VersionInfo {
    pub version: String,
}

/// The class of an expected QPDF failure.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum QpdfErrorKind {
    InvalidRequest,
    ProcessFailed,
    UnexpectedOutput,
}

/// An expected invalid request, QPDF process failure, or unrecognized result.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct QpdfError {
    pub kind: QpdfErrorKind,
    pub exit_code: Option<i64>,
    pub message: String,
}
