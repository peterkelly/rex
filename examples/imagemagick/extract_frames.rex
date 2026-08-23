// Workflow: Decode every frame of a CAS-backed animation, apply orientation
// metadata, and encode each frame separately as a metadata-free PNG.
//
// Run from the workspace root. Import the animated source into the store:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import animation.gif
//
// Create inputs.json using the printed hash:
//   {"animation":"<animation-hash>"}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/imagemagick/extract_frames.rex --inputs inputs.json
//
// On success the ImageOutput is MultipleImages. Its ordered image list has one
// entry per decoded frame, and each content field is that PNG frame's CAS hash.
import std.artifacts (Image);
import tools.imagemagick as IM;

fn main (animation: Hash) -> Result IM.ImageOutput IM.ImageMagickError =
    IM.transform
        (IM.StoredImage
            (Image { content = animation })
            IM.AllFrames
            [])
        [IM.AutoOrient]
        (IM.Encoding {
            format = IM.Format { name = "png" },
            mode = IM.SeparateFrames,
            options = [IM.WriteStripMetadata]
        });
