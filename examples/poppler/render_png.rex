// Render every selected PDF page as an ordered list of 144-DPI PNG blobs.
// Input JSON: {"input":"<pdf-hash>"}
import std.artifacts (Pdf);
import tools.poppler as P;

fn main (input: Hash) -> Result P.CairoOutput P.PopplerError =
    P.pdftocairo
        (Pdf { content = input })
        P.CairoFormat.Png
        P.PdfToCairoOptions {
            resolution = Some 144.0,
        };
