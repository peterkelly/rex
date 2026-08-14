// Workflow: Produce a high-contrast grayscale version of one CAS-backed image.
// It auto-orients the source, converts to gray with Rec.709 luminance, applies
// sigmoidal contrast and light sharpening, and emits an 8-bit PNG.
//
// Run from the workspace root. Import the source into the workflow store:
//
//   cargo run -p rex-workflow -- --store-path ./store store import photo.jpg
//
// Create inputs.json with the printed hash: {"input":"<photo-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/imagemagick/grayscale_and_contrast.rex \
//     --inputs inputs.json
//
// On success the ImageOutput contains a single Image whose content field is the
// CAS hash of the processed grayscale PNG.
import artifacts (Image);
import tools.imagemagick as IM;

fn main (input: Hash) -> Result IM.ImageOutput IM.ImageMagickError =
    IM.transform
        (IM.StoredImage (Image { content = input }) IM.AllFrames [])
        [
            IM.AutoOrient,
            IM.Grayscale IM.IntensityRec709Luminance,
            IM.SigmoidalContrast IM.Enabled 6.0 50.0,
            IM.Sharpen (IM.BlurGeometry { radius = 0.0, sigma = 0.8 })
        ]
        (IM.Encoding {
            format = IM.Format { name = "png" },
            mode = IM.AdjoinFrames,
            options = [IM.WriteDepth 8]
        });
