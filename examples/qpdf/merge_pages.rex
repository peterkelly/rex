// Merge every page from two PDFs using qpdf --pages and an empty primary PDF.
// Input JSON: {"first":"<pdf-hash>","second":"<pdf-hash>"}
import std.artifacts (Pdf);
import tools.qpdf as Q;

fn main (first: Hash) -> (second: Hash) -> Result Q.PdfOutput Q.QpdfError =
    Q.pages
        None
        None
        [
            Q.PageSource {
                pdf = Pdf { content = first },
                range = "1-z",
                password = None
            },
            Q.PageSource {
                pdf = Pdf { content = second },
                range = "1-z",
                password = None
            }
        ]
        None
        [Q.ObjectStreams Q.ObjectStreamsGenerate];
