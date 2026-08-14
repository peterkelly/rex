// Extract every embedded image in its most appropriate/native representation.
// Input JSON: {"input":"<pdf-hash>"}
import artifacts (Pdf);
import tools.poppler as P;

fn main (input: Hash) -> Result P.ExtractedImages P.PopplerError =
    P.pdfimages
        (Pdf { content = input })
        P.PdfImagesOptions {
            format = P.ImagesAll,
            include_page_numbers = true
        };
