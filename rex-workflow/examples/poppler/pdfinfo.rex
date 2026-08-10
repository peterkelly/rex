// Parse document metadata and page boxes with pdfinfo -box -isodates.
// Input JSON: {"input":"<pdf-hash>"}
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
