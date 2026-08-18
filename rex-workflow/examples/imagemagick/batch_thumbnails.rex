// Workflow: Turn a batch of CAS-backed images into independent WebP
// thumbnails. Every input is auto-oriented, scaled to fit within 320x320
// pixels without changing its aspect ratio, stripped of metadata, and encoded
// at quality 80.
//
// Run from the workspace root. Import each source using the same store:
//
//   cargo run -p rex-workflow -- --store-path ./store store import photo.jpg
//
// Create inputs.json from the printed hashes:
//   {"inputs":["<first-photo-hash>","<second-photo-hash>"]}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/imagemagick/batch_thumbnails.rex --inputs inputs.json
//
// On success the result is a list of Images in input order. Each Image's
// content field is the CAS hash of one newly encoded WebP thumbnail.
import std.artifacts (Image);
import tools.imagemagick as IM;

fn to_image (hash: Hash) -> Image =
    Image { content = hash };

fn main (inputs: List Hash) -> Result (List Image) IM.ImageMagickError =
    IM.transform_many
        (map to_image inputs)
        [
            IM.AutoOrient,
            IM.Resize (IM.FitWithin (IM.Size { width = 320, height = 320 })),
            IM.StripMetadata
        ]
        (IM.Encoding {
            format = IM.Format { name = "webp" },
            mode = IM.AdjoinFrames,
            options = [IM.WriteQuality 80]
        });
