use blake3::Hash;
use rex::storage::Store;
use rex_workflow::{
    modules::tools::executor::{
        CasInput, DockerToolExecutor, ExpectedOutput, InputKind, OciPlatform, OciToolImages,
        OutputKind, ToolArgument, ToolExecutionPlan, ToolExecutor, ToolProgram,
    },
    run::eval_rex,
    state::State,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    env,
    process::{Command, Stdio},
    str::FromStr,
    time::Duration,
};

const ENABLE_ENV: &str = "REX_WORKFLOW_DOCKER_TESTS";
const FFMPEG_IMAGE_ENV: &str = "REX_WORKFLOW_FFMPEG_IMAGE";
const GNUPLOT_IMAGE_ENV: &str = "REX_WORKFLOW_GNUPLOT_IMAGE";
const GRAPHVIZ_IMAGE_ENV: &str = "REX_WORKFLOW_GRAPHVIZ_IMAGE";
const IMAGEMAGICK_IMAGE_ENV: &str = "REX_WORKFLOW_IMAGEMAGICK_IMAGE";
const QPDF_IMAGE_ENV: &str = "REX_WORKFLOW_QPDF_IMAGE";
const POPPLER_IMAGE_ENV: &str = "REX_WORKFLOW_POPPLER_IMAGE";

struct DockerFixture {
    store: Store,
    state: State,
}

fn docker_fixture() -> Option<DockerFixture> {
    if !docker_tests_enabled() {
        eprintln!("skipping Docker integration test; set {ENABLE_ENV}=1 to enable the suite");
        return None;
    }

    let store = Store::new_in_memory();
    let images = OciToolImages::development(
        OciPlatform::native_linux(),
        image_reference(FFMPEG_IMAGE_ENV, "rex-tool-ffmpeg:local"),
        image_reference(GNUPLOT_IMAGE_ENV, "rex-tool-gnuplot:local"),
        image_reference(GRAPHVIZ_IMAGE_ENV, "rex-tool-graphviz:local"),
        image_reference(IMAGEMAGICK_IMAGE_ENV, "rex-tool-imagemagick:local"),
        image_reference(QPDF_IMAGE_ENV, "rex-tool-qpdf:local"),
        image_reference(POPPLER_IMAGE_ENV, "rex-tool-poppler:local"),
    );
    let state = State::docker(store.clone(), images);
    Some(DockerFixture { store, state })
}

fn docker_tests_enabled() -> bool {
    match env::var(ENABLE_ENV) {
        Err(env::VarError::NotPresent) => false,
        Err(env::VarError::NotUnicode(_)) => {
            panic!("{ENABLE_ENV} must contain Unicode text")
        }
        Ok(value) => parse_enabled_value(&value).unwrap_or_else(|| {
            panic!(
                "invalid {ENABLE_ENV} value {value:?}; use 1/true/on/yes to enable or 0/false/off/no to disable"
            )
        }),
    }
}

fn parse_enabled_value(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "0" | "false" | "off" | "no" => Some(false),
        "1" | "true" | "on" | "yes" => Some(true),
        _ => None,
    }
}

fn image_reference(variable: &str, default: &str) -> String {
    match env::var(variable) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => default.to_owned(),
        Err(env::VarError::NotUnicode(_)) => panic!("{variable} must contain Unicode text"),
    }
}

fn ok_value(value: &Value) -> &Value {
    value
        .as_object()
        .and_then(|object| object.get("Ok"))
        .unwrap_or_else(|| panic!("expected an Ok result, got {value}"))
}

fn content_hash(value: &Value) -> Hash {
    match value {
        Value::Object(object) => {
            if let Some(content) = object.get("content").and_then(Value::as_str) {
                return Hash::from_str(content)
                    .unwrap_or_else(|error| panic!("invalid content hash {content:?}: {error}"));
            }
            object
                .values()
                .find_map(find_content_hash)
                .unwrap_or_else(|| panic!("result contains no content hash: {value}"))
        }
        _ => panic!("expected an object containing a content hash, got {value}"),
    }
}

fn find_content_hash(value: &Value) -> Option<Hash> {
    match value {
        Value::Object(object) => {
            if let Some(content) = object.get("content").and_then(Value::as_str) {
                return Hash::from_str(content).ok();
            }
            object.values().find_map(find_content_hash)
        }
        Value::Array(values) => values.iter().find_map(find_content_hash),
        _ => None,
    }
}

#[test]
fn docker_test_toggle_values_are_explicit() {
    for value in ["", "0", "false", "FALSE", " off ", "no"] {
        assert_eq!(parse_enabled_value(value), Some(false));
    }
    for value in ["1", "true", "TRUE", " on ", "yes"] {
        assert_eq!(parse_enabled_value(value), Some(true));
    }
    for value in ["enabled", "disabled", "2", "sometimes"] {
        assert_eq!(parse_enabled_value(value), None);
    }
}

#[tokio::test]
async fn docker_reports_all_tool_versions() {
    let Some(fixture) = docker_fixture() else {
        return;
    };
    let source = r#"
        import tools.ffmpeg as FF;
        import tools.gnuplot as GP;
        import tools.graphviz as G;
        import tools.imagemagick as IM;
        import tools.qpdf as Q;
        import tools.poppler as P;

        (FF.version, GP.version, G.version, IM.version, Q.version, P.version)
    "#;

    let result = eval_rex(source, None, fixture.state)
        .await
        .expect("run tool version workflow in Docker");
    let versions = result
        .as_array()
        .unwrap_or_else(|| panic!("expected a tuple of tool versions, got {result}"));

    assert_eq!(versions.len(), 6);
    for version in versions {
        let version = ok_value(version);
        assert!(
            version.get("version").and_then(Value::as_str).is_some(),
            "tool returned no parsed version: {version}"
        );
    }
}

#[tokio::test]
async fn docker_gnuplot_renders_inline_data_as_svg() {
    let Some(fixture) = docker_fixture() else {
        return;
    };
    let source = r##"
        import tools.gnuplot as G;

        G.render_svg
            (G.Figure {
                panels = [
                    Some (G.Panel2D (G.Plot2D {
                        title = Some "Docker smoke test",
                        series = [
                            G.CurveSeries (G.Curve2D {
                                data = G.NumericXY [
                                    (0.0, 0.0),
                                    (1.0, 1.0),
                                    (2.0, 0.5)
                                ],
                                title = Some "values",
                                mode = G.LinesPoints
                            })
                        ]
                    }))
                ]
            })
            G.SvgOptions {}
    "##;

    let result = eval_rex(source, None, fixture.state)
        .await
        .expect("render gnuplot SVG in Docker");
    let svg_hash = content_hash(ok_value(&result));
    let svg = fixture.store.get(svg_hash).await.expect("read gnuplot SVG");
    assert!(
        svg.windows(b"<svg".len()).any(|window| window == b"<svg"),
        "gnuplot output is not SVG"
    );
}

#[tokio::test]
async fn docker_graphviz_renders_declared_input_as_svg() {
    let Some(fixture) = docker_fixture() else {
        return;
    };
    let source = r#"
        import tools.graphviz as G;

        G.render
            (G.Graph {
                nodes = {
                    prepare = G.NodeAttributes {},
                    render = G.NodeAttributes {}
                },
                edges = [
                    G.Edge {
                        from = G.Endpoint { node = "prepare", port = None },
                        to = G.Endpoint { node = "render", port = None },
                        attributes = G.EdgeAttributes {}
                    }
                ]
            })
            G.LayoutDot
            G.FormatSvg
    "#;

    let result = eval_rex(source, None, fixture.state)
        .await
        .expect("render Graphviz SVG in Docker");
    let svg_hash = content_hash(ok_value(&result));
    let svg = fixture
        .store
        .get(svg_hash)
        .await
        .expect("read Graphviz SVG");
    assert!(
        svg.windows(b"<svg".len()).any(|window| window == b"<svg"),
        "Graphviz output is not SVG"
    );
}

#[tokio::test]
async fn docker_reimports_generated_media_and_images() {
    let Some(fixture) = docker_fixture() else {
        return;
    };
    let source = r##"
        import tools.ffmpeg as FF;
        import tools.imagemagick as IM;

        (
            FF.transcode
                (FF.SineAudio (FF.SineAudioSource {
                    frequency = 440.0,
                    sample_rate = 8000,
                    duration = Some (FF.Time { seconds = 0.1 })
                }))
                []
                (FF.Encoding {
                    format = FF.ContainerFormat { name = "wav" },
                    video = None,
                    audio = Some (FF.AudioEncoding {
                        codec = FF.PcmS16Le,
                        options = []
                    }),
                    subtitle = None,
                    options = [],
                    metadata = dict_empty
                }),
            IM.generate
                (IM.LinearGradient
                    (IM.Size { width = 16, height = 16 })
                    (IM.Color { value = "#336699" })
                    (IM.Color { value = "#99ccff" }))
                []
                (IM.Encoding {
                    format = IM.Format { name = "png" },
                    mode = IM.AdjoinFrames,
                    options = []
                })
        )
    "##;

    let result = eval_rex(source, None, fixture.state)
        .await
        .expect("generate media and image in Docker");
    let outputs = result
        .as_array()
        .unwrap_or_else(|| panic!("expected a tuple of generated artifacts, got {result}"));
    assert_eq!(outputs.len(), 2);

    let wav_hash = content_hash(ok_value(&outputs[0]));
    let wav = fixture
        .store
        .get(wav_hash)
        .await
        .expect("read generated WAV");
    assert!(
        wav.starts_with(b"RIFF") && wav.get(8..12) == Some(b"WAVE"),
        "FFmpeg output is not a WAV file"
    );

    let png_hash = content_hash(ok_value(&outputs[1]));
    let png = fixture
        .store
        .get(png_hash)
        .await
        .expect("read generated PNG");
    assert!(
        png.starts_with(b"\x89PNG\r\n\x1a\n"),
        "ImageMagick output is not a PNG file"
    );
}

#[tokio::test]
async fn docker_materializes_pdf_inputs_for_qpdf_and_poppler() {
    let Some(fixture) = docker_fixture() else {
        return;
    };
    let pdf_hash = fixture
        .store
        .put(sample_pdf())
        .await
        .expect("store input PDF");
    let inputs = Some(json!({ "input": pdf_hash.to_hex().to_string() }));

    let qpdf_source = r#"
        import std.artifacts (Pdf);
        import tools.qpdf as Q;

        fn main (input: Hash) -> Result u64 Q.QpdfError =
            Q.show_npages (Pdf { content = input }) None;
    "#;
    let qpdf = eval_rex(qpdf_source, inputs.clone(), fixture.state.clone())
        .await
        .expect("count PDF pages with QPDF in Docker");
    assert_eq!(ok_value(&qpdf), &json!(1));

    let poppler_source = r#"
        import std.artifacts (Pdf);
        import tools.poppler as P;

        fn main (input: Hash) -> Result P.PdfInfo P.PopplerError =
            P.pdfinfo
                (Pdf { content = input })
                (P.PdfInfoOptions {
                    first_page = None,
                    last_page = None,
                    owner_password = None,
                    user_password = None
                });
    "#;
    let poppler = eval_rex(poppler_source, inputs, fixture.state)
        .await
        .expect("inspect PDF with Poppler in Docker");
    assert_eq!(ok_value(&poppler)["pages"], 1);
}

fn sample_pdf() -> Vec<u8> {
    let stream = b"BT /F1 18 Tf 72 720 Td (Hello PDF) Tj ET\n";
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        [
            format!("<< /Length {} >>\nstream\n", stream.len()).into_bytes(),
            stream.to_vec(),
            b"endstream".to_vec(),
        ]
        .concat(),
    ];
    let mut pdf = b"%PDF-1.4\n%\x80\x81\x82\x83\n".to_vec();
    let mut offsets = vec![0_usize];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.into_iter().skip(1) {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

fn docker_executor_fixture() -> Option<(Store, DockerToolExecutor)> {
    if !docker_tests_enabled() {
        eprintln!("skipping Docker integration test; set {ENABLE_ENV}=1 to enable the suite");
        return None;
    }
    let images = OciToolImages::development(
        OciPlatform::native_linux(),
        image_reference(FFMPEG_IMAGE_ENV, "rex-tool-ffmpeg:local"),
        image_reference(GNUPLOT_IMAGE_ENV, "rex-tool-gnuplot:local"),
        image_reference(GRAPHVIZ_IMAGE_ENV, "rex-tool-graphviz:local"),
        image_reference(IMAGEMAGICK_IMAGE_ENV, "rex-tool-imagemagick:local"),
        image_reference(QPDF_IMAGE_ENV, "rex-tool-qpdf:local"),
        image_reference(POPPLER_IMAGE_ENV, "rex-tool-poppler:local"),
    );
    Some((Store::new_in_memory(), DockerToolExecutor::new(images)))
}

fn ffmpeg_png_plan(kind: OutputKind, color: &str) -> ToolExecutionPlan {
    let output = match kind {
        OutputKind::Single | OutputKind::Numbered => ToolArgument::output(0),
        OutputKind::Directory | OutputKind::Tree => {
            ToolArgument::output_with_suffix(0, "/frame.png")
        }
    };
    let mut arguments = vec![
        ToolArgument::literal("-hide_banner"),
        ToolArgument::literal("-loglevel"),
        ToolArgument::literal("error"),
        ToolArgument::literal("-y"),
        ToolArgument::literal("-f"),
        ToolArgument::literal("lavfi"),
        ToolArgument::literal("-i"),
        ToolArgument::literal(format!("color=c={color}:s=16x16:r=1:d=2")),
        ToolArgument::literal("-frames:v"),
        ToolArgument::literal(if kind == OutputKind::Numbered {
            "2"
        } else {
            "1"
        }),
        output,
    ];
    if kind == OutputKind::Numbered {
        arguments.insert(arguments.len() - 1, ToolArgument::literal("-start_number"));
        arguments.insert(arguments.len() - 1, ToolArgument::literal("1"));
    }
    ToolExecutionPlan {
        program: ToolProgram::Ffmpeg,
        arguments,
        inputs: Vec::new(),
        outputs: vec![ExpectedOutput {
            kind,
            extension: "png".to_owned(),
        }],
    }
}

fn literal_plan(program: ToolProgram, arguments: &[&str]) -> ToolExecutionPlan {
    ToolExecutionPlan {
        program,
        arguments: arguments
            .iter()
            .copied()
            .map(ToolArgument::literal)
            .collect(),
        inputs: Vec::new(),
        outputs: Vec::new(),
    }
}

#[tokio::test]
async fn docker_supports_every_output_kind() {
    let Some((store, executor)) = docker_executor_fixture() else {
        return;
    };
    for (kind, color) in [
        (OutputKind::Single, "red"),
        (OutputKind::Numbered, "green"),
        (OutputKind::Directory, "blue"),
        (OutputKind::Tree, "yellow"),
    ] {
        let execution = executor
            .execute(&store, ffmpeg_png_plan(kind, color))
            .await
            .unwrap_or_else(|error| panic!("execute {kind:?} output plan: {error}"));
        assert_eq!(
            execution.exit_code,
            Some(0),
            "{kind:?}: {}",
            String::from_utf8_lossy(&execution.stderr)
        );
        let hashes = execution.outputs.get(&0).expect("output entry");
        let expected_count = if kind == OutputKind::Numbered { 2 } else { 1 };
        assert_eq!(hashes.len(), expected_count, "{kind:?}");
        if kind == OutputKind::Tree {
            let tree = store.get_tree(hashes[0]).await.expect("read output tree");
            assert!(tree.contains_key("frame.png"));
        } else {
            for hash in hashes {
                assert!(
                    store
                        .get(*hash)
                        .await
                        .expect("read output blob")
                        .starts_with(b"\x89PNG")
                );
            }
        }
    }
}

#[tokio::test]
async fn docker_enforces_input_root_host_and_network_isolation() {
    let Some((store, executor)) = docker_executor_fixture() else {
        return;
    };

    let input_bytes = b"immutable input".to_vec();
    let input_hash = store.put(input_bytes.clone()).await.unwrap();
    let mut overwrite_input = literal_plan(
        ToolProgram::Ffmpeg,
        &[
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=8x8",
            "-frames:v",
            "1",
            "/work/inputs/input-0000.bin",
        ],
    );
    overwrite_input.inputs.push(CasInput {
        hash: input_hash,
        extension: "bin".to_owned(),
        kind: InputKind::Blob,
    });
    let execution = executor.execute(&store, overwrite_input).await.unwrap();
    assert_ne!(execution.exit_code, Some(0));
    assert_eq!(store.get(input_hash).await.unwrap(), input_bytes);

    let root_write = executor
        .execute(
            &store,
            literal_plan(
                ToolProgram::Ffmpeg,
                &[
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=c=red:s=8x8",
                    "-frames:v",
                    "1",
                    "/must-not-write.png",
                ],
            ),
        )
        .await
        .unwrap();
    assert_ne!(root_write.exit_code, Some(0));

    let sentinel = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(sentinel.path(), b"host secret").unwrap();
    let sentinel_path = sentinel.path().to_string_lossy().into_owned();
    let hidden_host_file = executor
        .execute(
            &store,
            literal_plan(ToolProgram::Ffprobe, &["-v", "error", &sentinel_path]),
        )
        .await
        .unwrap();
    assert_ne!(hidden_host_file.exit_code, Some(0));

    let network = executor
        .execute(
            &store,
            literal_plan(
                ToolProgram::Ffprobe,
                &[
                    "-v",
                    "error",
                    "-rw_timeout",
                    "1000000",
                    "https://example.com/",
                ],
            ),
        )
        .await
        .unwrap();
    assert_ne!(network.exit_code, Some(0));
}

#[tokio::test]
async fn docker_uses_packaged_and_cas_supplied_fonts() {
    let Some((store, executor)) = docker_executor_fixture() else {
        return;
    };

    let fallback = ToolExecutionPlan {
        program: ToolProgram::ImageMagick,
        arguments: [
            "-background",
            "white",
            "-fill",
            "black",
            "-font",
            "DejaVu-Sans",
            "label:Rex",
        ]
        .into_iter()
        .map(ToolArgument::literal)
        .chain(std::iter::once(ToolArgument::output(0)))
        .collect(),
        inputs: Vec::new(),
        outputs: vec![ExpectedOutput {
            kind: OutputKind::Single,
            extension: "png".to_owned(),
        }],
    };
    let execution = executor.execute(&store, fallback).await.unwrap();
    assert_eq!(
        execution.exit_code,
        Some(0),
        "{}",
        String::from_utf8_lossy(&execution.stderr)
    );

    let font = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--pull=never",
            "--network=none",
            "--entrypoint",
            "cat",
            &image_reference(IMAGEMAGICK_IMAGE_ENV, "rex-tool-imagemagick:local"),
            "/usr/share/fonts/dejavu/DejaVuSans.ttf",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("extract packaged test font");
    assert!(
        font.status.success(),
        "{}",
        String::from_utf8_lossy(&font.stderr)
    );
    let font_hash = store.put(font.stdout).await.unwrap();
    let cas_font = ToolExecutionPlan {
        program: ToolProgram::ImageMagick,
        arguments: vec![
            ToolArgument::literal("-background"),
            ToolArgument::literal("white"),
            ToolArgument::literal("-fill"),
            ToolArgument::literal("black"),
            ToolArgument::literal("-font"),
            ToolArgument::input(0),
            ToolArgument::literal("label:Rex"),
            ToolArgument::output(0),
        ],
        inputs: vec![CasInput {
            hash: font_hash,
            extension: "ttf".to_owned(),
            kind: InputKind::Blob,
        }],
        outputs: vec![ExpectedOutput {
            kind: OutputKind::Single,
            extension: "png".to_owned(),
        }],
    };
    let execution = executor.execute(&store, cas_font).await.unwrap();
    assert_eq!(
        execution.exit_code,
        Some(0),
        "{}",
        String::from_utf8_lossy(&execution.stderr)
    );
}

#[tokio::test]
async fn docker_distinguishes_tool_failures_from_missing_images() {
    let Some((store, executor)) = docker_executor_fixture() else {
        return;
    };
    let failure = executor
        .execute(
            &store,
            literal_plan(ToolProgram::Ffmpeg, &["--definitely-not-an-ffmpeg-option"]),
        )
        .await
        .expect("ordinary tool failure is an execution result");
    assert_ne!(failure.exit_code, Some(0));
    assert!(!failure.stderr.is_empty());

    let missing = DockerToolExecutor::new(OciToolImages::development(
        OciPlatform::native_linux(),
        "rex-tool-image-that-does-not-exist:missing",
        "rex-tool-gnuplot:local",
        "rex-tool-graphviz:local",
        "rex-tool-imagemagick:local",
        "rex-tool-qpdf:local",
        "rex-tool-poppler:local",
    ));
    let error = missing
        .execute(&store, literal_plan(ToolProgram::Ffmpeg, &["-version"]))
        .await
        .expect_err("missing image is infrastructure failure");
    assert!(error.to_string().contains("create Docker tool container"));
    assert!(
        error
            .to_string()
            .contains("rex-tool-image-that-does-not-exist")
    );
}

fn rex_container_names() -> BTreeSet<String> {
    let output = Command::new("docker")
        .args([
            "container",
            "ls",
            "--all",
            "--format",
            "{{.Names}}",
            "--filter",
            "label=rex.workflow=true",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("list Rex containers");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn docker_cancellation_cleans_up_and_concurrent_runs_are_isolated() {
    let Some((store, executor)) = docker_executor_fixture() else {
        return;
    };
    let baseline = rex_container_names();
    let task_store = store.clone();
    let task_executor = executor.clone();
    let task = tokio::spawn(async move {
        task_executor
            .execute(
                &task_store,
                literal_plan(
                    ToolProgram::Ffmpeg,
                    &[
                        "-hide_banner",
                        "-loglevel",
                        "error",
                        "-re",
                        "-f",
                        "lavfi",
                        "-i",
                        "sine=frequency=440:duration=60",
                        "-f",
                        "null",
                        "-",
                    ],
                ),
            )
            .await
    });
    for _ in 0..50 {
        if rex_container_names().len() > baseline.len() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        rex_container_names().len() > baseline.len(),
        "long-running container never appeared"
    );
    task.abort();
    let _ = task.await;
    for _ in 0..50 {
        if rex_container_names() == baseline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        rex_container_names(),
        baseline,
        "cancellation leaked a container"
    );

    let red = executor.execute(&store, ffmpeg_png_plan(OutputKind::Single, "red"));
    let blue = executor.execute(&store, ffmpeg_png_plan(OutputKind::Single, "blue"));
    let (red, blue) = tokio::join!(red, blue);
    let red = red.unwrap();
    let blue = blue.unwrap();
    assert_eq!(red.exit_code, Some(0));
    assert_eq!(blue.exit_code, Some(0));
    assert_ne!(red.outputs[&0], blue.outputs[&0]);
    assert_eq!(
        rex_container_names(),
        baseline,
        "completed runs leaked containers"
    );
}
