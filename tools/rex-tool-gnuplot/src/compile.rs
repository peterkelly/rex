use super::types::*;
use crate::modules::tools::executor::{
    CasInput, ExpectedOutput, InputKind, OutputKind, ToolArgument, ToolExecutionPlan, ToolProgram,
};
use blake3::Hash;
use chrono::{DateTime, Utc};
use std::fmt::Write;

const MAX_PANELS: usize = 256;
const MAX_DIMENSION: u64 = 32_768;

#[derive(Clone, Copy, Debug)]
pub(crate) enum RenderTerminal {
    Png(PngOptions),
    Svg(SvgOptions),
    Pdf(PdfOptions),
}

impl RenderTerminal {
    fn extension(self) -> &'static str {
        match self {
            Self::Png(_) => "png",
            Self::Svg(_) => "svg",
            Self::Pdf(_) => "pdf",
        }
    }
}

pub(crate) fn render_plan(source: Hash, terminal: RenderTerminal) -> ToolExecutionPlan {
    ToolExecutionPlan {
        program: ToolProgram::new("gnuplot"),
        arguments: vec![
            ToolArgument::literal("-e"),
            ToolArgument::joined(vec![
                ToolArgument::literal("output_file=\""),
                ToolArgument::output(0),
                ToolArgument::literal("\""),
            ]),
            ToolArgument::input(0),
        ],
        inputs: vec![CasInput {
            hash: source,
            extension: "gp".to_owned(),
            kind: InputKind::Blob,
        }],
        outputs: vec![ExpectedOutput {
            kind: OutputKind::Single,
            extension: terminal.extension().to_owned(),
        }],
    }
}

pub(crate) fn version_plan() -> ToolExecutionPlan {
    ToolExecutionPlan {
        program: ToolProgram::new("gnuplot"),
        arguments: vec![ToolArgument::literal("--version")],
        inputs: Vec::new(),
        outputs: Vec::new(),
    }
}

pub(crate) fn serialize_figure(
    figure: &Figure,
    terminal: RenderTerminal,
) -> Result<String, GnuplotError> {
    Serializer::new(figure, terminal).serialize()
}

struct Serializer<'a> {
    figure: &'a Figure,
    terminal: RenderTerminal,
    data_blocks: Vec<String>,
    next_tag: usize,
}

impl<'a> Serializer<'a> {
    fn new(figure: &'a Figure, terminal: RenderTerminal) -> Self {
        Self {
            figure,
            terminal,
            data_blocks: Vec::new(),
            next_tag: 1,
        }
    }

    fn serialize(mut self) -> Result<String, GnuplotError> {
        let (rows, columns) = validate_figure(self.figure)?;
        validate_terminal(self.terminal)?;
        validate_theme(&self.figure.theme)?;

        let mut panels = Vec::with_capacity(self.figure.panels.len());
        for panel in &self.figure.panels {
            panels.push(match panel {
                Some(Panel::TwoDimensional(plot)) => Some(self.serialize_plot2d(plot)?),
                Some(Panel::ThreeDimensional(plot)) => Some(self.serialize_plot3d(plot)?),
                None => None,
            });
        }

        let mut output = String::new();
        output.push_str("reset\nset encoding utf8\n");
        self.write_terminal(&mut output)?;
        output.push_str("set output output_file\n");
        output.push_str("set datafile separator whitespace\n");
        for (index, block) in self.data_blocks.iter().enumerate() {
            writeln!(output, "$REX_DATA_{index} << EOD").unwrap();
            output.push_str(block);
            if !block.ends_with('\n') {
                output.push('\n');
            }
            output.push_str("EOD\n");
        }

        write!(output, "set multiplot").unwrap();
        if let Some(title) = &self.figure.title {
            write!(output, " title {}", quoted(title)?).unwrap();
        }
        write!(output, " layout {rows},{columns}").unwrap();
        match self.figure.layout.fill_order {
            GridFillOrder::RowsFirst => output.push_str(" rowsfirst"),
            GridFillOrder::ColumnsFirst => output.push_str(" columnsfirst"),
        }
        writeln!(
            output,
            " downwards margins 0.05,0.95,0.05,0.95 spacing {},{}",
            number(self.figure.layout.horizontal_spacing)?,
            number(self.figure.layout.vertical_spacing)?
        )
        .unwrap();

        for panel in panels {
            match panel {
                Some(script) => output.push_str(&script),
                None => output.push_str("set multiplot next\n"),
            }
        }
        output.push_str("unset multiplot\nunset output\n");
        Ok(output)
    }

    fn write_terminal(&self, output: &mut String) -> Result<(), GnuplotError> {
        let theme = &self.figure.theme;
        let font = format!("{},{}", theme.font.family, number(theme.font.size_points)?);
        let background = quoted(&theme.background_color)?;
        match self.terminal {
            RenderTerminal::Png(options) => {
                writeln!(
                    output,
                    "set terminal pngcairo size {},{} noenhanced font {} {} background rgb {}",
                    options.width_px,
                    options.height_px,
                    quoted(&font)?,
                    if options.transparent {
                        "transparent"
                    } else {
                        "notransparent"
                    },
                    background
                )
                .unwrap();
            }
            RenderTerminal::Svg(options) => {
                writeln!(
                    output,
                    "set terminal svg size {},{} noenhanced font {} background rgb {}",
                    options.width_px,
                    options.height_px,
                    quoted(&font)?,
                    background
                )
                .unwrap();
            }
            RenderTerminal::Pdf(options) => {
                writeln!(
                    output,
                    "set terminal pdfcairo size {}in,{}in noenhanced font {} background rgb {}",
                    number(options.width_inches)?,
                    number(options.height_inches)?,
                    quoted(&font)?,
                    background
                )
                .unwrap();
            }
        }
        Ok(())
    }

    fn serialize_plot2d(&mut self, plot: &Plot2D) -> Result<String, GnuplotError> {
        if plot.series.is_empty() {
            return Err(invalid("a 2-D panel must contain at least one series"));
        }
        let palette = plot.palette.as_ref().unwrap_or(&self.figure.theme.palette);
        validate_palette(palette)?;
        let bar_count = plot
            .series
            .iter()
            .filter(|series| matches!(series, Series2D::Bar(_)))
            .count();
        if bar_count > 0 && (bar_count != 1 || plot.series.len() != 1) {
            return Err(invalid(
                "a categorical bar chart must be the only series in its panel",
            ));
        }
        let domains = domains_2d(plot)?;
        validate_secondary_axes(plot)?;

        let mut output = panel_reset();
        self.write_common_panel(&mut output, &plot.legend, &plot.grid, palette)?;
        self.write_axis(
            &mut output,
            "x",
            &plot.x_axis,
            domains.x1,
            &self.figure.theme,
        )?;
        self.write_axis(
            &mut output,
            "y",
            &plot.y_axis,
            AxisDomain::Numeric,
            &self.figure.theme,
        )?;
        if let Some(axis) = &plot.x2_axis {
            output.push_str("set x2tics\n");
            self.write_axis(&mut output, "x2", axis, domains.x2, &self.figure.theme)?;
        }
        if let Some(axis) = &plot.y2_axis {
            output.push_str("set y2tics\n");
            self.write_axis(
                &mut output,
                "y2",
                axis,
                AxisDomain::Numeric,
                &self.figure.theme,
            )?;
        }
        self.write_axis(
            &mut output,
            "cb",
            &plot.color_axis,
            AxisDomain::Numeric,
            &self.figure.theme,
        )?;
        write_optional_title(
            &mut output,
            plot.title.as_deref(),
            &self.figure.theme.foreground_color,
        )?;
        match plot.aspect_ratio {
            Some(value) if value.is_finite() && value > 0.0 => {
                writeln!(output, "set size ratio {}", number(value)?).unwrap();
            }
            Some(_) => {
                return Err(invalid(
                    "2-D aspect_ratio must be finite and greater than zero",
                ));
            }
            None => output.push_str("set size noratio\n"),
        }
        if plot.show_colorbox {
            output.push_str("set colorbox\n");
        } else {
            output.push_str("unset colorbox\n");
        }
        self.write_annotations2d(&mut output, &plot.annotations)?;

        if let [Series2D::Bar(chart)] = plot.series.as_slice() {
            let (setting, clauses) = self.serialize_bar_chart(chart)?;
            output.push_str(&setting);
            writeln!(output, "plot {}", clauses.join(", \\\n    ")).unwrap();
            return Ok(output);
        }

        let mut clauses = Vec::new();
        for (index, series) in plot.series.iter().enumerate() {
            let fallback = cycle_color(&self.figure.theme, index)?;
            self.serialize_series2d(series, fallback, &mut clauses)?;
        }
        writeln!(output, "plot {}", clauses.join(", \\\n    ")).unwrap();
        Ok(output)
    }

    fn serialize_plot3d(&mut self, plot: &Plot3D) -> Result<String, GnuplotError> {
        if plot.series.is_empty() {
            return Err(invalid("a 3-D panel must contain at least one series"));
        }
        let palette = plot.palette.as_ref().unwrap_or(&self.figure.theme.palette);
        validate_palette(palette)?;
        let contour_lines = plot
            .series
            .iter()
            .filter(|series| {
                matches!(
                    series,
                    Series3D::Surface(Surface3D {
                        mode: SurfaceMode::ContourLines,
                        ..
                    })
                )
            })
            .count();
        if contour_lines > 0 && contour_lines != plot.series.len() {
            return Err(invalid(
                "contour-line surfaces cannot share a panel with other 3-D series",
            ));
        }
        validate_view(plot.view)?;

        let mut output = panel_reset();
        self.write_common_panel(&mut output, &plot.legend, &plot.grid, palette)?;
        self.write_axis(
            &mut output,
            "x",
            &plot.x_axis,
            AxisDomain::Numeric,
            &self.figure.theme,
        )?;
        self.write_axis(
            &mut output,
            "y",
            &plot.y_axis,
            AxisDomain::Numeric,
            &self.figure.theme,
        )?;
        self.write_axis(
            &mut output,
            "z",
            &plot.z_axis,
            AxisDomain::Numeric,
            &self.figure.theme,
        )?;
        self.write_axis(
            &mut output,
            "cb",
            &plot.color_axis,
            AxisDomain::Numeric,
            &self.figure.theme,
        )?;
        write_optional_title(
            &mut output,
            plot.title.as_deref(),
            &self.figure.theme.foreground_color,
        )?;
        if plot.show_colorbox {
            output.push_str("set colorbox\n");
        } else {
            output.push_str("unset colorbox\n");
        }
        writeln!(
            output,
            "set view {},{},{}",
            number(plot.view.elevation_degrees)?,
            number(plot.view.azimuth_degrees)?,
            number(plot.view.scale)?
        )
        .unwrap();
        if contour_lines > 0 {
            output.push_str("set contour base\nunset surface\n");
        } else {
            output.push_str("unset contour\nset surface\n");
        }
        self.write_annotations3d(&mut output, &plot.annotations)?;

        let mut clauses = Vec::new();
        for (index, series) in plot.series.iter().enumerate() {
            let fallback = cycle_color(&self.figure.theme, index)?;
            self.serialize_series3d(series, fallback, &mut clauses)?;
        }
        writeln!(output, "splot {}", clauses.join(", \\\n    ")).unwrap();
        Ok(output)
    }

    fn write_common_panel(
        &self,
        output: &mut String,
        legend: &Legend,
        grid: &Grid,
        palette: &Palette,
    ) -> Result<(), GnuplotError> {
        let theme = &self.figure.theme;
        writeln!(
            output,
            "set border lc rgb {}\nset tics textcolor rgb {}",
            quoted(&theme.foreground_color)?,
            quoted(&theme.foreground_color)?
        )
        .unwrap();
        write_legend(output, legend, &theme.foreground_color)?;
        write_grid(output, grid, theme)?;
        write_palette(output, palette)?;
        Ok(())
    }

    fn write_axis(
        &self,
        output: &mut String,
        name: &str,
        axis: &Axis,
        domain: AxisDomain,
        theme: &Theme,
    ) -> Result<(), GnuplotError> {
        validate_axis(name, axis, domain)?;
        if name == "x" || name == "x2" || name == "y" || name == "y2" {
            match domain {
                AxisDomain::Time => {
                    output.push_str("set timefmt \"%s\"\n");
                    writeln!(output, "set {name}data time").unwrap();
                }
                AxisDomain::Numeric | AxisDomain::Unspecified => {
                    writeln!(output, "set {name}data").unwrap()
                }
            }
        }
        match axis.scale {
            AxisScale::Linear => writeln!(output, "unset logscale {name}").unwrap(),
            AxisScale::Log(base) => {
                writeln!(output, "set logscale {name} {}", number(base)?).unwrap()
            }
        }
        let range = range_text(&axis.range)?;
        writeln!(
            output,
            "set {name}range {range} {}",
            if axis.reversed {
                "reverse"
            } else {
                "noreverse"
            }
        )
        .unwrap();
        match &axis.tick_format {
            TickFormat::Automatic => writeln!(output, "set format {name}").unwrap(),
            TickFormat::Numeric(format) | TickFormat::Time(format) => {
                writeln!(output, "set format {name} {}", quoted(format)?).unwrap()
            }
        }
        match &axis.label {
            Some(label) => writeln!(
                output,
                "set {name}label {} textcolor rgb {}",
                quoted(label)?,
                quoted(&theme.foreground_color)?
            )
            .unwrap(),
            None => writeln!(output, "unset {name}label").unwrap(),
        }
        Ok(())
    }

    fn serialize_series2d(
        &mut self,
        series: &Series2D,
        fallback: &str,
        clauses: &mut Vec<String>,
    ) -> Result<(), GnuplotError> {
        match series {
            Series2D::Curve(curve) => {
                let block = serialize_xy_data(&curve.data)?;
                let data = self.add_data(block);
                let title = title_clause(curve.title.as_deref())?;
                let axes = axes_clause(curve.axes);
                let style = curve_style(curve, fallback)?;
                clauses.push(format!("{data} using 1:2 {axes} {title} {style}"));
            }
            Series2D::Error(errors) => {
                let (block, kind) = serialize_errors(errors)?;
                let data = self.add_data(block);
                let title = title_clause(errors.title.as_deref())?;
                let axes = axes_clause(errors.axes);
                let line = line_options(&errors.line, fallback, true)?;
                let points = point_options(&errors.points, fallback, false)?;
                let with = match (kind, errors.connected) {
                    (ErrorKind::X, false) => "xerrorbars",
                    (ErrorKind::Y, false) => "yerrorbars",
                    (ErrorKind::XY, false) => "xyerrorbars",
                    (ErrorKind::X, true) => "xerrorlines",
                    (ErrorKind::Y, true) => "yerrorlines",
                    (ErrorKind::XY, true) => "xyerrorlines",
                };
                clauses.push(format!(
                    "{data} using {} {axes} {title} with {with} {line} {points}",
                    error_using(kind)
                ));
            }
            Series2D::Band(band) => {
                let block = serialize_band(&band.data)?;
                let data = self.add_data(block);
                let title = title_clause(band.title.as_deref())?;
                let axes = axes_clause(band.axes);
                let fill = fill_options(&band.fill, fallback)?;
                clauses.push(format!(
                    "{data} using 1:2:3 {axes} {title} with filledcurves {fill}"
                ));
            }
            Series2D::Histogram(histogram) => {
                let block = serialize_histogram(histogram)?;
                let data = self.add_data(block);
                let title = title_clause(histogram.title.as_deref())?;
                let axes = axes_clause(histogram.axes);
                let fill = fill_options(&histogram.fill, fallback)?;
                clauses.push(format!(
                    "{data} using 1:2:3 {axes} {title} with boxes {fill}"
                ));
            }
            Series2D::Heatmap(heatmap) => {
                validate_grid(&heatmap.grid, "heatmap")?;
                let block = serialize_grid(&heatmap.grid)?;
                let data = self.add_data(block);
                let title = title_clause(heatmap.title.as_deref())?;
                let axes = axes_clause(heatmap.axes);
                clauses.push(format!("{data} using 1:2:3 {axes} {title} with image"));
            }
            Series2D::Vector(vectors) => {
                if vectors.data.is_empty() {
                    return Err(invalid("a vector series must contain at least one vector"));
                }
                let mut block = String::new();
                for vector in &vectors.data {
                    for value in [vector.x, vector.y, vector.dx, vector.dy] {
                        validate_finite(value, "vector coordinate")?;
                    }
                    writeln!(
                        block,
                        "{} {} {} {}",
                        number(vector.x)?,
                        number(vector.y)?,
                        number(vector.dx)?,
                        number(vector.dy)?
                    )
                    .unwrap();
                }
                let data = self.add_data(block);
                let title = title_clause(vectors.title.as_deref())?;
                let axes = axes_clause(vectors.axes);
                let line = line_options(&vectors.line, fallback, true)?;
                clauses.push(format!(
                    "{data} using 1:2:3:4 {axes} {title} with vectors {} {line}",
                    arrow_head(vectors.head)
                ));
            }
            Series2D::Label(labels) => {
                if labels.data.is_empty() {
                    return Err(invalid("a label series must contain at least one label"));
                }
                validate_font(&labels.font)?;
                let mut block = String::new();
                for label in &labels.data {
                    validate_finite(label.x, "label x")?;
                    validate_finite(label.y, "label y")?;
                    writeln!(
                        block,
                        "{} {} {}",
                        number(label.x)?,
                        number(label.y)?,
                        data_string(&label.text)?
                    )
                    .unwrap();
                }
                let data = self.add_data(block);
                let title = title_clause(labels.title.as_deref())?;
                let axes = axes_clause(labels.axes);
                let color = labels.color.as_deref().unwrap_or(fallback);
                clauses.push(format!(
                    "{data} using 1:2:3 {axes} {title} with labels {} font {} textcolor rgb {}",
                    alignment(labels.alignment),
                    quoted(&format!(
                        "{},{}",
                        labels.font.family,
                        number(labels.font.size_points)?
                    ))?,
                    quoted(color)?
                ));
            }
            Series2D::Bar(_) => {
                return Err(invalid("internal bar-chart placement error"));
            }
        }
        Ok(())
    }

    fn serialize_bar_chart(
        &mut self,
        chart: &BarChart,
    ) -> Result<(String, Vec<String>), GnuplotError> {
        if chart.series.is_empty() {
            return Err(invalid("a bar chart must contain at least one bar series"));
        }
        validate_finite(chart.gap, "bar gap")?;
        if chart.gap < 0.0 {
            return Err(invalid("bar gap cannot be negative"));
        }
        let categories: Vec<&str> = chart.series[0]
            .values
            .iter()
            .map(|(category, _)| category.as_str())
            .collect();
        if categories.is_empty() {
            return Err(invalid("a bar series must contain at least one value"));
        }
        for series in &chart.series {
            if series.values.len() != categories.len()
                || series
                    .values
                    .iter()
                    .zip(&categories)
                    .any(|((actual, _), expected)| actual != expected)
            {
                return Err(invalid(
                    "all bar series must contain the same categories in the same order",
                ));
            }
            for (_, value) in &series.values {
                validate_finite(*value, "bar value")?;
            }
        }
        let mut block = String::new();
        for (row, category) in categories.iter().enumerate() {
            write!(block, "{}", data_string(category)?).unwrap();
            for series in &chart.series {
                write!(block, " {}", number(series.values[row].1)?).unwrap();
            }
            block.push('\n');
        }
        let data = self.add_data(block);
        let mut clauses = Vec::new();
        for (index, series) in chart.series.iter().enumerate() {
            let fallback = cycle_color(&self.figure.theme, index)?;
            let title = title_clause(series.title.as_deref())?;
            let fill = fill_options(&series.fill, fallback)?;
            clauses.push(format!(
                "{data} using {}:xticlabels(1) {title} with histograms {fill}",
                index + 2
            ));
        }
        let setting = match chart.arrangement {
            BarArrangement::Clustered => {
                format!("set style histogram clustered gap {}\n", number(chart.gap)?)
            }
            BarArrangement::Stacked => "set style histogram rowstacked\n".to_owned(),
        };
        Ok((setting, clauses))
    }

    fn serialize_series3d(
        &mut self,
        series: &Series3D,
        fallback: &str,
        clauses: &mut Vec<String>,
    ) -> Result<(), GnuplotError> {
        match series {
            Series3D::Point(points) => {
                let block = serialize_points3d(&points.data, "point cloud")?;
                let data = self.add_data(block);
                let title = title_clause(points.title.as_deref())?;
                let style = point_options(&points.points, fallback, true)?;
                clauses.push(format!("{data} using 1:2:3 {title} with points {style}"));
            }
            Series3D::Path(path) => {
                let block = serialize_path3d(&path.data)?;
                let data = self.add_data(block);
                let title = title_clause(path.title.as_deref())?;
                let line = line_options(&path.line, fallback, true)?;
                match &path.points {
                    Some(points) => {
                        let point = point_options(points, fallback, false)?;
                        clauses.push(format!(
                            "{data} using 1:2:3 {title} with linespoints {line} {point}"
                        ));
                    }
                    None => clauses.push(format!("{data} using 1:2:3 {title} with lines {line}")),
                }
            }
            Series3D::Surface(surface) => {
                validate_grid(&surface.grid, "surface")?;
                let block = serialize_grid(&surface.grid)?;
                let data = self.add_data(block);
                let title = title_clause(surface.title.as_deref())?;
                let line = line_options(&surface.line, fallback, true)?;
                let clause = match surface.mode {
                    SurfaceMode::Wireframe => {
                        format!("{data} using 1:2:3 {title} with lines {line}")
                    }
                    SurfaceMode::Colored => {
                        format!("{data} using 1:2:3 {title} with pm3d")
                    }
                    SurfaceMode::ContourLines => {
                        format!("{data} using 1:2:3 {title} with lines {line}")
                    }
                    SurfaceMode::FilledContours => {
                        format!("{data} using 1:2:3 {title} with contourfill at base")
                    }
                };
                clauses.push(clause);
            }
        }
        Ok(())
    }

    fn write_annotations2d(
        &mut self,
        output: &mut String,
        annotations: &[Annotation2D],
    ) -> Result<(), GnuplotError> {
        for annotation in annotations {
            let tag = self.take_tag();
            match annotation {
                Annotation2D::Text(text) => {
                    validate_font(&text.font)?;
                    writeln!(
                        output,
                        "set label {tag} {} at {} {} font {} textcolor rgb {} noenhanced",
                        quoted(&text.text)?,
                        position2d(text.position)?,
                        alignment(text.alignment),
                        quoted(&format!(
                            "{},{}",
                            text.font.family,
                            number(text.font.size_points)?
                        ))?,
                        quoted(&self.figure.theme.foreground_color)?
                    )
                    .unwrap();
                }
                Annotation2D::Arrow(arrow) => {
                    let line =
                        line_options(&arrow.line, &self.figure.theme.foreground_color, true)?;
                    writeln!(
                        output,
                        "set arrow {tag} from {} to {} {} {line}",
                        position2d(arrow.from)?,
                        position2d(arrow.to)?,
                        arrow_head(arrow.head)
                    )
                    .unwrap();
                }
                Annotation2D::ReferenceLine(reference) => {
                    validate_finite(reference.value, "reference-line value")?;
                    let line =
                        line_options(&reference.line, &self.figure.theme.foreground_color, true)?;
                    match reference.orientation {
                        ReferenceOrientation::Horizontal => writeln!(
                            output,
                            "set arrow {tag} from graph 0, first {} to graph 1, first {} nohead {line}",
                            number(reference.value)?,
                            number(reference.value)?
                        )
                        .unwrap(),
                        ReferenceOrientation::Vertical => writeln!(
                            output,
                            "set arrow {tag} from first {}, graph 0 to first {}, graph 1 nohead {line}",
                            number(reference.value)?,
                            number(reference.value)?
                        )
                        .unwrap(),
                    }
                    if let Some(label) = &reference.label {
                        let label_tag = self.take_tag();
                        match reference.orientation {
                            ReferenceOrientation::Horizontal => writeln!(
                                output,
                                "set label {label_tag} {} at graph 0.02, first {} left front textcolor rgb {} noenhanced",
                                quoted(label)?,
                                number(reference.value)?,
                                quoted(&self.figure.theme.foreground_color)?
                            )
                            .unwrap(),
                            ReferenceOrientation::Vertical => writeln!(
                                output,
                                "set label {label_tag} {} at first {}, graph 0.98 right front rotate by 90 textcolor rgb {} noenhanced",
                                quoted(label)?,
                                number(reference.value)?,
                                quoted(&self.figure.theme.foreground_color)?
                            )
                            .unwrap(),
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn write_annotations3d(
        &mut self,
        output: &mut String,
        annotations: &[Annotation3D],
    ) -> Result<(), GnuplotError> {
        for annotation in annotations {
            let tag = self.take_tag();
            match annotation {
                Annotation3D::Text(text) => {
                    validate_point3d(text.position, "3-D annotation")?;
                    validate_font(&text.font)?;
                    writeln!(
                        output,
                        "set label {tag} {} at first {}, first {}, first {} {} font {} textcolor rgb {} noenhanced",
                        quoted(&text.text)?,
                        number(text.position.x)?,
                        number(text.position.y)?,
                        number(text.position.z)?,
                        alignment(text.alignment),
                        quoted(&format!(
                            "{},{}",
                            text.font.family,
                            number(text.font.size_points)?
                        ))?,
                        quoted(&self.figure.theme.foreground_color)?
                    )
                    .unwrap();
                }
                Annotation3D::Arrow(arrow) => {
                    validate_point3d(arrow.from, "3-D arrow start")?;
                    validate_point3d(arrow.to, "3-D arrow end")?;
                    let line =
                        line_options(&arrow.line, &self.figure.theme.foreground_color, true)?;
                    writeln!(
                        output,
                        "set arrow {tag} from first {}, first {}, first {} to first {}, first {}, first {} {} {line}",
                        number(arrow.from.x)?,
                        number(arrow.from.y)?,
                        number(arrow.from.z)?,
                        number(arrow.to.x)?,
                        number(arrow.to.y)?,
                        number(arrow.to.z)?,
                        arrow_head(arrow.head)
                    )
                    .unwrap();
                }
            }
        }
        Ok(())
    }

    fn add_data(&mut self, data: String) -> String {
        let name = format!("$REX_DATA_{}", self.data_blocks.len());
        self.data_blocks.push(data);
        name
    }

    fn take_tag(&mut self) -> usize {
        let tag = self.next_tag;
        self.next_tag += 1;
        tag
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AxisDomain {
    Unspecified,
    Numeric,
    Time,
}

#[derive(Clone, Copy, Debug)]
struct Domains2D {
    x1: AxisDomain,
    x2: AxisDomain,
}

fn domains_2d(plot: &Plot2D) -> Result<Domains2D, GnuplotError> {
    let mut domains = Domains2D {
        x1: AxisDomain::Unspecified,
        x2: AxisDomain::Unspecified,
    };
    for series in &plot.series {
        let (binding, domain) = match series {
            Series2D::Curve(curve) => (
                curve.axes,
                match curve.data {
                    XYData::Numeric(_) | XYData::NumericSegments(_) => AxisDomain::Numeric,
                    XYData::Time(_) | XYData::TimeSegments(_) => AxisDomain::Time,
                },
            ),
            Series2D::Error(value) => (value.axes, AxisDomain::Numeric),
            Series2D::Band(value) => (value.axes, AxisDomain::Numeric),
            Series2D::Histogram(value) => (value.axes, AxisDomain::Numeric),
            Series2D::Heatmap(value) => (value.axes, AxisDomain::Numeric),
            Series2D::Vector(value) => (value.axes, AxisDomain::Numeric),
            Series2D::Label(value) => (value.axes, AxisDomain::Numeric),
            Series2D::Bar(_) => (AxisBinding::Primary, AxisDomain::Numeric),
        };
        let target = match binding {
            AxisBinding::Primary | AxisBinding::SecondaryY => &mut domains.x1,
            AxisBinding::SecondaryX | AxisBinding::Secondary => &mut domains.x2,
        };
        merge_domain(target, domain)?;
    }
    if domains.x1 == AxisDomain::Unspecified {
        domains.x1 = AxisDomain::Numeric;
    }
    if domains.x2 == AxisDomain::Unspecified {
        domains.x2 = AxisDomain::Numeric;
    }
    Ok(domains)
}

fn merge_domain(target: &mut AxisDomain, incoming: AxisDomain) -> Result<(), GnuplotError> {
    match (*target, incoming) {
        (AxisDomain::Unspecified, value) => *target = value,
        (current, value) if current == value => {}
        _ => {
            return Err(invalid(
                "numeric and timestamped series cannot share the same x axis",
            ));
        }
    }
    Ok(())
}

fn validate_figure(figure: &Figure) -> Result<(u64, u64), GnuplotError> {
    if figure.panels.is_empty() {
        return Err(invalid("a figure must contain at least one panel cell"));
    }
    if figure.panels.len() > MAX_PANELS {
        return Err(invalid(format!(
            "a figure cannot contain more than {MAX_PANELS} panel cells"
        )));
    }
    if figure.panels.iter().all(Option::is_none) {
        return Err(invalid(
            "a figure must contain at least one non-empty panel",
        ));
    }
    let columns = figure.layout.columns;
    if columns == 0 {
        return Err(invalid("figure layout columns must be greater than zero"));
    }
    let inferred_rows = (figure.panels.len() as u64).div_ceil(columns);
    let rows = figure.layout.rows.unwrap_or(inferred_rows);
    if rows == 0 || rows.saturating_mul(columns) < figure.panels.len() as u64 {
        return Err(invalid("figure layout does not have enough panel cells"));
    }
    if rows.saturating_mul(columns) > MAX_PANELS as u64 {
        return Err(invalid(format!(
            "a figure layout cannot contain more than {MAX_PANELS} cells"
        )));
    }
    for (name, value) in [
        ("horizontal panel spacing", figure.layout.horizontal_spacing),
        ("vertical panel spacing", figure.layout.vertical_spacing),
    ] {
        validate_finite(value, name)?;
        if !(0.0..1.0).contains(&value) {
            return Err(invalid(format!(
                "{name} must be at least zero and less than one"
            )));
        }
    }
    let horizontal_gaps = columns.saturating_sub(1) as f64 * figure.layout.horizontal_spacing;
    let vertical_gaps = rows.saturating_sub(1) as f64 * figure.layout.vertical_spacing;
    if horizontal_gaps >= 0.9 || vertical_gaps >= 0.9 {
        return Err(invalid(
            "figure panel spacing leaves no room inside the layout margins",
        ));
    }
    Ok((rows, columns))
}

fn validate_terminal(terminal: RenderTerminal) -> Result<(), GnuplotError> {
    match terminal {
        RenderTerminal::Png(options) => {
            validate_pixel_dimensions(options.width_px, options.height_px, "PNG")
        }
        RenderTerminal::Svg(options) => {
            validate_pixel_dimensions(options.width_px, options.height_px, "SVG")
        }
        RenderTerminal::Pdf(options) => {
            for (name, value) in [
                ("PDF width", options.width_inches),
                ("PDF height", options.height_inches),
            ] {
                validate_finite(value, name)?;
                if value <= 0.0 || value > 200.0 {
                    return Err(invalid(format!(
                        "{name} must be greater than zero and at most 200 inches"
                    )));
                }
            }
            Ok(())
        }
    }
}

fn validate_pixel_dimensions(width: u64, height: u64, name: &str) -> Result<(), GnuplotError> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(invalid(format!(
            "{name} dimensions must be between 1 and {MAX_DIMENSION} pixels"
        )));
    }
    Ok(())
}

fn validate_theme(theme: &Theme) -> Result<(), GnuplotError> {
    validate_font(&theme.font)?;
    validate_palette(&theme.palette)?;
    if theme.color_cycle.is_empty() {
        return Err(invalid("theme color_cycle must not be empty"));
    }
    for color in theme.color_cycle.iter().chain([
        &theme.background_color,
        &theme.foreground_color,
        &theme.grid_color,
    ]) {
        validate_text(color, "color")?;
    }
    Ok(())
}

fn validate_font(font: &Font) -> Result<(), GnuplotError> {
    validate_text(&font.family, "font family")?;
    validate_finite(font.size_points, "font size")?;
    if font.family.trim().is_empty() || font.size_points <= 0.0 || font.size_points > 512.0 {
        return Err(invalid(
            "font family must be non-empty and size must be between 0 and 512 points",
        ));
    }
    Ok(())
}

fn validate_palette(palette: &Palette) -> Result<(), GnuplotError> {
    if palette.stops.len() < 2 {
        return Err(invalid("a palette must contain at least two stops"));
    }
    let mut previous = None;
    for stop in &palette.stops {
        validate_finite(stop.position, "palette stop position")?;
        validate_text(&stop.color, "palette color")?;
        if !(0.0..=1.0).contains(&stop.position) {
            return Err(invalid(
                "palette stop positions must be between zero and one",
            ));
        }
        if previous.is_some_and(|value| stop.position <= value) {
            return Err(invalid(
                "palette stop positions must be strictly increasing",
            ));
        }
        previous = Some(stop.position);
    }
    Ok(())
}

fn validate_axis(name: &str, axis: &Axis, domain: AxisDomain) -> Result<(), GnuplotError> {
    match axis.scale {
        AxisScale::Linear => {}
        AxisScale::Log(base) => {
            validate_finite(base, "logarithm base")?;
            if base <= 0.0 || base == 1.0 {
                return Err(invalid(format!(
                    "{name} logarithm base must be positive and not equal to one"
                )));
            }
            if domain == AxisDomain::Time {
                return Err(invalid(format!(
                    "timestamped {name} axes cannot use logarithmic scaling"
                )));
            }
        }
    }
    match (&axis.range, domain) {
        (AxisRange::Numeric(_), AxisDomain::Time) => {
            return Err(invalid(format!(
                "timestamped {name} axis requires a TimeRange or AutoRange"
            )));
        }
        (AxisRange::Time(_), AxisDomain::Numeric | AxisDomain::Unspecified) => {
            return Err(invalid(format!(
                "numeric {name} axis cannot use a TimeRange"
            )));
        }
        (AxisRange::Numeric(bounds), _) => validate_bounds(*bounds, "axis range")?,
        (AxisRange::Time(bounds), _) if bounds.minimum >= bounds.maximum => {
            return Err(invalid("time axis minimum must be before its maximum"));
        }
        _ => {}
    }
    match (&axis.tick_format, domain) {
        (TickFormat::Time(_), AxisDomain::Numeric | AxisDomain::Unspecified) => {
            return Err(invalid(format!(
                "numeric {name} axis cannot use a time tick format"
            )));
        }
        (TickFormat::Numeric(_), AxisDomain::Time) => {
            return Err(invalid(format!(
                "timestamped {name} axis cannot use a numeric tick format"
            )));
        }
        _ => {}
    }
    Ok(())
}

fn validate_secondary_axes(plot: &Plot2D) -> Result<(), GnuplotError> {
    for series in &plot.series {
        let binding = match series {
            Series2D::Curve(value) => value.axes,
            Series2D::Error(value) => value.axes,
            Series2D::Band(value) => value.axes,
            Series2D::Histogram(value) => value.axes,
            Series2D::Heatmap(value) => value.axes,
            Series2D::Vector(value) => value.axes,
            Series2D::Label(value) => value.axes,
            Series2D::Bar(_) => AxisBinding::Primary,
        };
        if matches!(binding, AxisBinding::SecondaryX | AxisBinding::Secondary)
            && plot.x2_axis.is_none()
        {
            return Err(invalid("a series uses x2 but x2_axis is absent"));
        }
        if matches!(binding, AxisBinding::SecondaryY | AxisBinding::Secondary)
            && plot.y2_axis.is_none()
        {
            return Err(invalid("a series uses y2 but y2_axis is absent"));
        }
    }
    Ok(())
}

fn validate_view(view: View3D) -> Result<(), GnuplotError> {
    for (name, value) in [
        ("view elevation", view.elevation_degrees),
        ("view azimuth", view.azimuth_degrees),
        ("view scale", view.scale),
    ] {
        validate_finite(value, name)?;
    }
    if view.scale <= 0.0 {
        return Err(invalid("3-D view scale must be greater than zero"));
    }
    Ok(())
}

fn validate_grid(grid: &Grid2D, name: &str) -> Result<(), GnuplotError> {
    if grid.x.is_empty() || grid.y.is_empty() {
        return Err(invalid(format!("{name} grid axes must not be empty")));
    }
    if grid.values.len() != grid.y.len() || grid.values.iter().any(|row| row.len() != grid.x.len())
    {
        return Err(invalid(format!(
            "{name} grid values must have one row per y value and one column per x value"
        )));
    }
    for value in grid.x.iter().chain(&grid.y) {
        validate_finite(*value, "grid coordinate")?;
    }
    ensure_strictly_increasing(&grid.x, "grid x coordinates")?;
    ensure_strictly_increasing(&grid.y, "grid y coordinates")?;
    for value in grid.values.iter().flatten().flatten() {
        validate_finite(*value, "grid value")?;
    }
    Ok(())
}

fn ensure_strictly_increasing(values: &[f64], name: &str) -> Result<(), GnuplotError> {
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(invalid(format!("{name} must be strictly increasing")));
    }
    Ok(())
}

fn serialize_xy_data(data: &XYData) -> Result<String, GnuplotError> {
    let mut output = String::new();
    match data {
        XYData::Numeric(points) => write_numeric_xy(&mut output, points, "curve")?,
        XYData::NumericSegments(segments) => {
            validate_segments(segments, "curve")?;
            for (index, points) in segments.iter().enumerate() {
                if index > 0 {
                    output.push('\n');
                }
                write_numeric_xy(&mut output, points, "curve segment")?;
            }
        }
        XYData::Time(points) => write_time_xy(&mut output, points, "time curve")?,
        XYData::TimeSegments(segments) => {
            validate_segments(segments, "time curve")?;
            for (index, points) in segments.iter().enumerate() {
                if index > 0 {
                    output.push('\n');
                }
                write_time_xy(&mut output, points, "time curve segment")?;
            }
        }
    }
    Ok(output)
}

fn validate_segments<T>(segments: &[Vec<T>], name: &str) -> Result<(), GnuplotError> {
    if segments.is_empty() || segments.iter().any(Vec::is_empty) {
        return Err(invalid(format!(
            "{name} segments must contain at least one non-empty segment"
        )));
    }
    Ok(())
}

fn write_numeric_xy(
    output: &mut String,
    points: &[(f64, f64)],
    name: &str,
) -> Result<(), GnuplotError> {
    if points.is_empty() {
        return Err(invalid(format!("{name} data must not be empty")));
    }
    for (x, y) in points {
        validate_finite(*x, "x coordinate")?;
        validate_finite(*y, "y coordinate")?;
        writeln!(output, "{} {}", number(*x)?, number(*y)?).unwrap();
    }
    Ok(())
}

fn write_time_xy(
    output: &mut String,
    points: &[(DateTime<Utc>, f64)],
    name: &str,
) -> Result<(), GnuplotError> {
    if points.is_empty() {
        return Err(invalid(format!("{name} data must not be empty")));
    }
    for (x, y) in points {
        validate_finite(*y, "y coordinate")?;
        writeln!(output, "{} {}", datetime_number(x), number(*y)?).unwrap();
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum ErrorKind {
    X,
    Y,
    XY,
}

fn serialize_errors(errors: &ErrorBars2D) -> Result<(String, ErrorKind), GnuplotError> {
    if errors.data.is_empty() {
        return Err(invalid(
            "an error-bar series must contain at least one point",
        ));
    }
    let has_x = errors.data[0].x_error.is_some();
    let has_y = errors.data[0].y_error.is_some();
    if !has_x && !has_y {
        return Err(invalid(
            "an error-bar series must provide x_error or y_error",
        ));
    }
    if errors
        .data
        .iter()
        .any(|point| point.x_error.is_some() != has_x || point.y_error.is_some() != has_y)
    {
        return Err(invalid(
            "every point in an error-bar series must provide the same error dimensions",
        ));
    }
    let kind = match (has_x, has_y) {
        (true, false) => ErrorKind::X,
        (false, true) => ErrorKind::Y,
        (true, true) => ErrorKind::XY,
        (false, false) => unreachable!(),
    };
    let mut output = String::new();
    for point in &errors.data {
        validate_finite(point.x, "error point x")?;
        validate_finite(point.y, "error point y")?;
        write!(output, "{} {}", number(point.x)?, number(point.y)?).unwrap();
        if let Some(error) = point.x_error {
            let bounds = error_bounds(point.x, error, "x error")?;
            write!(
                output,
                " {} {}",
                number(bounds.minimum)?,
                number(bounds.maximum)?
            )
            .unwrap();
        }
        if let Some(error) = point.y_error {
            let bounds = error_bounds(point.y, error, "y error")?;
            write!(
                output,
                " {} {}",
                number(bounds.minimum)?,
                number(bounds.maximum)?
            )
            .unwrap();
        }
        output.push('\n');
    }
    Ok((output, kind))
}

fn error_bounds(
    center: f64,
    error: ErrorExtent,
    name: &str,
) -> Result<NumericBounds, GnuplotError> {
    match error {
        ErrorExtent::Symmetric(delta) => {
            validate_finite(delta, name)?;
            if delta < 0.0 {
                return Err(invalid(format!("{name} magnitude cannot be negative")));
            }
            Ok(NumericBounds {
                minimum: center - delta,
                maximum: center + delta,
            })
        }
        ErrorExtent::Absolute(bounds) => {
            validate_bounds(bounds, name)?;
            if center < bounds.minimum || center > bounds.maximum {
                return Err(invalid(format!(
                    "{name} bounds must contain the central value"
                )));
            }
            Ok(bounds)
        }
    }
}

fn error_using(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::X | ErrorKind::Y => "1:2:3:4",
        ErrorKind::XY => "1:2:3:4:5:6",
    }
}

fn serialize_band(data: &BandData) -> Result<String, GnuplotError> {
    let mut output = String::new();
    match data {
        BandData::Points(points) => write_band(&mut output, points, "band")?,
        BandData::Segments(segments) => {
            validate_segments(segments, "band")?;
            for (index, points) in segments.iter().enumerate() {
                if index > 0 {
                    output.push('\n');
                }
                write_band(&mut output, points, "band segment")?;
            }
        }
    }
    Ok(output)
}

fn write_band(output: &mut String, points: &[BandPoint2D], name: &str) -> Result<(), GnuplotError> {
    if points.is_empty() {
        return Err(invalid(format!("{name} data must not be empty")));
    }
    for point in points {
        for value in [point.x, point.lower, point.upper] {
            validate_finite(value, "band value")?;
        }
        if point.lower > point.upper {
            return Err(invalid("band lower value cannot exceed its upper value"));
        }
        writeln!(
            output,
            "{} {} {}",
            number(point.x)?,
            number(point.lower)?,
            number(point.upper)?
        )
        .unwrap();
    }
    Ok(())
}

fn serialize_histogram(histogram: &Histogram) -> Result<String, GnuplotError> {
    if histogram.samples.is_empty() {
        return Err(invalid("a histogram must contain at least one sample"));
    }
    for sample in &histogram.samples {
        validate_finite(*sample, "histogram sample")?;
    }
    let bounds = match histogram.range {
        Some(bounds) => {
            validate_bounds(bounds, "histogram range")?;
            bounds
        }
        None => {
            let minimum = histogram
                .samples
                .iter()
                .copied()
                .fold(f64::INFINITY, f64::min);
            let maximum = histogram
                .samples
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max);
            if minimum == maximum {
                NumericBounds {
                    minimum: minimum - 0.5,
                    maximum: maximum + 0.5,
                }
            } else {
                NumericBounds { minimum, maximum }
            }
        }
    };
    let (count, width) = match histogram.bins {
        HistogramBins::Count(count) if count > 0 => (
            count as usize,
            (bounds.maximum - bounds.minimum) / count as f64,
        ),
        HistogramBins::Count(_) => return Err(invalid("histogram bin count must be positive")),
        HistogramBins::Width(width) => {
            validate_finite(width, "histogram bin width")?;
            if width <= 0.0 {
                return Err(invalid("histogram bin width must be positive"));
            }
            (
                ((bounds.maximum - bounds.minimum) / width).ceil() as usize,
                width,
            )
        }
    };
    if count == 0 || count > 1_000_000 {
        return Err(invalid(
            "histogram configuration must produce between 1 and 1000000 bins",
        ));
    }
    let mut bins = vec![0_u64; count];
    let mut included = 0_u64;
    for sample in &histogram.samples {
        if *sample < bounds.minimum || *sample > bounds.maximum {
            continue;
        }
        let raw = ((*sample - bounds.minimum) / width).floor() as usize;
        let index = raw.min(count - 1);
        bins[index] += 1;
        included += 1;
    }
    if included == 0 {
        return Err(invalid("histogram range excludes every sample"));
    }
    let mut output = String::new();
    for (index, count_value) in bins.into_iter().enumerate() {
        let center = bounds.minimum + (index as f64 + 0.5) * width;
        let value = match histogram.normalization {
            HistogramNormalization::Counts => count_value as f64,
            HistogramNormalization::Probability => count_value as f64 / included as f64,
            HistogramNormalization::Density => count_value as f64 / (included as f64 * width),
        };
        writeln!(
            output,
            "{} {} {}",
            number(center)?,
            number(value)?,
            number(width * 0.9)?
        )
        .unwrap();
    }
    Ok(output)
}

fn serialize_grid(grid: &Grid2D) -> Result<String, GnuplotError> {
    let mut output = String::new();
    for (row, y) in grid.y.iter().enumerate() {
        for (column, x) in grid.x.iter().enumerate() {
            let value = grid.values[row][column]
                .map(number)
                .transpose()?
                .unwrap_or_else(|| "NaN".to_owned());
            writeln!(output, "{} {} {value}", number(*x)?, number(*y)?).unwrap();
        }
        output.push('\n');
    }
    Ok(output)
}

fn serialize_points3d(points: &[Point3D], name: &str) -> Result<String, GnuplotError> {
    if points.is_empty() {
        return Err(invalid(format!("a {name} must contain at least one point")));
    }
    let mut output = String::new();
    for point in points {
        validate_point3d(*point, name)?;
        writeln!(
            output,
            "{} {} {}",
            number(point.x)?,
            number(point.y)?,
            number(point.z)?
        )
        .unwrap();
    }
    Ok(output)
}

fn serialize_path3d(data: &PathData3D) -> Result<String, GnuplotError> {
    let mut output = String::new();
    match data {
        PathData3D::Points(points) => output.push_str(&serialize_points3d(points, "3-D path")?),
        PathData3D::Segments(segments) => {
            validate_segments(segments, "3-D path")?;
            for (index, points) in segments.iter().enumerate() {
                if index > 0 {
                    output.push('\n');
                }
                output.push_str(&serialize_points3d(points, "3-D path segment")?);
            }
        }
    }
    Ok(output)
}

fn validate_point3d(point: Point3D, name: &str) -> Result<(), GnuplotError> {
    for value in [point.x, point.y, point.z] {
        validate_finite(value, name)?;
    }
    Ok(())
}

fn write_grid(output: &mut String, grid: &Grid, theme: &Theme) -> Result<(), GnuplotError> {
    if grid.minor && grid.x {
        output.push_str("set mxtics 2\n");
    } else {
        output.push_str("unset mxtics\n");
    }
    if grid.minor && grid.y {
        output.push_str("set mytics 2\n");
    } else {
        output.push_str("unset mytics\n");
    }
    if !grid.x && !grid.y {
        output.push_str("unset grid\n");
        return Ok(());
    }
    output.push_str("set grid");
    if grid.x {
        output.push_str(" xtics");
    }
    if grid.y {
        output.push_str(" ytics");
    }
    if grid.minor {
        if grid.x {
            output.push_str(" mxtics");
        }
        if grid.y {
            output.push_str(" mytics");
        }
    }
    writeln!(output, " lc rgb {} dt 3", quoted(&theme.grid_color)?).unwrap();
    Ok(())
}

fn write_legend(
    output: &mut String,
    legend: &Legend,
    text_color: &str,
) -> Result<(), GnuplotError> {
    if !legend.visible {
        output.push_str("unset key\n");
        return Ok(());
    }
    output.push_str("set key");
    match legend.position {
        LegendPosition::TopLeft => output.push_str(" inside left top"),
        LegendPosition::TopRight => output.push_str(" inside right top"),
        LegendPosition::BottomLeft => output.push_str(" inside left bottom"),
        LegendPosition::BottomRight => output.push_str(" inside right bottom"),
        LegendPosition::OutsideRight => output.push_str(" outside right center"),
        LegendPosition::Below => output.push_str(" outside center bottom"),
    }
    if legend.horizontal {
        output.push_str(" horizontal");
    } else {
        output.push_str(" vertical");
    }
    if legend.reversed {
        output.push_str(" reverse");
    } else {
        output.push_str(" noreverse");
    }
    writeln!(output, " textcolor rgb {}", quoted(text_color)?).unwrap();
    Ok(())
}

fn write_palette(output: &mut String, palette: &Palette) -> Result<(), GnuplotError> {
    output.push_str("set palette defined (");
    for index in 0..palette.stops.len() {
        if index > 0 {
            output.push_str(", ");
        }
        let position = palette.stops[index].position;
        let color = if palette.reversed {
            &palette.stops[palette.stops.len() - 1 - index].color
        } else {
            &palette.stops[index].color
        };
        write!(output, "{} {}", number(position)?, quoted(color)?).unwrap();
    }
    output.push_str(")\n");
    Ok(())
}

fn range_text(range: &AxisRange) -> Result<String, GnuplotError> {
    match range {
        AxisRange::Auto => Ok("[*:*]".to_owned()),
        AxisRange::Numeric(bounds) => Ok(format!(
            "[{}:{}]",
            number(bounds.minimum)?,
            number(bounds.maximum)?
        )),
        AxisRange::Time(bounds) => Ok(format!(
            "[{}:{}]",
            datetime_number(&bounds.minimum),
            datetime_number(&bounds.maximum)
        )),
    }
}

fn write_optional_title(
    output: &mut String,
    title: Option<&str>,
    text_color: &str,
) -> Result<(), GnuplotError> {
    match title {
        Some(title) => writeln!(
            output,
            "set title {} textcolor rgb {} noenhanced",
            quoted(title)?,
            quoted(text_color)?
        )
        .unwrap(),
        None => output.push_str("unset title\n"),
    }
    Ok(())
}

fn title_clause(title: Option<&str>) -> Result<String, GnuplotError> {
    match title {
        Some(title) => Ok(format!("title {}", quoted(title)?)),
        None => Ok("notitle".to_owned()),
    }
}

fn axes_clause(binding: AxisBinding) -> &'static str {
    match binding {
        AxisBinding::Primary => "axes x1y1",
        AxisBinding::SecondaryX => "axes x2y1",
        AxisBinding::SecondaryY => "axes x1y2",
        AxisBinding::Secondary => "axes x2y2",
    }
}

fn curve_style(curve: &Curve2D, fallback: &str) -> Result<String, GnuplotError> {
    let line = line_options(&curve.line, fallback, true)?;
    let points = point_options(&curve.points, fallback, false)?;
    Ok(match curve.mode {
        CurveMode::Lines => format!("with lines {line}"),
        CurveMode::Points => format!(
            "with points {}",
            point_options(&curve.points, fallback, true)?
        ),
        CurveMode::LinesPoints => format!("with linespoints {line} {points}"),
        CurveMode::StepsBefore => format!("with fsteps {line}"),
        CurveMode::StepsCentered => format!("with histeps {line}"),
        CurveMode::StepsAfter => format!("with steps {line}"),
        CurveMode::Impulses => format!("with impulses {line}"),
    })
}

fn line_options(
    style: &LineStyle,
    fallback: &str,
    include_color: bool,
) -> Result<String, GnuplotError> {
    validate_finite(style.width, "line width")?;
    if style.width <= 0.0 {
        return Err(invalid("line width must be greater than zero"));
    }
    let mut output = String::new();
    if include_color {
        write!(
            output,
            "lc rgb {} ",
            quoted(style.color.as_deref().unwrap_or(fallback))?
        )
        .unwrap();
    }
    write!(
        output,
        "lw {} dt {}",
        number(style.width)?,
        dash_type(style.dash)
    )
    .unwrap();
    Ok(output)
}

fn point_options(
    style: &PointStyle,
    fallback: &str,
    include_color: bool,
) -> Result<String, GnuplotError> {
    validate_finite(style.size, "point size")?;
    if style.size <= 0.0 {
        return Err(invalid("point size must be greater than zero"));
    }
    let mut output = String::new();
    if include_color {
        write!(
            output,
            "lc rgb {} ",
            quoted(style.color.as_deref().unwrap_or(fallback))?
        )
        .unwrap();
    }
    write!(
        output,
        "pt {} ps {}",
        point_type(style.shape),
        number(style.size)?
    )
    .unwrap();
    Ok(output)
}

fn fill_options(style: &FillStyle, fallback: &str) -> Result<String, GnuplotError> {
    validate_finite(style.opacity, "fill opacity")?;
    if !(0.0..=1.0).contains(&style.opacity) {
        return Err(invalid("fill opacity must be between zero and one"));
    }
    let color = quoted(style.color.as_deref().unwrap_or(fallback))?;
    let mode = match style.mode {
        FillMode::Solid => format!("fs transparent solid {}", number(style.opacity)?),
        FillMode::Pattern(pattern) if pattern >= 0 => {
            format!("fs transparent pattern {pattern}")
        }
        FillMode::Pattern(_) => return Err(invalid("fill pattern cannot be negative")),
    };
    Ok(format!(
        "fc rgb {color} {mode} {}",
        if style.border { "border" } else { "noborder" }
    ))
}

fn dash_type(dash: DashPattern) -> i32 {
    match dash {
        DashPattern::Solid => 1,
        DashPattern::Dashed => 2,
        DashPattern::Dotted => 3,
        DashPattern::DashDot => 4,
    }
}

fn point_type(shape: PointShape) -> i32 {
    match shape {
        PointShape::Plus => 1,
        PointShape::Cross => 2,
        PointShape::Star => 3,
        PointShape::Square => 4,
        PointShape::FilledSquare => 5,
        PointShape::Circle => 6,
        PointShape::FilledCircle => 7,
        PointShape::Triangle => 8,
        PointShape::FilledTriangle => 9,
        PointShape::Diamond => 12,
        PointShape::FilledDiamond => 13,
    }
}

fn arrow_head(head: ArrowHead) -> &'static str {
    match head {
        ArrowHead::None => "nohead",
        ArrowHead::Open => "head empty",
        ArrowHead::Filled => "head filled",
    }
}

fn alignment(alignment: TextAlignment) -> &'static str {
    match alignment {
        TextAlignment::Left => "left",
        TextAlignment::Center => "center",
        TextAlignment::Right => "right",
    }
}

fn position2d(position: Position2D) -> Result<String, GnuplotError> {
    match position {
        Position2D::Data(x, y) => {
            validate_finite(x, "annotation x")?;
            validate_finite(y, "annotation y")?;
            Ok(format!("first {}, first {}", number(x)?, number(y)?))
        }
        Position2D::Panel(x, y) => {
            validate_finite(x, "panel-relative annotation x")?;
            validate_finite(y, "panel-relative annotation y")?;
            if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
                return Err(invalid(
                    "panel-relative annotation coordinates must be between zero and one",
                ));
            }
            Ok(format!("graph {}, graph {}", number(x)?, number(y)?))
        }
    }
}

fn panel_reset() -> String {
    concat!(
        "unset label\n",
        "unset arrow\n",
        "unset object\n",
        "unset key\n",
        "unset grid\n",
        "unset colorbox\n",
        "unset contour\n",
        "set surface\n",
        "unset logscale\n",
        "unset polar\n",
        "set autoscale\n",
        "unset x2tics\n",
        "unset y2tics\n",
        "unset x2label\n",
        "unset y2label\n",
        "set xdata\n",
        "set ydata\n",
        "set x2data\n",
        "set y2data\n",
        "set size noratio\n",
        "set view 60,30,1\n",
    )
    .to_owned()
}

fn cycle_color(theme: &Theme, index: usize) -> Result<&str, GnuplotError> {
    theme
        .color_cycle
        .get(index % theme.color_cycle.len())
        .map(String::as_str)
        .ok_or_else(|| invalid("theme color_cycle must not be empty"))
}

fn validate_bounds(bounds: NumericBounds, name: &str) -> Result<(), GnuplotError> {
    validate_finite(bounds.minimum, name)?;
    validate_finite(bounds.maximum, name)?;
    if bounds.minimum >= bounds.maximum {
        return Err(invalid(format!(
            "{name} minimum must be less than its maximum"
        )));
    }
    Ok(())
}

fn validate_finite(value: f64, name: &str) -> Result<(), GnuplotError> {
    if !value.is_finite() {
        return Err(invalid(format!("{name} must be finite")));
    }
    Ok(())
}

fn number(value: f64) -> Result<String, GnuplotError> {
    validate_finite(value, "numeric value")?;
    Ok(format!("{value:.17}"))
}

fn datetime_number(value: &DateTime<Utc>) -> String {
    let seconds = value.timestamp() as f64 + f64::from(value.timestamp_subsec_nanos()) / 1e9;
    format!("{seconds:.9}")
}

fn quoted(value: &str) -> Result<String, GnuplotError> {
    validate_text(value, "text")?;
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

fn data_string(value: &str) -> Result<String, GnuplotError> {
    quoted(value)
}

fn validate_text(value: &str, name: &str) -> Result<(), GnuplotError> {
    if value.chars().any(|character| {
        character == '\n' || character == '\r' || (character.is_control() && character != '\t')
    }) {
        return Err(invalid(format!(
            "{name} cannot contain line breaks or control characters"
        )));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> GnuplotError {
    GnuplotError {
        kind: GnuplotErrorKind::InvalidFigure,
        exit_code: None,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve_figure() -> Figure {
        Figure {
            panels: vec![Some(Panel::TwoDimensional(Plot2D {
                series: vec![Series2D::Curve(Curve2D {
                    data: XYData::Numeric(vec![(0.0, 1.0), (1.0, 2.0)]),
                    title: Some("observed".to_owned()),
                    ..Curve2D::default()
                })],
                ..Plot2D::default()
            }))],
            ..Figure::default()
        }
    }

    #[test]
    fn serializes_semantic_curve_without_external_paths() {
        let source =
            serialize_figure(&curve_figure(), RenderTerminal::Svg(SvgOptions::default())).unwrap();
        assert!(source.contains("$REX_DATA_0 << EOD"));
        assert!(source.contains("with lines"));
        assert!(source.contains("set output output_file"));
        assert!(!source.contains("system "));
        assert!(!source.contains("load "));
    }

    #[test]
    fn render_plan_reads_the_program_from_a_declared_input() {
        let source = blake3::hash(b"gnuplot program");
        let plan = render_plan(source, RenderTerminal::Svg(SvgOptions::default()));

        assert_eq!(plan.inputs.len(), 1);
        assert_eq!(plan.inputs[0].hash, source);
        assert_eq!(plan.inputs[0].extension, "gp");
        assert_eq!(plan.inputs[0].kind, InputKind::Blob);
        assert_eq!(plan.arguments.last(), Some(&ToolArgument::input(0)));
    }

    #[test]
    fn rejects_mixed_numeric_and_timestamped_x_data() {
        let timestamp = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let figure = Figure {
            panels: vec![Some(Panel::TwoDimensional(Plot2D {
                series: vec![
                    Series2D::Curve(Curve2D {
                        data: XYData::Numeric(vec![(0.0, 1.0)]),
                        ..Curve2D::default()
                    }),
                    Series2D::Curve(Curve2D {
                        data: XYData::Time(vec![(timestamp, 2.0)]),
                        ..Curve2D::default()
                    }),
                ],
                ..Plot2D::default()
            }))],
            ..Figure::default()
        };
        assert!(serialize_figure(&figure, RenderTerminal::Svg(SvgOptions::default())).is_err());
    }

    #[test]
    fn rejects_non_rectangular_heatmap() {
        let figure = Figure {
            panels: vec![Some(Panel::TwoDimensional(Plot2D {
                series: vec![Series2D::Heatmap(Heatmap2D {
                    grid: Grid2D {
                        x: vec![0.0, 1.0],
                        y: vec![0.0],
                        values: vec![vec![Some(1.0)]],
                    },
                    ..Heatmap2D::default()
                })],
                ..Plot2D::default()
            }))],
            ..Figure::default()
        };
        assert!(serialize_figure(&figure, RenderTerminal::Svg(SvgOptions::default())).is_err());
    }

    #[test]
    fn histogram_binning_is_deterministic() {
        let histogram = Histogram {
            samples: vec![0.0, 0.2, 0.8, 1.0],
            bins: HistogramBins::Count(2),
            range: Some(NumericBounds {
                minimum: 0.0,
                maximum: 1.0,
            }),
            ..Histogram::default()
        };
        let first = serialize_histogram(&histogram).unwrap();
        let second = serialize_histogram(&histogram).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.lines().count(), 2);
    }

    #[test]
    fn timestamp_data_selects_epoch_time_mode() {
        let timestamp = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let figure = Figure {
            panels: vec![Some(Panel::TwoDimensional(Plot2D {
                x_axis: Axis {
                    tick_format: TickFormat::Time("%Y-%m-%d".to_owned()),
                    ..Axis::default()
                },
                series: vec![Series2D::Curve(Curve2D {
                    data: XYData::Time(vec![(timestamp, 1.0)]),
                    ..Curve2D::default()
                })],
                ..Plot2D::default()
            }))],
            ..Figure::default()
        };
        let source = serialize_figure(&figure, RenderTerminal::Svg(SvgOptions::default())).unwrap();
        assert!(source.contains("set timefmt \"%s\""));
        assert!(source.contains("set xdata time"));
        assert!(source.contains("set format x \"%Y-%m-%d\""));
    }

    #[test]
    fn every_two_dimensional_series_kind_serializes() {
        let grid = Grid2D {
            x: vec![0.0, 1.0],
            y: vec![0.0, 1.0],
            values: vec![vec![Some(0.0), Some(1.0)], vec![Some(1.0), Some(2.0)]],
        };
        let panels = vec![
            Series2D::Curve(Curve2D {
                data: XYData::Numeric(vec![(0.0, 0.0), (1.0, 1.0)]),
                ..Curve2D::default()
            }),
            Series2D::Error(ErrorBars2D {
                data: vec![ErrorPoint2D {
                    x: 1.0,
                    y: 2.0,
                    x_error: None,
                    y_error: Some(ErrorExtent::Symmetric(0.2)),
                }],
                ..ErrorBars2D::default()
            }),
            Series2D::Band(Band2D {
                data: BandData::Points(vec![BandPoint2D {
                    x: 0.0,
                    lower: 0.5,
                    upper: 1.5,
                }]),
                ..Band2D::default()
            }),
            Series2D::Bar(BarChart {
                series: vec![BarSeries {
                    values: vec![("A".to_owned(), 1.0)],
                    ..BarSeries::default()
                }],
                ..BarChart::default()
            }),
            Series2D::Histogram(Histogram {
                samples: vec![0.0, 0.5, 1.0],
                ..Histogram::default()
            }),
            Series2D::Heatmap(Heatmap2D {
                grid,
                ..Heatmap2D::default()
            }),
            Series2D::Vector(Vectors2D {
                data: vec![Vector2D {
                    x: 0.0,
                    y: 0.0,
                    dx: 1.0,
                    dy: 1.0,
                }],
                ..Vectors2D::default()
            }),
            Series2D::Label(Labels2D {
                data: vec![LabelPoint2D {
                    x: 0.0,
                    y: 0.0,
                    text: "origin".to_owned(),
                }],
                ..Labels2D::default()
            }),
        ]
        .into_iter()
        .map(|series| {
            Some(Panel::TwoDimensional(Plot2D {
                series: vec![series],
                ..Plot2D::default()
            }))
        })
        .collect();
        let figure = Figure {
            layout: GridLayout {
                columns: 2,
                ..GridLayout::default()
            },
            panels,
            ..Figure::default()
        };
        let source = serialize_figure(&figure, RenderTerminal::Svg(SvgOptions::default())).unwrap();
        for style in [
            "with lines",
            "with yerrorbars",
            "with filledcurves",
            "with histograms",
            "with boxes",
            "with image",
            "with vectors",
            "with labels",
        ] {
            assert!(source.contains(style), "missing serialized style {style}");
        }
    }

    #[test]
    fn every_three_dimensional_series_kind_serializes() {
        let grid = || Grid2D {
            x: vec![0.0, 1.0],
            y: vec![0.0, 1.0],
            values: vec![vec![Some(0.0), Some(1.0)], vec![Some(1.0), Some(2.0)]],
        };
        let point = Point3D {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let series = vec![
            Series3D::Point(PointCloud3D {
                data: vec![point],
                ..PointCloud3D::default()
            }),
            Series3D::Path(Path3D {
                data: PathData3D::Points(vec![point]),
                ..Path3D::default()
            }),
            Series3D::Surface(Surface3D {
                grid: grid(),
                mode: SurfaceMode::Wireframe,
                ..Surface3D::default()
            }),
            Series3D::Surface(Surface3D {
                grid: grid(),
                mode: SurfaceMode::Colored,
                ..Surface3D::default()
            }),
            Series3D::Surface(Surface3D {
                grid: grid(),
                mode: SurfaceMode::ContourLines,
                ..Surface3D::default()
            }),
            Series3D::Surface(Surface3D {
                grid: grid(),
                mode: SurfaceMode::FilledContours,
                ..Surface3D::default()
            }),
        ];
        let panels = series
            .into_iter()
            .map(|series| {
                Some(Panel::ThreeDimensional(Plot3D {
                    series: vec![series],
                    ..Plot3D::default()
                }))
            })
            .collect();
        let figure = Figure {
            layout: GridLayout {
                columns: 2,
                ..GridLayout::default()
            },
            panels,
            ..Figure::default()
        };
        let source = serialize_figure(&figure, RenderTerminal::Svg(SvgOptions::default())).unwrap();
        for style in [
            "with points",
            "with lines",
            "with pm3d",
            "set contour base",
            "with contourfill at base",
        ] {
            assert!(source.contains(style), "missing serialized style {style}");
        }
    }
}
