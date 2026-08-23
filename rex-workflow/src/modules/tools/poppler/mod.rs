mod compile;
pub mod types;

use crate::{modules::tools::executor::ToolExecution, state::State};
use compile::*;
use rex::engine::{EngineError, Module};
use rex::storage::EntryKind;
use std::collections::BTreeMap;
use types::*;

type PopResult<T> = Result<T, PopplerError>;

pub fn module() -> Result<Module<State>, EngineError> {
    api::rex_module()
}

/// Poppler command-line utilities for inspecting, extracting, and rendering stored PDFs.
///
/// Exported function and option names intentionally follow `pdfinfo`, `pdftotext`, `pdftocairo`,
/// and `pdfimages` documentation. PDF inputs use the shared `std.artifacts.Pdf` type and generated
/// files remain content-addressed; heterogeneous `pdfimages` output is preserved as a CAS tree.
/// Expected command failures are returned as
/// `Err PopplerError`, while storage and executor failures remain Rex evaluation errors.
#[rex::module(
    name = "tools.poppler",
    defaults(PdfInfoOptions, PdfToTextOptions, PdfToCairoOptions, PdfImagesOptions,)
)]
mod api {
    use super::*;

    /// Run `pdfinfo -box -isodates` and parse document metadata plus requested page geometry.
    ///
    /// `first_page` and `last_page` are one-based and correspond to `pdfinfo -f` and `-l`.
    #[rex::export]
    pub(super) async fn pdfinfo(
        state: State,
        pdf: Pdf,
        options: PdfInfoOptions,
    ) -> Result<PopResult<PdfInfo>, EngineError> {
        let plan = match pdfinfo_plan(pdf, options) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        let execution = execute(&state, plan).await?;
        if execution.exit_code != Some(0) {
            return Ok(Err(process_error(&execution)));
        }
        Ok(parse_pdfinfo(&execution.stdout))
    }

    /// Run `pdftotext` and return its UTF-8 plain text, XHTML, or TSV as a CAS-backed file.
    ///
    /// `format` selects documented modes such as `-layout`, `-raw`, `-bbox-layout`, and `-tsv`.
    /// Structured formats preserve page and word geometry for downstream processing.
    #[rex::export]
    pub(super) async fn pdftotext(
        state: State,
        pdf: Pdf,
        options: PdfToTextOptions,
    ) -> Result<PopResult<TextFile>, EngineError> {
        let format = options.format;
        let plan = match pdftotext_plan(pdf, options) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        let execution = execute(&state, plan).await?;
        if execution.exit_code != Some(0) {
            return Ok(Err(process_error(&execution)));
        }
        match execution.outputs.get(&0).map(Vec::as_slice) {
            Some([content]) => Ok(Ok(TextFile {
                content: *content,
                format,
            })),
            Some(values) => Ok(Err(unexpected(format!(
                "pdftotext produced {} files instead of one",
                values.len()
            )))),
            None => Ok(Err(unexpected("pdftotext did not declare its output"))),
        }
    }

    /// Run `pdftocairo` and return one vector/single-page file or an ordered raster page sequence.
    ///
    /// PNG, JPEG, and TIFF normally produce one file per selected page; `single_file` requests only
    /// the first selected page. PDF and PostScript produce one file. SVG and EPS require one explicit
    /// page because those formats cannot represent an arbitrary PDF page sequence as one artifact.
    #[rex::export]
    pub(super) async fn pdftocairo(
        state: State,
        pdf: Pdf,
        format: CairoFormat,
        options: PdfToCairoOptions,
    ) -> Result<PopResult<CairoOutput>, EngineError> {
        let (plan, planned) = match pdftocairo_plan(pdf, format, options) {
            Ok(value) => value,
            Err(error) => return Ok(Err(error)),
        };
        let execution = execute(&state, plan).await?;
        if execution.exit_code != Some(0) {
            return Ok(Err(process_error(&execution)));
        }
        let files = if planned.raster {
            match raster_cairo_files(&state, &execution).await? {
                Ok(files) => files,
                Err(error) => return Ok(Err(error)),
            }
        } else {
            execution
                .outputs
                .get(&0)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|content| OutputFile { content })
                .collect::<Vec<_>>()
        };
        if files.is_empty() {
            return Ok(Err(unexpected("pdftocairo produced no output files")));
        }
        if planned.multiple {
            Ok(Ok(CairoOutput::CairoPageFiles(files)))
        } else {
            match files.as_slice() {
                [file] => Ok(Ok(CairoOutput::CairoSingleFile(file.clone()))),
                _ => Ok(Err(unexpected(format!(
                    "pdftocairo produced {} files where one was expected",
                    files.len()
                )))),
            }
        }
    }

    /// Run `pdfimages` and preserve all extracted image and auxiliary files in one CAS tree.
    ///
    /// `format` maps to flags including `-png`, `-tiff`, `-j`, `-jp2`, `-jbig2`, `-ccitt`, and
    /// `-all`. The tree retains filenames and extensions because one PDF may contain mixed encodings.
    #[rex::export]
    pub(super) async fn pdfimages(
        state: State,
        pdf: Pdf,
        options: PdfImagesOptions,
    ) -> Result<PopResult<ExtractedImages>, EngineError> {
        let plan = match pdfimages_plan(pdf, options) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        let execution = execute(&state, plan).await?;
        if execution.exit_code != Some(0) {
            return Ok(Err(process_error(&execution)));
        }
        match execution.outputs.get(&0).map(Vec::as_slice) {
            Some([content]) => Ok(Ok(ExtractedImages { content: *content })),
            Some(values) => Ok(Err(unexpected(format!(
                "pdfimages produced {} output roots instead of one",
                values.len()
            )))),
            None => Ok(Err(unexpected("pdfimages did not declare its output tree"))),
        }
    }

    /// Run `pdfimages -list` and parse one typed record for every image object.
    #[rex::export]
    pub(super) async fn pdfimages_list(
        state: State,
        pdf: Pdf,
        options: PdfImagesOptions,
    ) -> Result<PopResult<Vec<PdfImageInfo>>, EngineError> {
        let plan = match pdfimages_list_plan(pdf, options) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        let execution = execute(&state, plan).await?;
        if execution.exit_code != Some(0) {
            return Ok(Err(process_error(&execution)));
        }
        Ok(parse_pdfimages_list(&execution.stdout))
    }

    /// Return the installed Poppler version reported by `pdfinfo -v`.
    #[rex::export]
    pub(super) async fn version(state: State) -> Result<PopResult<VersionInfo>, EngineError> {
        let execution = execute(&state, version_plan()).await?;
        if execution.exit_code != Some(0) {
            return Ok(Err(process_error(&execution)));
        }
        let output = diagnostics(&execution);
        let first = output
            .lines()
            .next()
            .ok_or_else(|| EngineError::Custom("pdfinfo -v returned no output".into()))?;
        let version = first
            .strip_prefix("pdfinfo version ")
            .unwrap_or(first)
            .trim()
            .to_string();
        Ok(Ok(VersionInfo { version }))
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

async fn raster_cairo_files(
    state: &State,
    execution: &ToolExecution,
) -> Result<PopResult<Vec<OutputFile>>, EngineError> {
    let Some([tree]) = execution.outputs.get(&0).map(Vec::as_slice) else {
        return Ok(Err(unexpected(
            "pdftocairo did not produce one raster output tree",
        )));
    };
    let entries =
        state.store.get_tree(*tree).await.map_err(|error| {
            EngineError::Custom(format!("read pdftocairo output tree: {error}"))
        })?;
    let mut pages = Vec::with_capacity(entries.len());
    for (name, entry) in entries {
        if entry.kind != EntryKind::Blob {
            return Ok(Err(unexpected(format!(
                "pdftocairo produced a nested output `{name}`"
            ))));
        }
        let Some(page) = cairo_page_number(&name) else {
            return Ok(Err(unexpected(format!(
                "pdftocairo produced an unrecognized filename `{name}`"
            ))));
        };
        pages.push((
            page,
            name,
            OutputFile {
                content: entry.hash,
            },
        ));
    }
    pages.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
    Ok(Ok(pages.into_iter().map(|(_, _, file)| file).collect()))
}

fn cairo_page_number(name: &str) -> Option<u64> {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    if stem == "page" {
        Some(0)
    } else {
        stem.strip_prefix("page-")?.parse().ok()
    }
}

fn parse_pdfinfo(bytes: &[u8]) -> PopResult<PdfInfo> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| unexpected("pdfinfo returned non-UTF-8 output"))?;
    let mut fields = BTreeMap::new();
    let mut pages = BTreeMap::<u64, PageInfo>::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        if parse_numbered_page_line(line, &mut pages)? {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        if matches!(
            name,
            "Page size" | "Page rot" | "MediaBox" | "CropBox" | "BleedBox" | "TrimBox" | "ArtBox"
        ) {
            apply_page_field(
                page_entry(&mut pages, 1),
                name.trim_start_matches("Page "),
                value,
            )?;
        } else {
            fields.insert(name.to_string(), value.to_string());
        }
    }
    let page_count = parse_required_u64(&fields, "Pages")?;
    let mut other = fields.clone();
    for key in [
        "Title",
        "Subject",
        "Keywords",
        "Author",
        "Creator",
        "Producer",
        "CreationDate",
        "ModDate",
        "Custom Metadata",
        "Metadata Stream",
        "Tagged",
        "UserProperties",
        "Suspects",
        "Form",
        "JavaScript",
        "Pages",
        "Encrypted",
        "File size",
        "Optimized",
        "Linearized",
        "PDF version",
    ] {
        other.remove(key);
    }
    Ok(PdfInfo {
        title: optional_field(&fields, "Title"),
        subject: optional_field(&fields, "Subject"),
        keywords: optional_field(&fields, "Keywords"),
        author: optional_field(&fields, "Author"),
        creator: optional_field(&fields, "Creator"),
        producer: optional_field(&fields, "Producer"),
        creation_date: optional_field(&fields, "CreationDate"),
        modification_date: optional_field(&fields, "ModDate"),
        custom_metadata: bool_field(&fields, "Custom Metadata"),
        metadata_stream: bool_field(&fields, "Metadata Stream"),
        tagged: bool_field(&fields, "Tagged"),
        user_properties: bool_field(&fields, "UserProperties"),
        suspects: bool_field(&fields, "Suspects"),
        form: fields.get("Form").cloned().unwrap_or_default(),
        javascript: bool_field(&fields, "JavaScript"),
        pages: page_count,
        encrypted: bool_field(&fields, "Encrypted"),
        file_size: fields
            .get("File size")
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse().ok()),
        linearized: bool_field(&fields, "Linearized") || bool_field(&fields, "Optimized"),
        pdf_version: fields.get("PDF version").cloned().unwrap_or_default(),
        page_info: pages.into_values().collect(),
        other,
    })
}

fn parse_numbered_page_line(
    line: &str,
    pages: &mut BTreeMap<u64, PageInfo>,
) -> Result<bool, PopplerError> {
    let Some((left, value)) = line.split_once(':') else {
        return Ok(false);
    };
    let parts = left.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 3 || parts[0] != "Page" {
        return Ok(false);
    }
    let page = parts[1]
        .parse::<u64>()
        .map_err(|_| unexpected(format!("invalid pdfinfo page number `{}`", parts[1])))?;
    apply_page_field(page_entry(pages, page), parts[2], value.trim())?;
    Ok(true)
}

fn page_entry(pages: &mut BTreeMap<u64, PageInfo>, page: u64) -> &mut PageInfo {
    pages.entry(page).or_insert_with(|| PageInfo {
        page,
        width: None,
        height: None,
        rotation: None,
        media_box: None,
        crop_box: None,
        bleed_box: None,
        trim_box: None,
        art_box: None,
    })
}

fn apply_page_field(page: &mut PageInfo, name: &str, value: &str) -> Result<(), PopplerError> {
    match name {
        "size" => {
            let values = value.split_whitespace().collect::<Vec<_>>();
            if values.len() < 3 || values[1] != "x" {
                return Err(unexpected(format!("invalid pdfinfo page size `{value}`")));
            }
            page.width = Some(parse_f64(values[0], "page width")?);
            page.height = Some(parse_f64(values[2], "page height")?);
        }
        "rot" => {
            page.rotation = Some(
                value
                    .parse()
                    .map_err(|_| unexpected(format!("invalid pdfinfo rotation `{value}`")))?,
            );
        }
        "MediaBox" => page.media_box = Some(parse_box(value)?),
        "CropBox" => page.crop_box = Some(parse_box(value)?),
        "BleedBox" => page.bleed_box = Some(parse_box(value)?),
        "TrimBox" => page.trim_box = Some(parse_box(value)?),
        "ArtBox" => page.art_box = Some(parse_box(value)?),
        _ => {}
    }
    Ok(())
}

fn parse_box(value: &str) -> Result<PageBox, PopplerError> {
    let values = value.split_whitespace().collect::<Vec<_>>();
    if values.len() < 4 {
        return Err(unexpected(format!("invalid pdfinfo page box `{value}`")));
    }
    Ok(PageBox {
        x1: parse_f64(values[0], "box x1")?,
        y1: parse_f64(values[1], "box y1")?,
        x2: parse_f64(values[2], "box x2")?,
        y2: parse_f64(values[3], "box y2")?,
    })
}

fn parse_pdfimages_list(bytes: &[u8]) -> PopResult<Vec<PdfImageInfo>> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| unexpected("pdfimages -list returned non-UTF-8 output"))?;
    let mut images = Vec::new();
    for line in text.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 16
            || !fields[0]
                .chars()
                .all(|character| character.is_ascii_digit())
        {
            continue;
        }
        let integer = |index: usize, name: &str| {
            fields[index]
                .parse::<u64>()
                .map_err(|_| unexpected(format!("invalid pdfimages {name} `{}`", fields[index])))
        };
        images.push(PdfImageInfo {
            page: integer(0, "page")?,
            index: integer(1, "index")?,
            image_type: fields[2].to_string(),
            width: integer(3, "width")?,
            height: integer(4, "height")?,
            color: fields[5].to_string(),
            components: integer(6, "component count")?,
            bits_per_component: integer(7, "bits per component")?,
            encoding: fields[8].to_string(),
            interpolation: fields[9] == "yes",
            object: integer(10, "object number")?,
            generation: integer(11, "generation number")?,
            x_pixels_per_inch: integer(12, "X resolution")?,
            y_pixels_per_inch: integer(13, "Y resolution")?,
            size: fields[14].to_string(),
            ratio: fields[15].to_string(),
        });
    }
    Ok(images)
}

fn optional_field(fields: &BTreeMap<String, String>, name: &str) -> Option<String> {
    fields.get(name).filter(|value| !value.is_empty()).cloned()
}

fn bool_field(fields: &BTreeMap<String, String>, name: &str) -> bool {
    fields
        .get(name)
        .is_some_and(|value| value == "yes" || value.starts_with("yes "))
}

fn parse_required_u64(fields: &BTreeMap<String, String>, name: &str) -> Result<u64, PopplerError> {
    fields
        .get(name)
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| unexpected(format!("pdfinfo did not return a valid {name}")))
}

fn parse_f64(value: &str, context: &str) -> Result<f64, PopplerError> {
    value
        .parse()
        .map_err(|_| unexpected(format!("invalid pdfinfo {context} `{value}`")))
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

fn process_error(execution: &ToolExecution) -> PopplerError {
    PopplerError {
        kind: PopplerErrorKind::ProcessFailed,
        exit_code: execution.exit_code.map(i64::from),
        message: diagnostics(execution),
    }
}

fn unexpected(message: impl Into<String>) -> PopplerError {
    PopplerError {
        kind: PopplerErrorKind::UnexpectedOutput,
        exit_code: None,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::api::*;
    use super::*;
    use rex::storage::Store;

    #[test]
    fn pdfinfo_output_is_decoded_semantically() {
        let info = parse_pdfinfo(
            b"Title: sample\nPages: 1\nEncrypted: no\nPage size: 612 x 792 pts\nPage rot: 0\nMediaBox: 0 0 612 792\nFile size: 500 bytes\nOptimized: yes\nPDF version: 1.7\n",
        )
        .unwrap();
        assert_eq!(info.title.as_deref(), Some("sample"));
        assert_eq!(info.pages, 1);
        assert_eq!(info.page_info[0].width, Some(612.0));
        assert!(info.linearized);
    }

    #[test]
    fn pdfimages_rows_are_decoded_semantically() {
        let rows = parse_pdfimages_list(
            b"page num type width height color comp bpc enc interp object ID x-ppi y-ppi size ratio\n---\n1 0 image 200 100 gray 1 8 image no 8 0 72 72 317B 1.6%\n",
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].width, 200);
        assert_eq!(rows[0].object, 8);
    }

    #[test]
    fn cairo_page_filenames_have_numeric_order_keys() {
        assert_eq!(cairo_page_number("page-2.png"), Some(2));
        assert_eq!(cairo_page_number("page-10.png"), Some(10));
        assert_eq!(cairo_page_number("page.png"), Some(0));
        assert_eq!(cairo_page_number("unexpected.png"), None);
    }

    #[tokio::test]
    async fn docker_poppler_inspects_extracts_and_renders_when_enabled() {
        if std::env::var("REX_WORKFLOW_DOCKER_TESTS").as_deref() != Ok("1") {
            return;
        }
        let store = Store::new_in_memory();
        let source = Pdf {
            content: store.put(sample_pdf()).await.unwrap(),
        };
        let state = State::docker(
            store,
            crate::modules::tools::executor::OciToolImages::current_development(),
        );

        let info = pdfinfo(
            state.clone(),
            source.clone(),
            PdfInfoOptions {
                first_page: Some(1),
                last_page: Some(1),
                owner_password: None,
                user_password: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(info.pages, 1);
        assert_eq!(info.page_info[0].width, Some(612.0));

        let text = pdftotext(
            state.clone(),
            source.clone(),
            PdfToTextOptions {
                first_page: None,
                last_page: None,
                format: TextFormat::TabSeparated,
                resolution: None,
                crop: None,
                crop_box: false,
                discard_diagonal_text: false,
                column_spacing: None,
                end_of_line: EndOfLine::EolUnix,
                no_page_breaks: false,
                owner_password: None,
                user_password: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
        let text = String::from_utf8(state.store.get(text.content).await.unwrap()).unwrap();
        assert!(text.contains("Hello"));
        assert!(text.contains("PDF"));

        let rendered = pdftocairo(
            state.clone(),
            source.clone(),
            CairoFormat::CairoPng,
            PdfToCairoOptions {
                first_page: Some(1),
                last_page: Some(1),
                page_selection: PageSelection::AllPages,
                single_file: true,
                resolution: Some(72.0),
                resolution_x: None,
                resolution_y: None,
                scale_to: None,
                scale_to_x: None,
                scale_to_y: None,
                crop: None,
                crop_box: false,
                color: CairoColorMode::CairoColor,
                transparent: false,
                antialias: CairoAntialias::AntialiasDefault,
                jpeg_options: vec![],
                tiff_compression: None,
                owner_password: None,
                user_password: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
        let CairoOutput::CairoSingleFile(rendered) = rendered else {
            panic!("expected one rendered page")
        };
        assert!(state.store.size(rendered.content).await.unwrap() > 100);

        let rendered_pdf = pdftocairo(
            state.clone(),
            source.clone(),
            CairoFormat::CairoPdf,
            PdfToCairoOptions {
                first_page: None,
                last_page: None,
                page_selection: PageSelection::AllPages,
                single_file: false,
                resolution: None,
                resolution_x: None,
                resolution_y: None,
                scale_to: None,
                scale_to_x: None,
                scale_to_y: None,
                crop: None,
                crop_box: false,
                color: CairoColorMode::CairoColor,
                transparent: false,
                antialias: CairoAntialias::AntialiasDefault,
                jpeg_options: vec![],
                tiff_compression: None,
                owner_password: None,
                user_password: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
        let CairoOutput::CairoSingleFile(rendered_pdf) = rendered_pdf else {
            panic!("expected one rendered PDF")
        };
        assert!(
            state
                .store
                .get(rendered_pdf.content)
                .await
                .unwrap()
                .starts_with(b"%PDF-")
        );

        let extracted = pdfimages(
            state.clone(),
            source.clone(),
            PdfImagesOptions {
                first_page: None,
                last_page: None,
                format: PdfImagesFormat::ImagesAll,
                include_page_numbers: true,
                owner_password: None,
                user_password: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            state
                .store
                .get_tree(extracted.content)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            pdfimages_list(
                state,
                source,
                PdfImagesOptions {
                    first_page: None,
                    last_page: None,
                    format: PdfImagesFormat::ImagesDefault,
                    include_page_numbers: false,
                    owner_password: None,
                    user_password: None,
                },
            )
            .await
            .unwrap()
            .unwrap()
            .is_empty()
        );
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
