// Workflow: Assemble CAS-backed images into a four-column JPEG contact sheet.
// Each cell uses 240x180 geometry with 12-pixel spacing, a dark background and
// border, and a drop shadow; the final sheet is encoded at quality 90.
//
// Run from the workspace root. Import each image and a TrueType or OpenType
// font into the same store:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import photo-01.jpg
//   cargo run -p rex --bin rex -- --store-path ./store store import heading-font.ttf
//
// Put the printed hashes in the desired contact-sheet order in inputs.json:
//   {"inputs":["<photo-01-hash>","<photo-02-hash>"],"font":"<font-hash>"}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/imagemagick/contact_sheet.rex --inputs inputs.json
//
// On success the ImageOutput contains a single Image whose content field is the
// CAS hash of the complete JPEG contact sheet.
import std.artifacts (Image);
import tools.imagemagick as IM;

fn to_image (hash: Hash) -> Image =
    Image { content = hash };

fn main (inputs: List Hash, font: Hash) -> Result IM.ImageOutput IM.ImageMagickError =
    IM.montage
        (map to_image inputs)
        (IM.MontageLayout.Columns 4)
        [
            IM.MontageOption.Geometry "240x180+12+12",
            IM.MontageOption.Background (IM.Color { value = "#18181b" }),
            IM.MontageOption.Border 1 (IM.Color { value = "#3f3f46" }),
            IM.MontageOption.Shadow,
            IM.MontageOption.Font font
        ]
        (IM.Encoding {
            format = IM.Format.Format { name = "jpeg" },
            mode = IM.OutputMode.Adjoin,
            options = [IM.WriteOption.Quality 90]
        });
