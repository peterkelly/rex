mod compile;
pub mod types;

use crate::{modules::tools::executor::ToolExecution, state::State};
use compile::*;
use rex::engine::{EngineError, Module};
use types::*;

type QResult<T> = Result<T, QpdfError>;

pub fn module() -> Result<Module<State>, EngineError> {
    api::rex_module()
}

/// QPDF structural transformations and inspection for content-addressed PDF workflows.
///
/// Function and option names intentionally follow QPDF's command-line documentation. PDF inputs,
/// JSON reports, and generated PDFs are represented by content hashes rather than host paths.
/// Exit status 3 is preserved as a successful result with `warnings`; invalid requests and fatal
/// QPDF failures are returned as `Err QpdfError`. The available behavior depends on the installed
/// `qpdf` executable.
#[rex::module(name = "tools.qpdf")]
mod api {
    use super::*;

    /// Run `qpdf --check` and return clean, warning, or error status with complete diagnostics.
    ///
    /// A syntactically damaged PDF is a `CheckErrors` report rather than a Rex evaluation failure.
    /// `password` is passed with QPDF's `--password` option when present.
    #[rex::export]
    pub(super) async fn check(
        state: State,
        pdf: Pdf,
        password: Option<String>,
    ) -> Result<QResult<CheckReport>, EngineError> {
        let execution = execute(&state, check_plan(pdf, password)).await?;
        let status = match execution.exit_code {
            Some(0) => CheckStatus::CheckClean,
            Some(3) => CheckStatus::CheckWarnings,
            Some(2) => CheckStatus::CheckErrors,
            _ => return Ok(Err(process_error(&execution))),
        };
        Ok(Ok(CheckReport {
            status,
            diagnostics: diagnostics(&execution),
        }))
    }

    /// Return the page count printed by `qpdf --show-npages`.
    #[rex::export]
    pub(super) async fn show_npages(
        state: State,
        pdf: Pdf,
        password: Option<String>,
    ) -> Result<QResult<u64>, EngineError> {
        let execution = execute(&state, show_npages_plan(pdf, password)).await?;
        if execution.exit_code != Some(0) {
            return Ok(Err(process_error(&execution)));
        }
        let text = std::str::from_utf8(&execution.stdout).map_err(|_| {
            EngineError::Custom("qpdf --show-npages returned non-UTF-8 output".into())
        })?;
        match text.trim().parse::<u64>() {
            Ok(pages) => Ok(Ok(pages)),
            Err(_) => Ok(Err(unexpected(format!(
                "qpdf --show-npages returned `{}`",
                text.trim()
            )))),
        }
    }

    /// Write QPDF JSON version 2 for selected top-level keys and PDF objects.
    ///
    /// This is QPDF's `--json`, not QPDFJob JSON. Stream data can be omitted or included inline;
    /// separate stream-data files are not exposed because they would break the single JSON artifact.
    #[rex::export]
    pub(super) async fn json(
        state: State,
        pdf: Pdf,
        password: Option<String>,
        options: JsonOptions,
    ) -> Result<QResult<JsonOutput>, EngineError> {
        let plan = match json_plan(pdf, password, options) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        let execution = execute(&state, plan).await?;
        if !is_write_success(&execution) {
            return Ok(Err(process_error(&execution)));
        }
        match execution.outputs.get(&0).map(Vec::as_slice) {
            Some([content]) => Ok(Ok(JsonOutput {
                json: JsonFile { content: *content },
                warnings: warnings(&execution),
            })),
            Some(values) => Ok(Err(unexpected(format!(
                "qpdf --json produced {} files instead of one",
                values.len()
            )))),
            None => Ok(Err(unexpected("qpdf --json did not declare its output"))),
        }
    }

    /// Rewrite one PDF using ordered QPDF output and transformation options.
    ///
    /// `options` use QPDF terminology including `Linearize`, `ObjectStreams`, `Rotate`, `Decrypt`,
    /// and `Encrypt`. QPDF processes option groups according to its documented command-line rules.
    #[rex::export]
    pub(super) async fn transform(
        state: State,
        pdf: Pdf,
        password: Option<String>,
        options: Vec<WriteOption>,
    ) -> Result<QResult<PdfOutput>, EngineError> {
        let plan = match transform_plan(pdf, password, options) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        single_pdf_output(&state, plan).await
    }

    /// Run QPDF's `--pages` operation to select, merge, collate, or reorder PDF pages.
    ///
    /// `primary` supplies document-level information retained by QPDF; `None` uses `--empty`.
    /// Each `PageSource.range` uses QPDF page-range syntax such as `1-5`, `z-1`, or `1-z:odd`.
    /// `collate` maps to the positive page-group counts in QPDF's `--collate=n[,m,...]` option.
    #[rex::export]
    pub(super) async fn pages(
        state: State,
        primary: Option<Pdf>,
        password: Option<String>,
        sources: Vec<PageSource>,
        collate: Option<Vec<u64>>,
        options: Vec<WriteOption>,
    ) -> Result<QResult<PdfOutput>, EngineError> {
        let plan = match pages_plan(primary, password, sources, collate, options) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        single_pdf_output(&state, plan).await
    }

    /// Run QPDF's `--split-pages` operation and return numbered PDFs in page order.
    ///
    /// `pages_per_file` is the number of consecutive source pages written to each output PDF.
    #[rex::export]
    pub(super) async fn split_pages(
        state: State,
        pdf: Pdf,
        password: Option<String>,
        pages_per_file: u64,
        options: Vec<WriteOption>,
    ) -> Result<QResult<PdfSequenceOutput>, EngineError> {
        let plan = match split_pages_plan(pdf, password, pages_per_file, options) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        pdf_sequence_output(&state, plan).await
    }

    /// Overlay pages from `overlay_pdf` onto `pdf` using QPDF's page mapping options.
    #[rex::export]
    pub(super) async fn overlay(
        state: State,
        pdf: Pdf,
        password: Option<String>,
        overlay_pdf: Pdf,
        spec: OverlaySpec,
        options: Vec<WriteOption>,
    ) -> Result<QResult<PdfOutput>, EngineError> {
        let plan = match overlay_plan(false, pdf, password, overlay_pdf, spec, options) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        single_pdf_output(&state, plan).await
    }

    /// Place pages from `underlay_pdf` beneath `pdf` using QPDF's page mapping options.
    #[rex::export]
    pub(super) async fn underlay(
        state: State,
        pdf: Pdf,
        password: Option<String>,
        underlay_pdf: Pdf,
        spec: OverlaySpec,
        options: Vec<WriteOption>,
    ) -> Result<QResult<PdfOutput>, EngineError> {
        let plan = match overlay_plan(true, pdf, password, underlay_pdf, spec, options) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        single_pdf_output(&state, plan).await
    }

    /// Return the installed QPDF version reported by `qpdf --version`.
    #[rex::export]
    pub(super) async fn version(state: State) -> Result<QResult<VersionInfo>, EngineError> {
        let execution = execute(&state, version_plan()).await?;
        if execution.exit_code != Some(0) {
            return Ok(Err(process_error(&execution)));
        }
        let text = String::from_utf8(execution.stdout)
            .map_err(|_| EngineError::Custom("qpdf --version returned non-UTF-8 output".into()))?;
        let first = text
            .lines()
            .next()
            .ok_or_else(|| EngineError::Custom("qpdf --version returned no output".into()))?;
        let version = first
            .strip_prefix("qpdf version ")
            .unwrap_or(first)
            .trim()
            .to_string();
        Ok(Ok(VersionInfo { version }))
    }
}

async fn single_pdf_output(
    state: &State,
    plan: crate::modules::tools::executor::ToolExecutionPlan,
) -> Result<QResult<PdfOutput>, EngineError> {
    let execution = execute(state, plan).await?;
    if !is_write_success(&execution) {
        return Ok(Err(process_error(&execution)));
    }
    match execution.outputs.get(&0).map(Vec::as_slice) {
        Some([content]) => Ok(Ok(PdfOutput {
            pdf: Pdf { content: *content },
            warnings: warnings(&execution),
        })),
        Some(values) => Ok(Err(unexpected(format!(
            "QPDF produced {} files instead of one",
            values.len()
        )))),
        None => Ok(Err(unexpected("QPDF did not declare its output"))),
    }
}

async fn pdf_sequence_output(
    state: &State,
    plan: crate::modules::tools::executor::ToolExecutionPlan,
) -> Result<QResult<PdfSequenceOutput>, EngineError> {
    let execution = execute(state, plan).await?;
    if !is_write_success(&execution) {
        return Ok(Err(process_error(&execution)));
    }
    let pdfs = execution
        .outputs
        .get(&0)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|content| Pdf { content })
        .collect::<Vec<_>>();
    if pdfs.is_empty() {
        Ok(Err(unexpected("QPDF produced no split PDF files")))
    } else {
        Ok(Ok(PdfSequenceOutput {
            pdfs,
            warnings: warnings(&execution),
        }))
    }
}

async fn execute(
    state: &State,
    plan: crate::modules::tools::executor::ToolExecutionPlan,
) -> Result<ToolExecution, EngineError> {
    state
        .tools
        .execute(&state.store, plan)
        .await
        .map_err(|error| EngineError::Custom(error.to_string()))
}

fn is_write_success(execution: &ToolExecution) -> bool {
    matches!(execution.exit_code, Some(0 | 3))
}

fn warnings(execution: &ToolExecution) -> String {
    if execution.exit_code == Some(3) {
        diagnostics(execution)
    } else {
        String::new()
    }
}

fn diagnostics(execution: &ToolExecution) -> String {
    let stdout = String::from_utf8_lossy(&execution.stdout)
        .trim()
        .to_string();
    let stderr = String::from_utf8_lossy(&execution.stderr)
        .trim()
        .to_string();
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout,
        (true, false) => stderr,
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

fn process_error(execution: &ToolExecution) -> QpdfError {
    QpdfError {
        kind: QpdfErrorKind::ProcessFailed,
        exit_code: execution.exit_code.map(i64::from),
        message: diagnostics(execution),
    }
}

fn unexpected(message: impl Into<String>) -> QpdfError {
    QpdfError {
        kind: QpdfErrorKind::UnexpectedOutput,
        exit_code: None,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::api::*;
    use super::*;
    use crate::storage::store::Store;

    #[tokio::test]
    async fn real_qpdf_checks_transforms_merges_splits_and_exports_json_when_available() {
        if std::process::Command::new("qpdf")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let store = Store::new_in_memory();
        let source = Pdf {
            content: store.put(sample_pdf()).await.unwrap(),
        };
        let state = State::local(store);

        let report = check(state.clone(), source.clone(), None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(report.status, CheckStatus::CheckClean);
        assert_eq!(
            show_npages(state.clone(), source.clone(), None)
                .await
                .unwrap()
                .unwrap(),
            1
        );

        let transformed = transform(
            state.clone(),
            source.clone(),
            None,
            vec![
                WriteOption::Linearize,
                WriteOption::ObjectStreams(ObjectStreamMode::ObjectStreamsGenerate),
                WriteOption::Rotate(RotationSpec {
                    rotation: Rotation::RelativeRotation(90),
                    pages: Some("1".to_string()),
                }),
            ],
        )
        .await
        .unwrap()
        .unwrap();
        assert!(transformed.warnings.is_empty());
        assert_eq!(
            check(state.clone(), transformed.pdf, None)
                .await
                .unwrap()
                .unwrap()
                .status,
            CheckStatus::CheckClean
        );

        let overlaid = overlay(
            state.clone(),
            source.clone(),
            None,
            source.clone(),
            OverlaySpec {
                password: None,
                to: Some("1".to_string()),
                from: Some("1".to_string()),
                repeat: None,
            },
            vec![],
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            check(state.clone(), overlaid.pdf, None)
                .await
                .unwrap()
                .unwrap()
                .status,
            CheckStatus::CheckClean
        );

        let encrypted = transform(
            state.clone(),
            source.clone(),
            None,
            vec![WriteOption::Encrypt(EncryptionSpec {
                user_password: Some("reader".to_string()),
                owner_password: Some("owner".to_string()),
                method: EncryptionMethod::EncryptionAes256,
                print: PrintPermission::PrintLowResolution,
                modify: ModifyPermission::ModifyNone,
                extract: false,
                accessibility: true,
                annotate: false,
                assemble: false,
                form: false,
                cleartext_metadata: false,
            })],
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            show_npages(
                state.clone(),
                encrypted.pdf.clone(),
                Some("reader".to_string())
            )
            .await
            .unwrap()
            .unwrap(),
            1
        );
        let decrypted = transform(
            state.clone(),
            encrypted.pdf,
            Some("reader".to_string()),
            vec![WriteOption::Decrypt],
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            check(state.clone(), decrypted.pdf, None)
                .await
                .unwrap()
                .unwrap()
                .status,
            CheckStatus::CheckClean
        );

        let merged = pages(
            state.clone(),
            None,
            None,
            vec![
                PageSource {
                    pdf: source.clone(),
                    range: "1".to_string(),
                    password: None,
                },
                PageSource {
                    pdf: source.clone(),
                    range: "1".to_string(),
                    password: None,
                },
            ],
            Some(vec![1, 1]),
            vec![],
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            show_npages(state.clone(), merged.pdf.clone(), None)
                .await
                .unwrap()
                .unwrap(),
            2
        );
        let split = split_pages(state.clone(), merged.pdf, None, 1, vec![])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(split.pdfs.len(), 2);

        let json = json(
            state.clone(),
            source,
            None,
            JsonOptions {
                keys: vec![JsonKey::JsonPages],
                objects: vec![],
                stream_data: JsonStreamData::JsonStreamDataNone,
                decode_level: DecodeLevel::DecodeNone,
            },
        )
        .await
        .unwrap()
        .unwrap();
        let bytes = state.store.get(json.json.content).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["pages"].as_array().unwrap().len(), 1);
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
}
