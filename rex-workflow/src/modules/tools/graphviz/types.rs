use blake3::Hash;
use rex::Rex;
use std::collections::BTreeMap;

/// Text displayed by Graphviz.
///
/// `TextLabel` is ordinary text that the serializer safely quotes. `HtmlLabel` contains the
/// markup between DOT's outer `<` and `>` delimiters and enables Graphviz's HTML-like label
/// language.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum Label {
    TextLabel(String),
    HtmlLabel(String),
}

/// Whether the graph contains directed or undirected edges.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum GraphKind {
    DirectedGraph,
    UndirectedGraph,
}

/// Maximum or desired graph dimensions in inches.
///
/// When `height` is absent, `width` is used for both dimensions. `minimum` changes the dimensions
/// from a maximum to a desired minimum.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct GraphSize {
    pub width: f64,
    pub height: Option<f64>,
    pub minimum: bool,
}

/// A graph aspect-ratio policy.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub enum GraphRatio {
    RatioValue(f64),
    RatioFill,
    RatioCompress,
    RatioExpand,
    RatioAuto,
}

/// Graph canvas margins in inches.
///
/// When `vertical` is absent, `horizontal` is used for both axes.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct GraphMargin {
    pub horizontal: f64,
    pub vertical: Option<f64>,
}

/// The primary direction in which ranked graphs are laid out.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum RankDirection {
    RankTopToBottom,
    RankLeftToRight,
    RankBottomToTop,
    RankRightToLeft,
}

/// The input-order constraint applied to each node's incoming or outgoing edges.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum EdgeOrdering {
    OrderIncoming,
    OrderOutgoing,
}

/// How Graphviz should represent edges.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum SplineMode {
    SplinesNone,
    SplinesLine,
    SplinesPolyline,
    SplinesCurved,
    SplinesOrtho,
    SplinesSpline,
    SplinesCompound,
}

/// Final graph-level rendering and layout attributes.
///
/// These are values for the completed graph, not order-dependent DOT assignments. `extra` is an
/// escape hatch for attributes supported by the packaged Graphviz installation but not modeled by
/// a typed field. A collision between a typed field and `extra` is rejected.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct GraphAttributes {
    pub label: Option<Label>,
    pub font: Option<Font>,
    pub size: Option<GraphSize>,
    pub ratio: Option<GraphRatio>,
    pub margin: Option<GraphMargin>,
    pub rank_direction: Option<RankDirection>,
    pub ordering: Option<EdgeOrdering>,
    pub rotate: Option<i64>,
    pub center: Option<bool>,
    pub color: Option<String>,
    pub background_color: Option<String>,
    pub overlap: Option<String>,
    pub stylesheet: Option<String>,
    pub splines: Option<SplineMode>,
    pub extra: BTreeMap<String, String>,
}

/// A standard compass position used to attach an edge to a node or named port.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum CompassPoint {
    CompassNorth,
    CompassNorthEast,
    CompassEast,
    CompassSouthEast,
    CompassSouth,
    CompassSouthWest,
    CompassWest,
    CompassNorthWest,
    CompassCenter,
    CompassAny,
}

/// A named region and/or compass position on an edge endpoint.
///
/// At least one field must be present. Named regions are defined by record or HTML-like labels.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct Port {
    pub name: Option<String>,
    pub compass: Option<CompassPoint>,
}

/// One node endpoint of an edge.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct Endpoint {
    pub node: String,
    pub port: Option<Port>,
}

/// Font settings shared by graph, node, and edge labels.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct Font {
    pub family: Option<String>,
    pub size_points: Option<f64>,
    pub color: Option<String>,
}

/// A clickable Graphviz element and its browser metadata.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct Link {
    pub url: String,
    pub target: Option<String>,
    pub tooltip: Option<String>,
}

/// How explicit node dimensions interact with the node label.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum NodeSizing {
    SizeAtLeast,
    SizeFixed,
    SizeFixedShape,
}

/// Optional node dimensions and sizing policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Rex)]
pub struct NodeSize {
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub sizing: Option<NodeSizing>,
}

/// A built-in node shape available in the packaged Graphviz runtime.
///
/// Synonyms are intentionally omitted: for example, use `ShapeBox` rather than `rect`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum NodeShape {
    ShapeBox,
    ShapePolygon,
    ShapeEllipse,
    ShapeCircle,
    ShapePoint,
    ShapeEgg,
    ShapeTriangle,
    ShapePlainText,
    ShapePlain,
    ShapeDiamond,
    ShapeTrapezium,
    ShapeParallelogram,
    ShapeHouse,
    ShapePentagon,
    ShapeHexagon,
    ShapeSeptagon,
    ShapeOctagon,
    ShapeDoubleCircle,
    ShapeDoubleOctagon,
    ShapeTripleOctagon,
    ShapeInvertedTriangle,
    ShapeInvertedTrapezium,
    ShapeInvertedHouse,
    ShapeSquare,
    ShapeStar,
    ShapeUnderline,
    ShapeCylinder,
    ShapeNote,
    ShapeTab,
    ShapeFolder,
    ShapeThreeDimensionalBox,
    ShapeComponent,
    ShapePromoter,
    ShapeCodingSequence,
    ShapeTerminator,
    ShapeUntranslatedRegion,
    ShapePrimerSite,
    ShapeRestrictionSite,
    ShapeFivePrimeOverhang,
    ShapeThreePrimeOverhang,
    ShapeNoOverhang,
    ShapeAssembly,
    ShapeSignature,
    ShapeInsulator,
    ShapeRiboSite,
    ShapeRnaStability,
    ShapeProteaseSite,
    ShapeProteinStability,
    ShapeReversePromoter,
    ShapeRightArrow,
    ShapeLeftArrow,
    ShapeLeftPromoter,
    ShapeRecord,
    ShapeRoundedRecord,
}

/// Optional geometry controls for polygon-based node shapes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Rex)]
pub struct PolygonOptions {
    pub regular: Option<bool>,
    pub peripheries: Option<i64>,
    pub sides: Option<i64>,
    pub orientation_degrees: Option<f64>,
    pub distortion: Option<f64>,
    pub skew: Option<f64>,
}

/// One built-in node appearance style.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum NodeStyle {
    NodeFilled,
    NodeInvisible,
    NodeDiagonals,
    NodeRounded,
    NodeDashed,
    NodeDotted,
    NodeSolid,
    NodeBold,
}

/// Final attributes of a node or graph-level node defaults.
///
/// Every typed attribute is optional. A present `styles` list must be non-empty. `extra` is an
/// escape hatch for supported attributes not modeled by a typed field; collisions are rejected.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct NodeAttributes {
    pub label: Option<Label>,
    pub font: Option<Font>,
    pub size: Option<NodeSize>,
    pub shape: Option<NodeShape>,
    pub polygon: Option<PolygonOptions>,
    pub outline_color: Option<String>,
    pub fill_color: Option<String>,
    pub styles: Option<Vec<NodeStyle>>,
    pub external_label: Option<Label>,
    pub link: Option<Link>,
    pub extra: BTreeMap<String, String>,
}

/// One built-in edge line style.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum EdgeStyle {
    EdgeSolid,
    EdgeDashed,
    EdgeDotted,
    EdgeBold,
    EdgeInvisible,
}

/// Which ends of an edge display arrow glyphs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum EdgeDirection {
    ArrowsForward,
    ArrowsBackward,
    ArrowsBoth,
    ArrowsNone,
}

/// A commonly used built-in Graphviz arrow shape.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum ArrowShape {
    ArrowNormal,
    ArrowInverted,
    ArrowDot,
    ArrowOpenDot,
    ArrowInvertedDot,
    ArrowOpenInvertedDot,
    ArrowTee,
    ArrowEmpty,
    ArrowInvertedEmpty,
    ArrowOpen,
    ArrowHalfOpen,
    ArrowDiamond,
    ArrowOpenDiamond,
    ArrowBox,
    ArrowOpenBox,
    ArrowCrow,
    ArrowNone,
}

/// Attributes attached specifically to the head or tail end of an edge.
#[derive(Clone, Debug, Default, Eq, PartialEq, Rex)]
pub struct EdgeEnd {
    pub arrow: Option<ArrowShape>,
    pub clip_to_node: Option<bool>,
    pub link: Option<Link>,
    pub port_group: Option<String>,
}

/// Labels placed near the head and tail of an edge.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct EdgeEndpointLabels {
    pub head: Option<Label>,
    pub tail: Option<Label>,
    pub font: Option<Font>,
    pub distance: Option<f64>,
    pub angle_degrees: Option<f64>,
}

/// Final attributes of an edge or graph-level edge defaults.
///
/// A single color sets the line color; multiple colors request parallel colored lines. Present
/// `styles` and `colors` lists must be non-empty. `extra` follows the node collision rule.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct EdgeAttributes {
    pub label: Option<Label>,
    pub font: Option<Font>,
    pub weight: Option<f64>,
    pub styles: Option<Vec<EdgeStyle>>,
    pub colors: Option<Vec<String>>,
    pub direction: Option<EdgeDirection>,
    pub arrow_scale: Option<f64>,
    pub head: Option<EdgeEnd>,
    pub tail: Option<EdgeEnd>,
    pub endpoint_labels: Option<EdgeEndpointLabels>,
    pub link: Option<Link>,
    pub decorate_label: Option<bool>,
    pub extra: BTreeMap<String, String>,
}

/// One semantic edge connecting exactly two declared node endpoints.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct Edge {
    pub from: Endpoint,
    pub to: Endpoint,
    pub attributes: EdgeAttributes,
}

/// Whether a node group is a plain layout subgraph or a visible cluster.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum SubgraphKind {
    PlainSubgraph,
    Cluster,
}

/// A rank constraint applied to the nodes in a plain subgraph or cluster.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum RankConstraint {
    RankSame,
    RankMinimum,
    RankSource,
    RankMaximum,
    RankSink,
}

/// Final attributes of a semantic subgraph.
#[derive(Clone, Debug, Default, PartialEq, Rex)]
pub struct SubgraphAttributes {
    pub label: Option<Label>,
    pub rank: Option<RankConstraint>,
    /// Minimum space in points between a cluster border and its contents.
    pub padding: Option<f64>,
    pub color: Option<String>,
    pub background_color: Option<String>,
    pub extra: BTreeMap<String, String>,
}

/// A semantic node group in the graph's subgraph arena.
///
/// Node identifiers remain global across the complete graph. Each node must therefore be defined
/// in exactly one `nodes` dictionary. `subgraphs` contains arena keys for nested groups.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct Subgraph {
    pub kind: SubgraphKind,
    pub id: Option<String>,
    pub attributes: SubgraphAttributes,
    pub nodes: BTreeMap<String, NodeAttributes>,
    pub edges: Vec<Edge>,
    pub subgraphs: Vec<String>,
}

impl Default for Subgraph {
    fn default() -> Self {
        Self {
            kind: SubgraphKind::PlainSubgraph,
            id: None,
            attributes: SubgraphAttributes::default(),
            nodes: BTreeMap::new(),
            edges: Vec::new(),
            subgraphs: Vec::new(),
        }
    }
}

/// A complete semantic graph.
///
/// Nodes have one final attribute value and edges connect declared nodes explicitly. Graph-level
/// node and edge defaults remain available for shared styling. The DOT serializer chooses a
/// deterministic private statement order; Rex code does not express order-dependent DOT state.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct Graph {
    pub strict: bool,
    pub kind: GraphKind,
    pub id: Option<String>,
    pub attributes: GraphAttributes,
    pub node_defaults: NodeAttributes,
    pub edge_defaults: EdgeAttributes,
    pub nodes: BTreeMap<String, NodeAttributes>,
    pub edges: Vec<Edge>,
    pub subgraphs: BTreeMap<String, Subgraph>,
}

impl Default for Graph {
    fn default() -> Self {
        Self {
            strict: false,
            kind: GraphKind::DirectedGraph,
            id: None,
            attributes: GraphAttributes::default(),
            node_defaults: NodeAttributes::default(),
            edge_defaults: EdgeAttributes::default(),
            nodes: BTreeMap::new(),
            edges: Vec::new(),
            subgraphs: BTreeMap::new(),
        }
    }
}

/// A Graphviz layout engine selected for rendering.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum LayoutEngine {
    LayoutDot,
    LayoutNeato,
    LayoutFdp,
    LayoutSfdp,
    LayoutCirco,
    LayoutTwopi,
    LayoutOsage,
    LayoutPatchwork,
    LayoutNop,
    LayoutNop2,
}

/// A headless output format selected for rendering.
///
/// Actual availability depends on the packaged Graphviz installation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum RenderFormat {
    FormatAscii,
    FormatBmp,
    FormatCanon,
    FormatDot,
    FormatDotJson,
    FormatEps,
    FormatFig,
    FormatGd,
    FormatGd2,
    FormatGif,
    FormatJpeg,
    FormatJson,
    FormatJson0,
    FormatPdf,
    FormatPic,
    FormatPlain,
    FormatPlainExt,
    FormatPng,
    FormatPostScript,
    FormatPostScript2,
    FormatPov,
    FormatSvg,
    FormatSvgz,
    FormatTga,
    FormatTiff,
    FormatVml,
    FormatVmlz,
    FormatVrml,
    FormatWebp,
    FormatXDot,
    FormatXDot12,
    FormatXDot14,
    FormatXDotJson,
}

/// One rendered Graphviz artifact stored in the workflow CAS.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct RenderedGraph {
    pub content: Hash,
    pub format: RenderFormat,
}

/// The installed Graphviz version reported by `dot -V`.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct VersionInfo {
    pub version: String,
}

/// The class of an expected Graphviz request or process failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub enum GraphvizErrorKind {
    InvalidGraph,
    ProcessFailed,
    UnexpectedOutput,
}

/// An expected invalid graph, Graphviz process failure, or unrecognized result.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct GraphvizError {
    pub kind: GraphvizErrorKind,
    pub exit_code: Option<i64>,
    pub message: String,
}
