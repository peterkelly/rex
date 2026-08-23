// Workflow: Extract the top-left 128x128-pixel region of a CAS-backed image as
// raw, interleaved red/green/blue channel bytes using ImageMagick's char-sized
// pixel storage.
//
// Run from the workspace root. Import an image at least 128x128 pixels:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import image.png
//
// Create inputs.json with the printed hash: {"input":"<image-hash>"}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/imagemagick/extract_pixels.rex --inputs inputs.json
//
// On success the PixelBuffer identifies the RGB channels and char storage type.
// Its content field is the CAS hash of the raw byte buffer, rather than an
// encoded image filename; the requested region determines its dimensions.
import std.artifacts (Image);
import tools.imagemagick as IM;

fn main (input: Hash) -> Result IM.PixelBuffer IM.ImageMagickError =
    IM.extract_pixels
        (Image { content = input })
        (IM.PixelSpec {
            region = IM.PixelRectangle
                (IM.Rectangle { width = 128, height = 128, x = 0, y = 0 }),
            channels = [IM.ChannelRed, IM.ChannelGreen, IM.ChannelBlue],
            storage_type = IM.PixelsChar
        });
