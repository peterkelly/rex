// Workflow: Convert one CAS-backed image into a cleaned binary mask. It changes
// the image to grayscale using Rec.709 luminance, thresholds at 50%, then
// performs opening and closing with a radius-2 disk kernel before encoding an
// 8-bit PNG.
//
// Run from the workspace root. Import the source into the workflow store:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import mask-source.png
//
// Create inputs.json with the printed hash: {"input":"<source-hash>"}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/imagemagick/morphology.rex --inputs inputs.json
//
// On success the ImageOutput contains a single Image whose content field is the
// CAS hash of the morphologically cleaned PNG mask.
import std.artifacts (Image);
import tools.imagemagick as IM;

fn main (input: Hash) -> Result IM.ImageOutput IM.ImageMagickError =
    IM.transform
        (IM.ImageSource.Stored (Image { content = input }) IM.FrameSelection.All [])
        [
            IM.Grayscale IM.IntensityMethod.Rec709Luminance,
            IM.Threshold "50%",
            IM.Morphology IM.MorphologyMethod.Open "Disk:2",
            IM.Morphology IM.MorphologyMethod.Close "Disk:2"
        ]
        (IM.Encoding {
            format = IM.Format.Format { name = "png" },
            mode = IM.OutputMode.Adjoin,
            options = [IM.WriteOption.Depth 8]
        });
