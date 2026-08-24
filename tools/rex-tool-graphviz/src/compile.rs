use super::types::*;
use crate::modules::tools::executor::{
    CasInput, ExpectedOutput, InputKind, OutputKind, ToolArgument, ToolExecutionPlan, ToolProgram,
};
use blake3::Hash;
use std::collections::{BTreeMap, BTreeSet};

const MAX_SUBGRAPH_DEPTH: usize = 256;

pub(crate) fn render_plan(
    source: Hash,
    layout: LayoutEngine,
    format: RenderFormat,
) -> ToolExecutionPlan {
    ToolExecutionPlan {
        program: ToolProgram::new("dot"),
        arguments: vec![
            ToolArgument::literal(format!("-K{}", layout_name(layout))),
            ToolArgument::literal(format!("-T{}", format_name(format))),
            ToolArgument::literal("-o"),
            ToolArgument::output(0),
            ToolArgument::input(0),
        ],
        inputs: vec![CasInput {
            hash: source,
            extension: "dot".to_owned(),
            kind: InputKind::Blob,
        }],
        outputs: vec![ExpectedOutput {
            kind: OutputKind::Single,
            extension: format_extension(format).to_owned(),
        }],
    }
}

pub(crate) fn version_plan() -> ToolExecutionPlan {
    ToolExecutionPlan {
        program: ToolProgram::new("dot"),
        arguments: vec![ToolArgument::literal("-V")],
        inputs: Vec::new(),
        outputs: Vec::new(),
    }
}

pub(crate) fn serialize_graph(graph: &Graph) -> Result<String, GraphvizError> {
    Serializer::new(graph).serialize()
}

#[derive(Clone, Debug)]
enum AttributeValue {
    Text(String),
    Html(String),
}

type AttributeMap = BTreeMap<String, AttributeValue>;

struct Serializer<'a> {
    graph: &'a Graph,
    output: String,
    root_subgraphs: Vec<String>,
}

impl<'a> Serializer<'a> {
    fn new(graph: &'a Graph) -> Self {
        Self {
            graph,
            output: String::new(),
            root_subgraphs: Vec::new(),
        }
    }

    fn serialize(mut self) -> Result<String, GraphvizError> {
        self.root_subgraphs = validate_graph(self.graph)?;

        if self.graph.strict {
            self.output.push_str("strict ");
        }
        self.output.push_str(match self.graph.kind {
            GraphKind::Directed => "digraph",
            GraphKind::Undirected => "graph",
        });
        if let Some(id) = &self.graph.id {
            self.output.push(' ');
            write_quoted(&mut self.output, id)?;
        }
        self.output.push_str(" {\n");

        self.write_default_attributes()?;
        self.write_nodes(&self.graph.nodes, 1)?;
        for key in self.root_subgraphs.clone() {
            self.write_subgraph(&key, 1)?;
        }
        self.write_edges(&self.graph.edges, 1)?;

        self.output.push_str("}\n");
        Ok(self.output)
    }

    fn write_default_attributes(&mut self) -> Result<(), GraphvizError> {
        let graph = graph_attributes(&self.graph.attributes)?;
        write_attribute_statement(&mut self.output, 1, "graph", &graph, false)?;

        let nodes = node_attributes(&self.graph.node_defaults)?;
        write_attribute_statement(&mut self.output, 1, "node", &nodes, false)?;

        let edges = edge_attributes(&self.graph.edge_defaults)?;
        write_attribute_statement(&mut self.output, 1, "edge", &edges, false)
    }

    fn write_nodes(
        &mut self,
        nodes: &BTreeMap<String, NodeAttributes>,
        depth: usize,
    ) -> Result<(), GraphvizError> {
        for (id, source) in nodes {
            write_indent(&mut self.output, depth);
            write_quoted(&mut self.output, id)?;
            let attributes = node_attributes(source)?;
            write_attributes(&mut self.output, &attributes, false)?;
            self.output.push_str(";\n");
        }
        Ok(())
    }

    fn write_edges(&mut self, edges: &[Edge], depth: usize) -> Result<(), GraphvizError> {
        let operator = match self.graph.kind {
            GraphKind::Directed => " -> ",
            GraphKind::Undirected => " -- ",
        };
        for edge in edges {
            write_indent(&mut self.output, depth);
            write_endpoint(&mut self.output, &edge.from)?;
            self.output.push_str(operator);
            write_endpoint(&mut self.output, &edge.to)?;
            let attributes = edge_attributes(&edge.attributes)?;
            write_attributes(&mut self.output, &attributes, false)?;
            self.output.push_str(";\n");
        }
        Ok(())
    }

    fn write_subgraph(&mut self, key: &str, depth: usize) -> Result<(), GraphvizError> {
        if depth > MAX_SUBGRAPH_DEPTH {
            return Err(invalid(format!(
                "subgraph nesting exceeds the limit of {MAX_SUBGRAPH_DEPTH}"
            )));
        }
        let subgraph = self
            .graph
            .subgraphs
            .get(key)
            .ok_or_else(|| invalid(format!("subgraph `{key}` is not defined")))?;

        write_indent(&mut self.output, depth);
        self.output.push_str("subgraph");
        match subgraph.kind {
            SubgraphKind::Plain => {
                if let Some(id) = &subgraph.id {
                    self.output.push(' ');
                    write_quoted(&mut self.output, id)?;
                }
            }
            SubgraphKind::Cluster => {
                self.output.push(' ');
                let base = subgraph.id.as_deref().unwrap_or(key);
                let name = if base.starts_with("cluster") {
                    base.to_owned()
                } else {
                    format!("cluster_{base}")
                };
                write_quoted(&mut self.output, &name)?;
            }
        }
        self.output.push_str(" {\n");

        let attributes = subgraph_attributes(&subgraph.attributes)?;
        write_attribute_statement(&mut self.output, depth + 1, "graph", &attributes, false)?;
        self.write_nodes(&subgraph.nodes, depth + 1)?;
        for child in &subgraph.subgraphs {
            self.write_subgraph(child, depth + 1)?;
        }
        self.write_edges(&subgraph.edges, depth + 1)?;

        write_indent(&mut self.output, depth);
        self.output.push_str("}\n");
        Ok(())
    }
}

fn validate_graph(graph: &Graph) -> Result<Vec<String>, GraphvizError> {
    let mut nodes = BTreeSet::new();
    collect_nodes("graph", &graph.nodes, &mut nodes)?;
    for (key, subgraph) in &graph.subgraphs {
        collect_nodes(&format!("subgraph `{key}`"), &subgraph.nodes, &mut nodes)?;
    }

    let mut parents = BTreeMap::<String, String>::new();
    for (parent, subgraph) in &graph.subgraphs {
        let mut local = BTreeSet::new();
        for child in &subgraph.subgraphs {
            if !local.insert(child) {
                return Err(invalid(format!(
                    "subgraph `{parent}` contains child `{child}` more than once"
                )));
            }
            if !graph.subgraphs.contains_key(child) {
                return Err(invalid(format!(
                    "subgraph `{parent}` refers to missing child `{child}`"
                )));
            }
            if let Some(previous) = parents.insert(child.clone(), parent.clone()) {
                return Err(invalid(format!(
                    "subgraph `{child}` has multiple parents: `{previous}` and `{parent}`"
                )));
            }
        }
    }

    let mut active = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for key in graph.subgraphs.keys() {
        validate_subgraph_tree(graph, key, 1, &mut active, &mut visited)?;
    }

    for edge in &graph.edges {
        validate_edge(edge, &nodes)?;
    }
    for subgraph in graph.subgraphs.values() {
        for edge in &subgraph.edges {
            validate_edge(edge, &nodes)?;
        }
    }

    Ok(graph
        .subgraphs
        .keys()
        .filter(|key| !parents.contains_key(*key))
        .cloned()
        .collect())
}

fn collect_nodes(
    scope: &str,
    source: &BTreeMap<String, NodeAttributes>,
    nodes: &mut BTreeSet<String>,
) -> Result<(), GraphvizError> {
    for id in source.keys() {
        if !nodes.insert(id.clone()) {
            return Err(invalid(format!(
                "node `{id}` is defined more than once; duplicate found in {scope}"
            )));
        }
    }
    Ok(())
}

fn validate_subgraph_tree(
    graph: &Graph,
    key: &str,
    depth: usize,
    active: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
) -> Result<(), GraphvizError> {
    if depth > MAX_SUBGRAPH_DEPTH {
        return Err(invalid(format!(
            "subgraph nesting exceeds the limit of {MAX_SUBGRAPH_DEPTH}"
        )));
    }
    if visited.contains(key) {
        return Ok(());
    }
    if !active.insert(key.to_owned()) {
        return Err(invalid(format!("subgraph cycle detected at `{key}`")));
    }
    let subgraph = graph
        .subgraphs
        .get(key)
        .ok_or_else(|| invalid(format!("subgraph `{key}` is not defined")))?;
    for child in &subgraph.subgraphs {
        validate_subgraph_tree(graph, child, depth + 1, active, visited)?;
    }
    active.remove(key);
    visited.insert(key.to_owned());
    Ok(())
}

fn validate_edge(edge: &Edge, nodes: &BTreeSet<String>) -> Result<(), GraphvizError> {
    for endpoint in [&edge.from, &edge.to] {
        if !nodes.contains(&endpoint.node) {
            return Err(invalid(format!(
                "edge refers to undefined node `{}`",
                endpoint.node
            )));
        }
        if let Some(port) = &endpoint.port
            && port.name.is_none()
            && port.compass.is_none()
        {
            return Err(invalid(format!(
                "edge endpoint `{}` has an empty port",
                endpoint.node
            )));
        }
    }
    Ok(())
}

fn graph_attributes(source: &GraphAttributes) -> Result<AttributeMap, GraphvizError> {
    let mut attributes = extras(&source.extra);
    if let Some(label) = &source.label {
        insert_label(&mut attributes, "label", label)?;
    }
    if let Some(font) = &source.font {
        add_font(&mut attributes, font, "fontname", "fontsize", "fontcolor")?;
    }
    if let Some(size) = source.size {
        let width = finite_number(size.width, "graph size width")?;
        let mut value = if let Some(height) = size.height {
            format!("{width},{}", finite_number(height, "graph size height")?)
        } else {
            width
        };
        if size.minimum {
            value.push('!');
        }
        insert_text(&mut attributes, "size", value)?;
    }
    if let Some(ratio) = source.ratio {
        let value = match ratio {
            GraphRatio::Value(value) => finite_number(value, "graph ratio")?,
            GraphRatio::Fill => "fill".to_owned(),
            GraphRatio::Compress => "compress".to_owned(),
            GraphRatio::Expand => "expand".to_owned(),
            GraphRatio::Auto => "auto".to_owned(),
        };
        insert_text(&mut attributes, "ratio", value)?;
    }
    if let Some(margin) = source.margin {
        let horizontal = finite_number(margin.horizontal, "graph horizontal margin")?;
        let value = if let Some(vertical) = margin.vertical {
            format!(
                "{horizontal},{}",
                finite_number(vertical, "graph vertical margin")?
            )
        } else {
            horizontal
        };
        insert_text(&mut attributes, "margin", value)?;
    }
    if let Some(direction) = source.rank_direction {
        insert_text(
            &mut attributes,
            "rankdir",
            match direction {
                RankDirection::TopToBottom => "TB",
                RankDirection::LeftToRight => "LR",
                RankDirection::BottomToTop => "BT",
                RankDirection::RightToLeft => "RL",
            },
        )?;
    }
    if let Some(ordering) = source.ordering {
        insert_text(
            &mut attributes,
            "ordering",
            match ordering {
                EdgeOrdering::Incoming => "in",
                EdgeOrdering::Outgoing => "out",
            },
        )?;
    }
    if let Some(rotate) = source.rotate {
        insert_text(&mut attributes, "rotate", rotate.to_string())?;
    }
    if let Some(center) = source.center {
        insert_text(&mut attributes, "center", center.to_string())?;
    }
    if let Some(color) = &source.color {
        insert_text(&mut attributes, "color", color)?;
    }
    if let Some(color) = &source.background_color {
        insert_text(&mut attributes, "bgcolor", color)?;
    }
    if let Some(overlap) = &source.overlap {
        insert_text(&mut attributes, "overlap", overlap)?;
    }
    if let Some(stylesheet) = &source.stylesheet {
        insert_text(&mut attributes, "stylesheet", stylesheet)?;
    }
    if let Some(splines) = source.splines {
        insert_text(
            &mut attributes,
            "splines",
            match splines {
                SplineMode::None => "none",
                SplineMode::Line => "line",
                SplineMode::Polyline => "polyline",
                SplineMode::Curved => "curved",
                SplineMode::Ortho => "ortho",
                SplineMode::Spline => "spline",
                SplineMode::Compound => "compound",
            },
        )?;
    }
    Ok(attributes)
}

fn subgraph_attributes(source: &SubgraphAttributes) -> Result<AttributeMap, GraphvizError> {
    let mut attributes = extras(&source.extra);
    if let Some(label) = &source.label {
        insert_label(&mut attributes, "label", label)?;
    }
    if let Some(rank) = source.rank {
        insert_text(
            &mut attributes,
            "rank",
            match rank {
                RankConstraint::Same => "same",
                RankConstraint::Minimum => "min",
                RankConstraint::Source => "source",
                RankConstraint::Maximum => "max",
                RankConstraint::Sink => "sink",
            },
        )?;
    }
    if let Some(padding) = source.padding {
        if padding < 0.0 {
            return Err(invalid("subgraph padding cannot be negative"));
        }
        insert_number(&mut attributes, "margin", padding, "subgraph padding")?;
    }
    if let Some(color) = &source.color {
        insert_text(&mut attributes, "color", color)?;
    }
    if let Some(color) = &source.background_color {
        insert_text(&mut attributes, "bgcolor", color)?;
    }
    Ok(attributes)
}

fn node_attributes(source: &NodeAttributes) -> Result<AttributeMap, GraphvizError> {
    let mut attributes = extras(&source.extra);
    if let Some(label) = &source.label {
        insert_label(&mut attributes, "label", label)?;
    }
    if let Some(font) = &source.font {
        add_font(&mut attributes, font, "fontname", "fontsize", "fontcolor")?;
    }
    if let Some(size) = source.size {
        if let Some(width) = size.width {
            insert_number(&mut attributes, "width", width, "node width")?;
        }
        if let Some(height) = size.height {
            insert_number(&mut attributes, "height", height, "node height")?;
        }
        if let Some(sizing) = size.sizing {
            insert_text(
                &mut attributes,
                "fixedsize",
                match sizing {
                    NodeSizing::AtLeast => "false",
                    NodeSizing::Fixed => "true",
                    NodeSizing::FixedShape => "shape",
                },
            )?;
        }
    }
    if let Some(shape) = source.shape {
        insert_text(&mut attributes, "shape", node_shape_value(shape))?;
    }
    if let Some(polygon) = source.polygon {
        if let Some(regular) = polygon.regular {
            insert_text(&mut attributes, "regular", regular.to_string())?;
        }
        if let Some(peripheries) = polygon.peripheries {
            insert_text(&mut attributes, "peripheries", peripheries.to_string())?;
        }
        if let Some(sides) = polygon.sides {
            insert_text(&mut attributes, "sides", sides.to_string())?;
        }
        if let Some(orientation) = polygon.orientation_degrees {
            insert_number(
                &mut attributes,
                "orientation",
                orientation,
                "node polygon orientation",
            )?;
        }
        if let Some(distortion) = polygon.distortion {
            insert_number(
                &mut attributes,
                "distortion",
                distortion,
                "node polygon distortion",
            )?;
        }
        if let Some(skew) = polygon.skew {
            insert_number(&mut attributes, "skew", skew, "node polygon skew")?;
        }
    }
    if let Some(color) = &source.outline_color {
        insert_text(&mut attributes, "color", color)?;
    }
    if let Some(color) = &source.fill_color {
        insert_text(&mut attributes, "fillcolor", color)?;
    }
    if let Some(styles) = &source.styles {
        if styles.is_empty() {
            return Err(invalid("node styles must not be an empty present list"));
        }
        insert_text(
            &mut attributes,
            "style",
            styles
                .iter()
                .map(|style| node_style_value(*style))
                .collect::<Vec<_>>()
                .join(","),
        )?;
    }
    if let Some(label) = &source.external_label {
        insert_label(&mut attributes, "xlabel", label)?;
    }
    if let Some(link) = &source.link {
        add_link(&mut attributes, link, "href", "target", "tooltip")?;
    }
    Ok(attributes)
}

fn edge_attributes(source: &EdgeAttributes) -> Result<AttributeMap, GraphvizError> {
    let mut attributes = extras(&source.extra);
    if let Some(label) = &source.label {
        insert_label(&mut attributes, "label", label)?;
    }
    if let Some(font) = &source.font {
        add_font(&mut attributes, font, "fontname", "fontsize", "fontcolor")?;
    }
    if let Some(weight) = source.weight {
        insert_number(&mut attributes, "weight", weight, "edge weight")?;
    }
    if let Some(styles) = &source.styles {
        if styles.is_empty() {
            return Err(invalid("edge styles must not be an empty present list"));
        }
        insert_text(
            &mut attributes,
            "style",
            styles
                .iter()
                .map(|style| edge_style_value(*style))
                .collect::<Vec<_>>()
                .join(","),
        )?;
    }
    if let Some(colors) = &source.colors {
        if colors.is_empty() {
            return Err(invalid("edge colors must not be an empty present list"));
        }
        insert_text(&mut attributes, "color", colors.join(":"))?;
    }
    if let Some(direction) = source.direction {
        insert_text(
            &mut attributes,
            "dir",
            match direction {
                EdgeDirection::Forward => "forward",
                EdgeDirection::Backward => "back",
                EdgeDirection::Both => "both",
                EdgeDirection::None => "none",
            },
        )?;
    }
    if let Some(scale) = source.arrow_scale {
        insert_number(&mut attributes, "arrowsize", scale, "edge arrow scale")?;
    }
    if let Some(head) = &source.head {
        add_edge_end(&mut attributes, head, true)?;
    }
    if let Some(tail) = &source.tail {
        add_edge_end(&mut attributes, tail, false)?;
    }
    if let Some(labels) = &source.endpoint_labels {
        if let Some(label) = &labels.head {
            insert_label(&mut attributes, "headlabel", label)?;
        }
        if let Some(label) = &labels.tail {
            insert_label(&mut attributes, "taillabel", label)?;
        }
        if let Some(font) = &labels.font {
            add_font(
                &mut attributes,
                font,
                "labelfontname",
                "labelfontsize",
                "labelfontcolor",
            )?;
        }
        if let Some(distance) = labels.distance {
            insert_number(
                &mut attributes,
                "labeldistance",
                distance,
                "edge endpoint label distance",
            )?;
        }
        if let Some(angle) = labels.angle_degrees {
            insert_number(
                &mut attributes,
                "labelangle",
                angle,
                "edge endpoint label angle",
            )?;
        }
    }
    if let Some(link) = &source.link {
        add_link(&mut attributes, link, "href", "target", "tooltip")?;
    }
    if let Some(decorate) = source.decorate_label {
        insert_text(&mut attributes, "decorate", decorate.to_string())?;
    }
    Ok(attributes)
}

fn add_edge_end(
    attributes: &mut AttributeMap,
    end: &EdgeEnd,
    head: bool,
) -> Result<(), GraphvizError> {
    let (arrow_name, clip_name, href_name, target_name, tooltip_name, group_name) = if head {
        (
            "arrowhead",
            "headclip",
            "headhref",
            "headtarget",
            "headtooltip",
            "samehead",
        )
    } else {
        (
            "arrowtail",
            "tailclip",
            "tailhref",
            "tailtarget",
            "tailtooltip",
            "sametail",
        )
    };
    if let Some(arrow) = end.arrow {
        insert_text(attributes, arrow_name, arrow_shape_value(arrow))?;
    }
    if let Some(clip) = end.clip_to_node {
        insert_text(attributes, clip_name, clip.to_string())?;
    }
    if let Some(link) = &end.link {
        add_link(attributes, link, href_name, target_name, tooltip_name)?;
    }
    if let Some(group) = &end.port_group {
        insert_text(attributes, group_name, group)?;
    }
    Ok(())
}

fn add_font(
    attributes: &mut AttributeMap,
    font: &Font,
    family_name: &str,
    size_name: &str,
    color_name: &str,
) -> Result<(), GraphvizError> {
    if let Some(family) = &font.family {
        insert_text(attributes, family_name, family)?;
    }
    if let Some(size) = font.size_points {
        insert_number(attributes, size_name, size, "font size")?;
    }
    if let Some(color) = &font.color {
        insert_text(attributes, color_name, color)?;
    }
    Ok(())
}

fn add_link(
    attributes: &mut AttributeMap,
    link: &Link,
    href_name: &str,
    target_name: &str,
    tooltip_name: &str,
) -> Result<(), GraphvizError> {
    insert_text(attributes, href_name, &link.url)?;
    if let Some(target) = &link.target {
        insert_text(attributes, target_name, target)?;
    }
    if let Some(tooltip) = &link.tooltip {
        insert_text(attributes, tooltip_name, tooltip)?;
    }
    Ok(())
}

fn extras(source: &BTreeMap<String, String>) -> AttributeMap {
    source
        .iter()
        .map(|(name, value)| (name.clone(), AttributeValue::Text(value.clone())))
        .collect()
}

fn insert_number(
    attributes: &mut AttributeMap,
    name: &str,
    value: f64,
    context: &str,
) -> Result<(), GraphvizError> {
    insert_text(attributes, name, finite_number(value, context)?)
}

fn insert_text(
    attributes: &mut AttributeMap,
    name: &str,
    value: impl Into<String>,
) -> Result<(), GraphvizError> {
    insert_value(attributes, name, AttributeValue::Text(value.into()))
}

fn insert_label(
    attributes: &mut AttributeMap,
    name: &str,
    label: &Label,
) -> Result<(), GraphvizError> {
    let value = match label {
        Label::Text(value) => AttributeValue::Text(value.clone()),
        Label::Html(value) => AttributeValue::Html(value.clone()),
    };
    insert_value(attributes, name, value)
}

fn insert_value(
    attributes: &mut AttributeMap,
    name: &str,
    value: AttributeValue,
) -> Result<(), GraphvizError> {
    if attributes.insert(name.to_owned(), value).is_some() {
        return Err(invalid(format!(
            "attribute `{name}` is set by both a typed field and `extra`"
        )));
    }
    Ok(())
}

fn write_attribute_statement(
    output: &mut String,
    depth: usize,
    target: &str,
    attributes: &AttributeMap,
    include_empty: bool,
) -> Result<(), GraphvizError> {
    if attributes.is_empty() && !include_empty {
        return Ok(());
    }
    write_indent(output, depth);
    output.push_str(target);
    write_attributes(output, attributes, true)?;
    output.push_str(";\n");
    Ok(())
}

fn write_attributes(
    output: &mut String,
    attributes: &AttributeMap,
    include_empty: bool,
) -> Result<(), GraphvizError> {
    if attributes.is_empty() && !include_empty {
        return Ok(());
    }
    output.push_str(" [");
    for (index, (name, value)) in attributes.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        write_quoted(output, name)?;
        output.push('=');
        write_attribute_value(output, value)?;
    }
    output.push(']');
    Ok(())
}

fn write_attribute_value(output: &mut String, value: &AttributeValue) -> Result<(), GraphvizError> {
    match value {
        AttributeValue::Text(value) => write_quoted(output, value),
        AttributeValue::Html(value) => {
            validate_html_label(value)?;
            output.push('<');
            output.push_str(value);
            output.push('>');
            Ok(())
        }
    }
}

fn write_endpoint(output: &mut String, endpoint: &Endpoint) -> Result<(), GraphvizError> {
    write_quoted(output, &endpoint.node)?;
    if let Some(port) = &endpoint.port {
        output.push(':');
        if let Some(name) = &port.name {
            write_quoted(output, name)?;
            if port.compass.is_some() {
                output.push(':');
            }
        }
        if let Some(compass) = port.compass {
            write_compass(output, compass);
        }
    }
    Ok(())
}

fn write_quoted(output: &mut String, value: &str) -> Result<(), GraphvizError> {
    if value
        .chars()
        .rev()
        .take_while(|character| *character == '\\')
        .count()
        % 2
        != 0
    {
        return Err(invalid(
            "quoted DOT strings cannot end in an odd number of backslashes",
        ));
    }
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\0' => return Err(invalid("DOT strings cannot contain NUL bytes")),
            character if character.is_control() && character != '\t' => {
                return Err(invalid(format!(
                    "DOT strings cannot contain control character U+{:04X}",
                    character as u32
                )));
            }
            character => output.push(character),
        }
    }
    output.push('"');
    Ok(())
}

fn validate_html_label(value: &str) -> Result<(), GraphvizError> {
    if value.contains('\0') {
        return Err(invalid("HTML labels cannot contain NUL bytes"));
    }
    let mut quote = None;
    let mut angle_depth = 0usize;
    for character in value.chars() {
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' if angle_depth > 0 => quote = Some(character),
            '<' => angle_depth += 1,
            '>' => {
                angle_depth = angle_depth
                    .checked_sub(1)
                    .ok_or_else(|| invalid("HTML labels require matched angle brackets"))?;
            }
            _ => {}
        }
    }
    if quote.is_some() || angle_depth != 0 {
        return Err(invalid(
            "HTML labels require matched quotes and angle brackets",
        ));
    }
    Ok(())
}

fn finite_number(value: f64, context: &str) -> Result<String, GraphvizError> {
    if !value.is_finite() {
        return Err(invalid(format!("{context} must be finite")));
    }
    Ok(value.to_string())
}

fn write_indent(output: &mut String, depth: usize) {
    for _ in 0..depth {
        output.push_str("  ");
    }
}

fn write_compass(output: &mut String, compass: CompassPoint) {
    output.push_str(match compass {
        CompassPoint::North => "n",
        CompassPoint::NorthEast => "ne",
        CompassPoint::East => "e",
        CompassPoint::SouthEast => "se",
        CompassPoint::South => "s",
        CompassPoint::SouthWest => "sw",
        CompassPoint::West => "w",
        CompassPoint::NorthWest => "nw",
        CompassPoint::Center => "c",
        CompassPoint::Any => "_",
    });
}

fn node_style_value(style: NodeStyle) -> &'static str {
    match style {
        NodeStyle::Filled => "filled",
        NodeStyle::Invisible => "invis",
        NodeStyle::Diagonals => "diagonals",
        NodeStyle::Rounded => "rounded",
        NodeStyle::Dashed => "dashed",
        NodeStyle::Dotted => "dotted",
        NodeStyle::Solid => "solid",
        NodeStyle::Bold => "bold",
    }
}

fn edge_style_value(style: EdgeStyle) -> &'static str {
    match style {
        EdgeStyle::Solid => "solid",
        EdgeStyle::Dashed => "dashed",
        EdgeStyle::Dotted => "dotted",
        EdgeStyle::Bold => "bold",
        EdgeStyle::Invisible => "invis",
    }
}

fn arrow_shape_value(shape: ArrowShape) -> &'static str {
    match shape {
        ArrowShape::Normal => "normal",
        ArrowShape::Inverted => "inv",
        ArrowShape::Dot => "dot",
        ArrowShape::OpenDot => "odot",
        ArrowShape::InvertedDot => "invdot",
        ArrowShape::OpenInvertedDot => "invodot",
        ArrowShape::Tee => "tee",
        ArrowShape::Empty => "empty",
        ArrowShape::InvertedEmpty => "invempty",
        ArrowShape::Open => "open",
        ArrowShape::HalfOpen => "halfopen",
        ArrowShape::Diamond => "diamond",
        ArrowShape::OpenDiamond => "odiamond",
        ArrowShape::Box => "box",
        ArrowShape::OpenBox => "obox",
        ArrowShape::Crow => "crow",
        ArrowShape::None => "none",
    }
}

fn node_shape_value(shape: NodeShape) -> &'static str {
    match shape {
        NodeShape::Box => "box",
        NodeShape::Polygon => "polygon",
        NodeShape::Ellipse => "ellipse",
        NodeShape::Circle => "circle",
        NodeShape::Point => "point",
        NodeShape::Egg => "egg",
        NodeShape::Triangle => "triangle",
        NodeShape::PlainText => "plaintext",
        NodeShape::Plain => "plain",
        NodeShape::Diamond => "diamond",
        NodeShape::Trapezium => "trapezium",
        NodeShape::Parallelogram => "parallelogram",
        NodeShape::House => "house",
        NodeShape::Pentagon => "pentagon",
        NodeShape::Hexagon => "hexagon",
        NodeShape::Septagon => "septagon",
        NodeShape::Octagon => "octagon",
        NodeShape::DoubleCircle => "doublecircle",
        NodeShape::DoubleOctagon => "doubleoctagon",
        NodeShape::TripleOctagon => "tripleoctagon",
        NodeShape::InvertedTriangle => "invtriangle",
        NodeShape::InvertedTrapezium => "invtrapezium",
        NodeShape::InvertedHouse => "invhouse",
        NodeShape::Square => "square",
        NodeShape::Star => "star",
        NodeShape::Underline => "underline",
        NodeShape::Cylinder => "cylinder",
        NodeShape::Note => "note",
        NodeShape::Tab => "tab",
        NodeShape::Folder => "folder",
        NodeShape::ThreeDimensionalBox => "box3d",
        NodeShape::Component => "component",
        NodeShape::Promoter => "promoter",
        NodeShape::CodingSequence => "cds",
        NodeShape::Terminator => "terminator",
        NodeShape::UntranslatedRegion => "utr",
        NodeShape::PrimerSite => "primersite",
        NodeShape::RestrictionSite => "restrictionsite",
        NodeShape::FivePrimeOverhang => "fivepoverhang",
        NodeShape::ThreePrimeOverhang => "threepoverhang",
        NodeShape::NoOverhang => "noverhang",
        NodeShape::Assembly => "assembly",
        NodeShape::Signature => "signature",
        NodeShape::Insulator => "insulator",
        NodeShape::RiboSite => "ribosite",
        NodeShape::RnaStability => "rnastab",
        NodeShape::ProteaseSite => "proteasesite",
        NodeShape::ProteinStability => "proteinstab",
        NodeShape::ReversePromoter => "rpromoter",
        NodeShape::RightArrow => "rarrow",
        NodeShape::LeftArrow => "larrow",
        NodeShape::LeftPromoter => "lpromoter",
        NodeShape::Record => "record",
        NodeShape::RoundedRecord => "Mrecord",
    }
}

pub(crate) fn layout_name(layout: LayoutEngine) -> &'static str {
    match layout {
        LayoutEngine::Dot => "dot",
        LayoutEngine::Neato => "neato",
        LayoutEngine::Fdp => "fdp",
        LayoutEngine::Sfdp => "sfdp",
        LayoutEngine::Circo => "circo",
        LayoutEngine::Twopi => "twopi",
        LayoutEngine::Osage => "osage",
        LayoutEngine::Patchwork => "patchwork",
        LayoutEngine::Nop => "nop",
        LayoutEngine::Nop2 => "nop2",
    }
}

pub(crate) fn format_name(format: RenderFormat) -> &'static str {
    match format {
        RenderFormat::Ascii => "ascii",
        RenderFormat::Bmp => "bmp",
        RenderFormat::Canon => "canon",
        RenderFormat::Dot => "dot",
        RenderFormat::DotJson => "dot_json",
        RenderFormat::Eps => "eps",
        RenderFormat::Fig => "fig",
        RenderFormat::Gd => "gd",
        RenderFormat::Gd2 => "gd2",
        RenderFormat::Gif => "gif",
        RenderFormat::Jpeg => "jpeg",
        RenderFormat::Json => "json",
        RenderFormat::Json0 => "json0",
        RenderFormat::Pdf => "pdf",
        RenderFormat::Pic => "pic",
        RenderFormat::Plain => "plain",
        RenderFormat::PlainExt => "plain-ext",
        RenderFormat::Png => "png",
        RenderFormat::PostScript => "ps",
        RenderFormat::PostScript2 => "ps2",
        RenderFormat::Pov => "pov",
        RenderFormat::Svg => "svg",
        RenderFormat::Svgz => "svgz",
        RenderFormat::Tga => "tga",
        RenderFormat::Tiff => "tiff",
        RenderFormat::Vml => "vml",
        RenderFormat::Vmlz => "vmlz",
        RenderFormat::Vrml => "vrml",
        RenderFormat::Webp => "webp",
        RenderFormat::XDot => "xdot",
        RenderFormat::XDot12 => "xdot1.2",
        RenderFormat::XDot14 => "xdot1.4",
        RenderFormat::XDotJson => "xdot_json",
    }
}

fn format_extension(format: RenderFormat) -> &'static str {
    match format {
        RenderFormat::Canon | RenderFormat::Dot => "dot",
        RenderFormat::XDot | RenderFormat::XDot12 | RenderFormat::XDot14 => "xdot",
        RenderFormat::DotJson
        | RenderFormat::Json
        | RenderFormat::Json0
        | RenderFormat::XDotJson => "json",
        RenderFormat::Plain | RenderFormat::PlainExt => "txt",
        RenderFormat::PostScript | RenderFormat::PostScript2 => "ps",
        RenderFormat::Jpeg => "jpg",
        RenderFormat::Tiff => "tiff",
        RenderFormat::Ascii => "txt",
        RenderFormat::Bmp => "bmp",
        RenderFormat::Eps => "eps",
        RenderFormat::Fig => "fig",
        RenderFormat::Gd => "gd",
        RenderFormat::Gd2 => "gd2",
        RenderFormat::Gif => "gif",
        RenderFormat::Pdf => "pdf",
        RenderFormat::Pic => "pic",
        RenderFormat::Png => "png",
        RenderFormat::Pov => "pov",
        RenderFormat::Svg => "svg",
        RenderFormat::Svgz => "svgz",
        RenderFormat::Tga => "tga",
        RenderFormat::Vml => "vml",
        RenderFormat::Vmlz => "vmlz",
        RenderFormat::Vrml => "vrml",
        RenderFormat::Webp => "webp",
    }
}

pub(crate) fn invalid(message: impl Into<String>) -> GraphvizError {
    GraphvizError {
        kind: GraphvizErrorKind::InvalidGraph,
        exit_code: None,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Label {
        Label::Text(value.to_owned())
    }

    fn endpoint(node: &str) -> Endpoint {
        Endpoint {
            node: node.to_owned(),
            port: None,
        }
    }

    fn edge(from: &str, to: &str) -> Edge {
        Edge {
            from: endpoint(from),
            to: endpoint(to),
            attributes: EdgeAttributes::default(),
        }
    }

    #[test]
    fn serializes_semantic_graph_in_canonical_phases() {
        let node_defaults = NodeAttributes {
            shape: Some(NodeShape::Box),
            ..NodeAttributes::default()
        };
        let graph = Graph {
            strict: true,
            kind: GraphKind::Directed,
            id: Some("workflow".to_owned()),
            attributes: GraphAttributes {
                rank_direction: Some(RankDirection::LeftToRight),
                ..GraphAttributes::default()
            },
            node_defaults,
            edge_defaults: EdgeAttributes {
                colors: Some(vec!["navy".to_owned()]),
                ..EdgeAttributes::default()
            },
            nodes: BTreeMap::from([("outside".to_owned(), NodeAttributes::default())]),
            edges: vec![edge("inside", "outside")],
            subgraphs: BTreeMap::from([(
                "build".to_owned(),
                Subgraph {
                    kind: SubgraphKind::Cluster,
                    id: Some("build".to_owned()),
                    attributes: SubgraphAttributes {
                        label: Some(text("Build")),
                        padding: Some(16.0),
                        ..SubgraphAttributes::default()
                    },
                    nodes: BTreeMap::from([(
                        "inside".to_owned(),
                        NodeAttributes {
                            label: Some(Label::Html("<B>inside</B>".to_owned())),
                            ..NodeAttributes::default()
                        },
                    )]),
                    edges: Vec::new(),
                    subgraphs: Vec::new(),
                },
            )]),
        };

        let source = serialize_graph(&graph).unwrap();
        assert!(source.starts_with("strict digraph \"workflow\" {\n"));
        assert!(source.contains("graph [\"rankdir\"=\"LR\"]"));
        assert!(source.contains("node [\"shape\"=\"box\"]"));
        assert!(source.contains("edge [\"color\"=\"navy\"]"));
        assert!(source.contains("subgraph \"cluster_build\""));
        assert!(source.contains("\"margin\"=\"16\""));
        assert!(source.contains("\"inside\" [\"label\"=<<B>inside</B>>]"));
        assert!(source.contains("\"inside\" -> \"outside\""));
    }

    #[test]
    fn render_plan_reads_the_program_from_a_declared_input() {
        let source = blake3::hash(b"DOT program");
        let plan = render_plan(source, LayoutEngine::Dot, RenderFormat::Svg);

        assert_eq!(plan.inputs.len(), 1);
        assert_eq!(plan.inputs[0].hash, source);
        assert_eq!(plan.inputs[0].extension, "dot");
        assert_eq!(plan.inputs[0].kind, InputKind::Blob);
        assert_eq!(plan.arguments.last(), Some(&ToolArgument::input(0)));
    }

    #[test]
    fn serializes_typed_node_and_edge_attributes() {
        let node_attributes = NodeAttributes {
            label: Some(Label::Html("<B>typed node</B>".to_owned())),
            font: Some(Font {
                family: Some("DejaVu Sans".to_owned()),
                size_points: Some(12.0),
                color: Some("navy".to_owned()),
            }),
            size: Some(NodeSize {
                width: Some(2.0),
                height: Some(1.0),
                sizing: Some(NodeSizing::FixedShape),
            }),
            shape: Some(NodeShape::Polygon),
            polygon: Some(PolygonOptions {
                regular: Some(true),
                peripheries: Some(2),
                sides: Some(6),
                orientation_degrees: Some(30.0),
                distortion: Some(0.25),
                skew: Some(-0.1),
            }),
            outline_color: Some("blue".to_owned()),
            fill_color: Some("lightblue".to_owned()),
            styles: Some(vec![NodeStyle::Rounded, NodeStyle::Filled]),
            external_label: Some(text("outside")),
            link: Some(Link {
                url: "https://example.com/node".to_owned(),
                target: Some("_blank".to_owned()),
                tooltip: Some("node details".to_owned()),
            }),
            extra: BTreeMap::from([("pin".to_owned(), "true".to_owned())]),
        };
        let edge_attributes = EdgeAttributes {
            label: Some(text("main label")),
            font: Some(Font {
                family: Some("DejaVu Serif".to_owned()),
                size_points: Some(10.0),
                color: Some("black".to_owned()),
            }),
            weight: Some(2.5),
            styles: Some(vec![EdgeStyle::Dashed, EdgeStyle::Bold]),
            colors: Some(vec!["red".to_owned(), "blue".to_owned()]),
            direction: Some(EdgeDirection::Both),
            arrow_scale: Some(1.5),
            head: Some(EdgeEnd {
                arrow: Some(ArrowShape::OpenDiamond),
                clip_to_node: Some(false),
                link: None,
                port_group: Some("shared-head".to_owned()),
            }),
            tail: Some(EdgeEnd {
                arrow: Some(ArrowShape::Crow),
                clip_to_node: Some(true),
                link: None,
                port_group: Some("shared-tail".to_owned()),
            }),
            endpoint_labels: Some(EdgeEndpointLabels {
                head: Some(text("head")),
                tail: Some(text("tail")),
                font: None,
                distance: Some(1.25),
                angle_degrees: Some(20.0),
            }),
            link: Some(Link {
                url: "https://example.com/edge".to_owned(),
                target: None,
                tooltip: Some("edge details".to_owned()),
            }),
            decorate_label: Some(true),
            extra: BTreeMap::from([("constraint".to_owned(), "false".to_owned())]),
        };
        let graph = Graph {
            nodes: BTreeMap::from([
                ("a".to_owned(), node_attributes),
                ("b".to_owned(), NodeAttributes::default()),
            ]),
            edges: vec![Edge {
                from: endpoint("a"),
                to: endpoint("b"),
                attributes: edge_attributes,
            }],
            ..Graph::default()
        };

        let source = serialize_graph(&graph).unwrap();
        assert!(source.contains("\"fixedsize\"=\"shape\""));
        assert!(source.contains("\"shape\"=\"polygon\""));
        assert!(source.contains("\"style\"=\"rounded,filled\""));
        assert!(source.contains("\"color\"=\"red:blue\""));
        assert!(source.contains("\"arrowhead\"=\"odiamond\""));
        assert!(source.contains("\"constraint\"=\"false\""));
    }

    #[test]
    fn rejects_undefined_and_duplicate_nodes() {
        let graph = Graph {
            nodes: BTreeMap::from([("a".to_owned(), NodeAttributes::default())]),
            edges: vec![edge("a", "missing")],
            ..Graph::default()
        };
        assert!(
            serialize_graph(&graph)
                .unwrap_err()
                .message
                .contains("undefined node")
        );

        let graph = Graph {
            nodes: BTreeMap::from([("a".to_owned(), NodeAttributes::default())]),
            subgraphs: BTreeMap::from([(
                "group".to_owned(),
                Subgraph {
                    nodes: BTreeMap::from([("a".to_owned(), NodeAttributes::default())]),
                    ..Subgraph::default()
                },
            )]),
            ..Graph::default()
        };
        assert!(
            serialize_graph(&graph)
                .unwrap_err()
                .message
                .contains("defined more than once")
        );
    }

    #[test]
    fn rejects_invalid_subgraph_hierarchies_and_ports() {
        let graph = Graph {
            subgraphs: BTreeMap::from([(
                "parent".to_owned(),
                Subgraph {
                    subgraphs: vec!["missing".to_owned()],
                    ..Subgraph::default()
                },
            )]),
            ..Graph::default()
        };
        assert!(serialize_graph(&graph).is_err());

        let graph = Graph {
            subgraphs: BTreeMap::from([
                (
                    "a".to_owned(),
                    Subgraph {
                        subgraphs: vec!["b".to_owned()],
                        ..Subgraph::default()
                    },
                ),
                (
                    "b".to_owned(),
                    Subgraph {
                        subgraphs: vec!["a".to_owned()],
                        ..Subgraph::default()
                    },
                ),
            ]),
            ..Graph::default()
        };
        assert!(
            serialize_graph(&graph)
                .unwrap_err()
                .message
                .contains("cycle")
        );

        let graph = Graph {
            nodes: BTreeMap::from([
                ("a".to_owned(), NodeAttributes::default()),
                ("b".to_owned(), NodeAttributes::default()),
            ]),
            edges: vec![Edge {
                from: Endpoint {
                    node: "a".to_owned(),
                    port: Some(Port {
                        name: None,
                        compass: None,
                    }),
                },
                to: endpoint("b"),
                attributes: EdgeAttributes::default(),
            }],
            ..Graph::default()
        };
        assert!(
            serialize_graph(&graph)
                .unwrap_err()
                .message
                .contains("empty port")
        );
    }

    #[test]
    fn rejects_invalid_attribute_values() {
        let error = node_attributes(&NodeAttributes {
            styles: Some(Vec::new()),
            ..NodeAttributes::default()
        })
        .unwrap_err();
        assert!(error.message.contains("node styles"));

        let error = edge_attributes(&EdgeAttributes {
            colors: Some(Vec::new()),
            ..EdgeAttributes::default()
        })
        .unwrap_err();
        assert!(error.message.contains("edge colors"));

        let error = graph_attributes(&GraphAttributes {
            ratio: Some(GraphRatio::Value(f64::NAN)),
            ..GraphAttributes::default()
        })
        .unwrap_err();
        assert!(error.message.contains("finite"));

        let error = subgraph_attributes(&SubgraphAttributes {
            padding: Some(-1.0),
            ..SubgraphAttributes::default()
        })
        .unwrap_err();
        assert!(error.message.contains("cannot be negative"));

        let error = node_attributes(&NodeAttributes {
            fill_color: Some("blue".to_owned()),
            extra: BTreeMap::from([("fillcolor".to_owned(), "red".to_owned())]),
            ..NodeAttributes::default()
        })
        .unwrap_err();
        assert!(error.message.contains("both a typed field and `extra`"));
    }
}
