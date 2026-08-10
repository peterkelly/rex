// Check one CAS-backed PDF using qpdf --check.
// Input JSON: {"input":"<pdf-hash>"}
import tools.qpdf as Q;

fn main (input: Hash) -> Result Q.CheckReport Q.QpdfError =
    Q.check (Q.Pdf { content = input }) None;
