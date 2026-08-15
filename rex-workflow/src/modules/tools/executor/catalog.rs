use super::{ToolBundle, ToolProgram};

/// Everything an executor needs to select and enter a tool runtime.
///
/// `prefix_arguments` holds arguments that select a subcommand inside a
/// shared executable. Keeping them here ensures local and container
/// executors enter the tool through the same command line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ToolRuntime {
    pub(super) bundle: ToolBundle,
    pub(super) local_executable: &'static str,
    pub(super) container_executable: &'static str,
    pub(super) prefix_arguments: &'static [&'static str],
}

pub(super) const fn runtime(program: ToolProgram) -> ToolRuntime {
    match program {
        ToolProgram::Ffmpeg => ToolRuntime {
            bundle: ToolBundle::Ffmpeg,
            local_executable: "ffmpeg",
            container_executable: "ffmpeg",
            prefix_arguments: &[],
        },
        ToolProgram::Ffprobe => ToolRuntime {
            bundle: ToolBundle::Ffmpeg,
            local_executable: "ffprobe",
            container_executable: "ffprobe",
            prefix_arguments: &[],
        },
        ToolProgram::Graphviz => ToolRuntime {
            bundle: ToolBundle::Graphviz,
            local_executable: "dot",
            container_executable: "dot",
            prefix_arguments: &[],
        },
        ToolProgram::ImageMagick => image_magick(&[]),
        ToolProgram::ImageMagickMogrify => image_magick(&["mogrify"]),
        ToolProgram::ImageMagickIdentify => image_magick(&["identify"]),
        ToolProgram::ImageMagickCompare => image_magick(&["compare"]),
        ToolProgram::ImageMagickComposite => image_magick(&["composite"]),
        ToolProgram::ImageMagickMontage => image_magick(&["montage"]),
        ToolProgram::ImageMagickStream => image_magick(&["stream"]),
        ToolProgram::Qpdf => ToolRuntime {
            bundle: ToolBundle::Qpdf,
            local_executable: "qpdf",
            container_executable: "qpdf",
            prefix_arguments: &[],
        },
        ToolProgram::PdfInfo => poppler("pdfinfo"),
        ToolProgram::PdfToText => poppler("pdftotext"),
        ToolProgram::PdfToCairo => poppler("pdftocairo"),
        ToolProgram::PdfImages => poppler("pdfimages"),
    }
}

const fn image_magick(prefix_arguments: &'static [&'static str]) -> ToolRuntime {
    ToolRuntime {
        bundle: ToolBundle::ImageMagick,
        local_executable: "magick",
        container_executable: "magick",
        prefix_arguments,
    }
}

const fn poppler(executable: &'static str) -> ToolRuntime {
    ToolRuntime {
        bundle: ToolBundle::Poppler,
        local_executable: executable,
        container_executable: executable,
        prefix_arguments: &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_maps_every_program_to_its_bundle_and_command() {
        let cases = [
            (ToolProgram::Ffmpeg, ToolBundle::Ffmpeg, "ffmpeg", &[][..]),
            (ToolProgram::Ffprobe, ToolBundle::Ffmpeg, "ffprobe", &[][..]),
            (ToolProgram::Graphviz, ToolBundle::Graphviz, "dot", &[][..]),
            (
                ToolProgram::ImageMagick,
                ToolBundle::ImageMagick,
                "magick",
                &[][..],
            ),
            (
                ToolProgram::ImageMagickMogrify,
                ToolBundle::ImageMagick,
                "magick",
                &["mogrify"][..],
            ),
            (
                ToolProgram::ImageMagickIdentify,
                ToolBundle::ImageMagick,
                "magick",
                &["identify"][..],
            ),
            (
                ToolProgram::ImageMagickCompare,
                ToolBundle::ImageMagick,
                "magick",
                &["compare"][..],
            ),
            (
                ToolProgram::ImageMagickComposite,
                ToolBundle::ImageMagick,
                "magick",
                &["composite"][..],
            ),
            (
                ToolProgram::ImageMagickMontage,
                ToolBundle::ImageMagick,
                "magick",
                &["montage"][..],
            ),
            (
                ToolProgram::ImageMagickStream,
                ToolBundle::ImageMagick,
                "magick",
                &["stream"][..],
            ),
            (ToolProgram::Qpdf, ToolBundle::Qpdf, "qpdf", &[][..]),
            (
                ToolProgram::PdfInfo,
                ToolBundle::Poppler,
                "pdfinfo",
                &[][..],
            ),
            (
                ToolProgram::PdfToText,
                ToolBundle::Poppler,
                "pdftotext",
                &[][..],
            ),
            (
                ToolProgram::PdfToCairo,
                ToolBundle::Poppler,
                "pdftocairo",
                &[][..],
            ),
            (
                ToolProgram::PdfImages,
                ToolBundle::Poppler,
                "pdfimages",
                &[][..],
            ),
        ];

        for (program, bundle, executable, prefix_arguments) in cases {
            assert_eq!(
                runtime(program),
                ToolRuntime {
                    bundle,
                    local_executable: executable,
                    container_executable: executable,
                    prefix_arguments,
                }
            );
            assert_eq!(program.bundle(), bundle);
        }
    }
}
