// Workflow: Create a square 256x256 PNG thumbnail from one CAS-backed image. It
// auto-orients the source, scales it to fill the target, centers the result in a
// transparent 256-pixel extent, and strips metadata.
//
// Run from the workspace root. Import the source into the workflow store:
//
//   cargo run -p rex-workflow -- --store-path ./store store import photo.jpg
//
// Create inputs.json with the printed hash: {"input":"<photo-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/imagemagick/square_thumbnail.rex --inputs inputs.json
//
// On success the ImageOutput contains a single Image whose content field is the
// CAS hash of the 256x256 PNG thumbnail.
import std.artifacts (Image);
import tools.imagemagick as IM;

fn main (input: Hash) -> Result IM.ImageOutput IM.ImageMagickError =
    IM.transform
        (IM.StoredImage
            (Image { content = input })
            IM.AllFrames
            [])
        [
            IM.AutoOrient,
            IM.Resize (IM.FillArea (IM.Size { width = 256, height = 256 })),
            IM.Extent
                (IM.Rectangle { width = 256, height = 256, x = 0, y = 0 })
                IM.GravityCenter
                (IM.Color { value = "transparent" })
        ]
        (IM.Encoding {
            format = IM.Format { name = "png" },
            mode = IM.AdjoinFrames,
            options = [IM.WriteStripMetadata]
        });
