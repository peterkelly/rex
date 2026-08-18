// Workflow: Compare an expected image with an actual image using ImageMagick's
// structural-similarity metric (SSIM), allowing a 1% pixel fuzz tolerance. A
// visual difference image is retained as part of the comparison result.
//
// Run from the workspace root. Import both files into the same store:
//
//   cargo run -p rex-workflow -- --store-path ./store store import expected.png
//   cargo run -p rex-workflow -- --store-path ./store store import actual.png
//
// Create inputs.json with the two printed hashes:
//   {"expected":"<expected-hash>","actual":"<actual-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/imagemagick/compare_images.rex --inputs inputs.json
//
// On success the Comparison reports whether the images are equal, the numeric
// distortion, and an Image whose content field is the CAS hash of the rendered
// difference image.
import std.artifacts (Image);
import tools.imagemagick as IM;

fn main (expected: Hash) -> (actual: Hash) -> Result IM.Comparison IM.ImageMagickError =
    IM.compare
        (Image { content = expected })
        (Image { content = actual })
        IM.MetricStructuralSimilarity
        [IM.CompareFuzz "1%"];
