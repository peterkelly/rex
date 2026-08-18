// Parse document metadata and page boxes with pdfinfo -box -isodates.
// Input JSON: {"input":"<pdf-hash>"}
import std.artifacts (Pdf);
import tools.poppler as P;

fn main (input: Hash) -> Result P.PdfInfo P.PopplerError =
    P.pdfinfo
        (Pdf { content = input })
        P.PdfInfoOptions {};
