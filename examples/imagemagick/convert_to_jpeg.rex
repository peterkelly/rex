// Workflow: Convert one CAS-backed image to a broadly compatible JPEG. It
// applies orientation metadata, removes alpha against a white background,
// converts to sRGB, strips metadata, and writes quality 88 with 4:2:0 sampling.
//
// Run from the workspace root. Import the source into the workflow store:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import source.png
//
// Create inputs.json using the hash printed by the import command:
//   {"input":"<source-hash>"}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/imagemagick/convert_to_jpeg.rex --inputs inputs.json
//
// On success the ImageOutput contains a single Image whose content field is the
// CAS hash of the converted JPEG.
import std.artifacts (Image);
import tools.imagemagick as IM;

fn main (input: Hash) -> Result IM.ImageOutput IM.ImageMagickError =
    IM.transform
        (IM.StoredImage
            (Image { content = input })
            IM.AllFrames
            [IM.ReadBackground (IM.Color { value = "white" })])
        [
            IM.AutoOrient,
            IM.Alpha IM.AlphaDeactivate,
            IM.ConvertColorspace IM.ColorspaceSrgb
        ]
        (IM.Encoding {
            format = IM.Format { name = "jpeg" },
            mode = IM.AdjoinFrames,
            options = [
                IM.WriteQuality 88,
                IM.WriteSamplingFactor "4:2:0",
                IM.WriteStripMetadata
            ]
        });
