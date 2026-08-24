// Workflow: Auto-orient one CAS-backed image and scale it down to fit within a
// 1600x1600-pixel box while preserving aspect ratio. The result is written as
// quality-82 WebP with metadata removed.
//
// Run from the workspace root. Import the source into the workflow store:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import photo.jpg
//
// Create inputs.json with the printed hash: {"input":"<photo-hash>"}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/imagemagick/resize.rex --inputs inputs.json
//
// On success the ImageOutput contains a single Image whose content field is the
// CAS hash of the resized WebP image.
import std.artifacts (Image);
import tools.imagemagick as IM;

fn main (input: Hash) -> Result IM.ImageOutput IM.ImageMagickError =
    IM.transform
        (IM.ImageSource.Stored
            (Image { content = input })
            IM.FrameSelection.All
            [])
        [
            IM.AutoOrient,
            IM.Resize (IM.FitWithin (IM.Size.Size { width = 1600, height = 1600 }))
        ]
        (IM.Encoding {
            format = IM.Format.Format { name = "webp" },
            mode = IM.OutputMode.Adjoin,
            options = [IM.WriteOption.Quality 82, IM.WriteOption.StripMetadata]
        });
