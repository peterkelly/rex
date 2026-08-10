// Workflow: Place a logo over a background image using normal alpha compositing.
// The logo is aligned to the bottom-right corner with a 32-pixel inset, and the
// composed result is encoded as PNG.
//
// Run from the workspace root. Import the background and logo into one store:
//
//   cargo run -p rex-workflow -- --store-path ./store store import background.jpg
//   cargo run -p rex-workflow -- --store-path ./store store import logo.png
//
// Create inputs.json with the hashes printed by the import commands:
//   {"background":"<background-hash>","logo":"<logo-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/imagemagick/composite_logo.rex --inputs inputs.json
//
// On success the ImageOutput contains a single Image whose content field is the
// CAS hash of the composited PNG.
import tools.imagemagick as IM;

fn main (background: Hash) -> (logo: Hash) -> Result IM.ImageOutput IM.ImageMagickError =
    IM.composite
        (IM.Image { content = background })
        (IM.Image { content = logo })
        None
        IM.ComposeOver
        [
            IM.CompositeGravity IM.GravitySouthEast,
            IM.CompositeGeometry
                (IM.Rectangle { width = 0, height = 0, x = 32, y = 32 })
        ]
        (IM.Encoding {
            format = IM.Format { name = "png" },
            mode = IM.AdjoinFrames,
            options = []
        });
