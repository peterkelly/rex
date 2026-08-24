// Extract every embedded image in its most appropriate/native representation.
// Input JSON: {"input":"<pdf-hash>"}
import std.artifacts (Pdf);
import tools.poppler as P;

fn main (input: Hash) -> Result P.ExtractedImages P.PopplerError =
    P.pdfimages
        (Pdf { content = input })
        P.PdfImagesOptions {
            format = P.PdfImagesFormat.All,
            include_page_numbers = true
        };
