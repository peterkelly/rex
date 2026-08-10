// Extract word geometry as UTF-8 tab-separated records stored in the CAS.
// Input JSON: {"input":"<pdf-hash>"}
import tools.poppler as P;

fn main (input: Hash) -> Result P.TextFile P.PopplerError =
    P.pdftotext
        (P.Pdf { content = input })
        (P.PdfToTextOptions {
            first_page = None,
            last_page = None,
            format = P.TabSeparated,
            resolution = None,
            crop = None,
            crop_box = false,
            discard_diagonal_text = false,
            column_spacing = None,
            end_of_line = P.EolUnix,
            no_page_breaks = true,
            owner_password = None,
            user_password = None
        });
