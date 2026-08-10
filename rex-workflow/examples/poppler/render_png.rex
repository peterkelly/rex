// Render every selected PDF page as an ordered list of 144-DPI PNG blobs.
// Input JSON: {"input":"<pdf-hash>"}
import tools.poppler as P;

fn main (input: Hash) -> Result P.CairoOutput P.PopplerError =
    P.pdftocairo
        (P.Pdf { content = input })
        P.CairoPng
        (P.PdfToCairoOptions {
            first_page = None,
            last_page = None,
            page_selection = P.AllPages,
            single_file = false,
            resolution = Some 144.0,
            resolution_x = None,
            resolution_y = None,
            scale_to = None,
            scale_to_x = None,
            scale_to_y = None,
            crop = None,
            crop_box = false,
            color = P.CairoColor,
            transparent = false,
            antialias = P.AntialiasDefault,
            jpeg_options = [],
            tiff_compression = None,
            owner_password = None,
            user_password = None
        });
