#[path = "compile.rs"]
mod compile;
#[path = "types.rs"]
pub mod types;

use crate::{modules::tools::executor::ToolExecution, state::State};
use compile::*;
use rex::engine::{EngineError, Module};
use types::*;

type PlotResult<T> = Result<T, GnuplotError>;

pub fn module() -> Result<Module<State>, EngineError> {
    api::rex_module()
}

/// Semantic, headless plotting of inline Rex data with gnuplot.
///
/// Workflows construct immutable figures from typed panels, axes, series, annotations, and styles.
/// Raw gnuplot commands, host paths, mutable sessions, and file-backed table inputs are not exposed.
/// The host validates the completed model, compiles it to a private gnuplot program, executes it in
/// the configured tool runtime, and returns a shared content-addressed image or PDF artifact.
#[rex::module(
    name = "tools.gnuplot",
    defaults(
        Figure,
        GridLayout,
        Theme,
        Font,
        Palette,
        Axis,
        Grid,
        Legend,
        LineStyle,
        PointStyle,
        FillStyle,
        Curve2D,
        ErrorBars2D,
        Band2D,
        BarSeries,
        BarChart,
        Histogram,
        Grid2D,
        Heatmap2D,
        Vectors2D,
        Labels2D,
        Plot2D,
        PointCloud3D,
        Path3D,
        Surface3D,
        View3D,
        Plot3D,
        PngOptions,
        SvgOptions,
        PdfOptions,
    )
)]
mod api {
    use super::*;

    /// Render a semantic figure as a PNG image stored in the workflow CAS.
    ///
    /// All datasets are supplied inline through the figure's series. Invalid dimensions, grids,
    /// axes, errors, styles, or incompatible series return `Err GnuplotError` without starting
    /// gnuplot. The packaged runtime uses the headless `pngcairo` terminal.
    #[rex::export]
    pub(super) async fn render_png(
        state: State,
        figure: Figure,
        options: PngOptions,
    ) -> Result<PlotResult<Image>, EngineError> {
        let terminal = RenderTerminal::Png(options);
        let execution = match render(&state, &figure, terminal).await? {
            Ok(execution) => execution,
            Err(error) => return Ok(Err(error)),
        };
        single_content(&execution)
            .map_or_else(|error| Ok(Err(error)), |content| Ok(Ok(Image { content })))
    }

    /// Render a semantic figure as an SVG image stored in the workflow CAS.
    ///
    /// SVG dimensions are CSS pixels. Text and user data are safely quoted by the private
    /// serializer; no Rex value is interpreted as a gnuplot command.
    #[rex::export]
    pub(super) async fn render_svg(
        state: State,
        figure: Figure,
        options: SvgOptions,
    ) -> Result<PlotResult<Image>, EngineError> {
        let terminal = RenderTerminal::Svg(options);
        let execution = match render(&state, &figure, terminal).await? {
            Ok(execution) => execution,
            Err(error) => return Ok(Err(error)),
        };
        single_content(&execution)
            .map_or_else(|error| Ok(Err(error)), |content| Ok(Ok(Image { content })))
    }

    /// Render a semantic figure as a PDF document stored in the workflow CAS.
    ///
    /// PDF dimensions are specified in inches and rendered with the headless `pdfcairo` terminal.
    #[rex::export]
    pub(super) async fn render_pdf(
        state: State,
        figure: Figure,
        options: PdfOptions,
    ) -> Result<PlotResult<Pdf>, EngineError> {
        let terminal = RenderTerminal::Pdf(options);
        let execution = match render(&state, &figure, terminal).await? {
            Ok(execution) => execution,
            Err(error) => return Ok(Err(error)),
        };
        single_content(&execution)
            .map_or_else(|error| Ok(Err(error)), |content| Ok(Ok(Pdf { content })))
    }

    /// Return the installed version reported by `gnuplot --version`.
    #[rex::export]
    pub(super) async fn version(state: State) -> Result<PlotResult<VersionInfo>, EngineError> {
        let execution = execute(&state, version_plan()).await?;
        if execution.exit_code != Some(0) {
            return Ok(Err(process_error(&execution)));
        }
        let diagnostics = diagnostics(&execution);
        let first = diagnostics
            .lines()
            .next()
            .ok_or_else(|| EngineError::Custom("gnuplot --version returned no output".into()))?;
        let version = first
            .strip_prefix("gnuplot ")
            .unwrap_or(first)
            .trim()
            .to_owned();
        Ok(Ok(VersionInfo { version }))
    }
}

async fn render(
    state: &State,
    figure: &Figure,
    terminal: RenderTerminal,
) -> Result<Result<ToolExecution, GnuplotError>, EngineError> {
    let source = match serialize_figure(figure, terminal) {
        Ok(source) => source,
        Err(error) => return Ok(Err(error)),
    };
    let source = state.store.put(source.as_bytes()).await.map_err(|error| {
        EngineError::Custom(format!("store generated gnuplot program: {error}"))
    })?;
    let execution = execute(state, render_plan(source, terminal)).await?;
    if execution.exit_code != Some(0) {
        return Ok(Err(process_error(&execution)));
    }
    Ok(Ok(execution))
}

async fn execute(
    state: &State,
    plan: crate::modules::tools::executor::ToolExecutionPlan,
) -> Result<ToolExecution, EngineError> {
    state
        .execute_tool(plan)
        .await
        .map_err(|error| EngineError::Custom(error.to_string()))
}

fn single_content(execution: &ToolExecution) -> Result<blake3::Hash, GnuplotError> {
    match execution.outputs.get(&0).map(Vec::as_slice) {
        Some([content]) => Ok(*content),
        Some(values) => Err(unexpected(format!(
            "gnuplot produced {} files instead of one",
            values.len()
        ))),
        None => Err(unexpected("gnuplot did not declare its output")),
    }
}

fn diagnostics(execution: &ToolExecution) -> String {
    let mut output = String::from_utf8_lossy(&execution.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&execution.stdout);
    if !stdout.trim().is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&stdout);
    }
    output.trim().to_owned()
}

fn process_error(execution: &ToolExecution) -> GnuplotError {
    GnuplotError {
        kind: GnuplotErrorKind::ProcessFailed,
        exit_code: execution.exit_code.map(i64::from),
        message: diagnostics(execution),
    }
}

fn unexpected(message: impl Into<String>) -> GnuplotError {
    GnuplotError {
        kind: GnuplotErrorKind::UnexpectedOutput,
        exit_code: None,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::api::*;
    use super::*;
    use rex::storage::Store;

    fn simple_figure() -> Figure {
        Figure {
            panels: vec![Some(Panel::Panel2D(Plot2D {
                series: vec![Series2D::CurveSeries(Curve2D {
                    data: XYData::NumericXY(vec![(0.0, 0.0), (1.0, 1.0)]),
                    ..Curve2D::default()
                })],
                ..Plot2D::default()
            }))],
            ..Figure::default()
        }
    }

    #[tokio::test]
    async fn docker_gnuplot_renders_svg_when_enabled() {
        if std::env::var("REX_WORKFLOW_DOCKER_TESTS").as_deref() != Ok("1") {
            return;
        }
        let store = Store::new_in_memory();
        let state = crate::development_state(store.clone());
        let output = render_svg(state, simple_figure(), SvgOptions::default())
            .await
            .unwrap()
            .unwrap();
        let bytes = store.get(output.content).await.unwrap();
        let svg = String::from_utf8(bytes).unwrap();
        assert!(svg.contains("<svg"));
    }
}
