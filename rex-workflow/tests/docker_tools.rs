use blake3::Hash;
use rex_workflow::{
    modules::tools::executor::DockerToolImages, run::eval_rex, state::State, storage::store::Store,
};
use serde_json::{Value, json};
use std::{env, str::FromStr};

const ENABLE_ENV: &str = "REX_WORKFLOW_DOCKER_TESTS";
const FFMPEG_IMAGE_ENV: &str = "REX_WORKFLOW_DOCKER_FFMPEG_IMAGE";
const IMAGEMAGICK_IMAGE_ENV: &str = "REX_WORKFLOW_DOCKER_IMAGEMAGICK_IMAGE";
const QPDF_IMAGE_ENV: &str = "REX_WORKFLOW_DOCKER_QPDF_IMAGE";
const POPPLER_IMAGE_ENV: &str = "REX_WORKFLOW_DOCKER_POPPLER_IMAGE";

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
    let images = DockerToolImages::new(
        image_reference(FFMPEG_IMAGE_ENV, "rex-tool-ffmpeg:local"),
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
        import tools.imagemagick as IM;
        import tools.qpdf as Q;
        import tools.poppler as P;

        (FF.version, IM.version, Q.version, P.version)
    "#;

    let result = eval_rex(source, None, fixture.state)
        .await
        .expect("run tool version workflow in Docker");
    let versions = result
        .as_array()
        .unwrap_or_else(|| panic!("expected a tuple of tool versions, got {result}"));

    assert_eq!(versions.len(), 4);
    for version in versions {
        let version = ok_value(version);
        assert!(
            version.get("version").and_then(Value::as_str).is_some(),
            "tool returned no parsed version: {version}"
        );
    }
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
        import tools.qpdf as Q;

        fn main (input: Hash) -> Result u64 Q.QpdfError =
            Q.show_npages (Q.Pdf { content = input }) None;
    "#;
    let qpdf = eval_rex(qpdf_source, inputs.clone(), fixture.state.clone())
        .await
        .expect("count PDF pages with QPDF in Docker");
    assert_eq!(ok_value(&qpdf), &json!(1));

    let poppler_source = r#"
        import tools.poppler as P;

        fn main (input: Hash) -> Result P.PdfInfo P.PopplerError =
            P.pdfinfo
                (P.Pdf { content = input })
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
