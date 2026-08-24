// Workflow: Ask ImageMagick to identify a CAS-backed image using ping mode, so
// it reads image metadata without fully decoding pixel data where the format
// permits. Multi-frame inputs produce one metadata record per frame.
//
// Run from the workspace root. Import the image into the workflow store:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import image.gif
//
// Create inputs.json with the printed hash: {"input":"<image-hash>"}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/imagemagick/identify.rex --inputs inputs.json
//
// On success the result is an ordered list of ImageInfo records describing such
// properties as format, dimensions, colorspace, depth, frame, and byte size.
// No new image content is stored.
import std.artifacts (Image);
import tools.imagemagick as IM;

fn main (input: Hash) -> Result (List IM.ImageInfo) IM.ImageMagickError =
    IM.identify
        (Image { content = input })
        [IM.IdentifyOption.Ping];
