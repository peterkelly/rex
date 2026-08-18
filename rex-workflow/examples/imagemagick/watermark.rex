// Workflow: Add a fixed Rex watermark to one CAS-backed image. After applying
// orientation metadata, it draws a translucent black 520x72 rectangle near the
// top-left corner and writes "Generated with Rex" over it in white 32-point
// text, then encodes the result as PNG.
//
// Run from the workspace root. Import the source and a TrueType or OpenType
// font into the workflow store:
//
//   cargo run -p rex-workflow -- --store-path ./store store import photo.jpg
//   cargo run -p rex-workflow -- --store-path ./store store import heading-font.ttf
//
// Create inputs.json with the printed hashes:
//   {"input":"<photo-hash>","font":"<font-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/imagemagick/watermark.rex --inputs inputs.json
//
// On success the ImageOutput contains a single Image whose content field is the
// CAS hash of the watermarked PNG.
import std.artifacts (Image);
import tools.imagemagick as IM;

fn main (input: Hash, font: Hash) -> Result IM.ImageOutput IM.ImageMagickError =
    IM.transform
        (IM.StoredImage (Image { content = input }) IM.AllFrames [])
        [
            IM.AutoOrient,
            IM.Draw
                [
                    IM.DrawFill (IM.Color { value = "rgba(0,0,0,0.55)" }),
                    IM.DrawNoStroke
                ]
                [
                    IM.DrawRectangle
                        (IM.Rectangle { width = 520, height = 72, x = 24, y = 24 })
                ],
            IM.Draw
                [
                    IM.DrawFill (IM.Color { value = "white" }),
                    IM.DrawFont font,
                    IM.DrawPointSize 32.0
                ]
                [
                    IM.DrawText (IM.Point { x = 44.0, y = 70.0 }) "Generated with Rex"
                ]
        ]
        (IM.Encoding {
            format = IM.Format { name = "png" },
            mode = IM.AdjoinFrames,
            options = []
        });
