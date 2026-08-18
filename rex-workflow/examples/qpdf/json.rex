// Export page, outline, and encryption information as QPDF JSON version 2.
// Input JSON: {"input":"<pdf-hash>"}
import std.artifacts (Pdf);
import tools.qpdf as Q;

fn main (input: Hash) -> Result Q.JsonOutput Q.QpdfError =
    Q.json
        (Pdf { content = input })
        None
        Q.JsonOptions {
            keys = [Q.JsonPages, Q.JsonOutlines, Q.JsonEncrypt]
        };
