// Extract every embedded image in its most appropriate/native representation.
// Input JSON: {"input":"<pdf-hash>"}
import tools.poppler as P;

fn main (input: Hash) -> Result P.ExtractedImages P.PopplerError =
    P.pdfimages
        (P.Pdf { content = input })
        (P.PdfImagesOptions {
            first_page = None,
            last_page = None,
            format = P.ImagesAll,
            include_page_numbers = true,
            owner_password = None,
            user_password = None
        });
