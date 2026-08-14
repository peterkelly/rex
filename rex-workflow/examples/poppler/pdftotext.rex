// Extract word geometry as UTF-8 tab-separated records stored in the CAS.
// Input JSON: {"input":"<pdf-hash>"}
import tools.poppler as P;

fn main (input: Hash) -> Result P.TextFile P.PopplerError =
    P.pdftotext
        (P.Pdf { content = input })
        P.PdfToTextOptions {
            format = P.TabSeparated,
            no_page_breaks = true
        };
