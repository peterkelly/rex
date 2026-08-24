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
        (IM.ImageSource.Stored
            (Image { content = input })
            IM.FrameSelection.All
            [IM.ReadOption.Background (IM.Color { value = "white" })])
        [
            IM.AutoOrient,
            IM.Alpha IM.AlphaMode.Deactivate,
            IM.ConvertColorspace IM.Colorspace.Srgb
        ]
        (IM.Encoding {
            format = IM.Format.Format { name = "jpeg" },
            mode = IM.OutputMode.Adjoin,
            options = [
                IM.WriteOption.Quality 88,
                IM.WriteOption.SamplingFactor "4:2:0",
                IM.WriteOption.StripMetadata
            ]
        });
