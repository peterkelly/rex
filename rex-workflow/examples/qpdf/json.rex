// Export page, outline, and encryption information as QPDF JSON version 2.
// Input JSON: {"input":"<pdf-hash>"}
import tools.qpdf as Q;

fn main (input: Hash) -> Result Q.JsonOutput Q.QpdfError =
    Q.json
        (Q.Pdf { content = input })
        None
        (Q.JsonOptions {
            keys = [Q.JsonPages, Q.JsonOutlines, Q.JsonEncrypt],
            objects = [],
            stream_data = Q.JsonStreamDataNone,
            decode_level = Q.DecodeGeneralized
        });
