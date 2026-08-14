// Check one CAS-backed PDF using qpdf --check.
// Input JSON: {"input":"<pdf-hash>"}
import artifacts (Pdf);
import tools.qpdf as Q;

fn main (input: Hash) -> Result Q.CheckReport Q.QpdfError =
    Q.check (Pdf { content = input }) None;
