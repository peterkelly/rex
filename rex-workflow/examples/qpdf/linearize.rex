// Rewrite a PDF for fast web view while making generated IDs reproducible.
// Input JSON: {"input":"<pdf-hash>"}
import tools.qpdf as Q;

fn main (input: Hash) -> Result Q.PdfOutput Q.QpdfError =
    Q.transform
        (Q.Pdf { content = input })
        None
        [Q.Linearize, Q.DeterministicId];
