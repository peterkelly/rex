// Workflow: Combine one or more CAS-backed images into an animated GIF. It
// coalesces animation frames, fits each image within 640x640 pixels, assigns an
// 8-centisecond delay, and asks ImageMagick to optimize the frame sequence.
//
// Run from the workspace root. Import each source image with the same store:
//
//   cargo run -p rex-workflow -- --store-path ./store store import frame-01.png
//
// Put the resulting hashes in display order in inputs.json:
//   {"frames":["<frame-01-hash>","<frame-02-hash>"]}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/imagemagick/animated_gif.rex --inputs inputs.json
//
// On success the result is an ImageOutput containing a single Image whose
// content field is the CAS hash of the encoded, multi-frame GIF.
import tools.imagemagick as IM;

fn read_frame (hash: Hash) -> IM.ImageInstruction =
    IM.ReadImage
        (IM.StoredImage (IM.Image { content = hash }) IM.AllFrames []);

fn main (frames: List Hash) -> Result IM.ImageOutput IM.ImageMagickError =
    let
        reads = map read_frame frames,
        program = reads + [
            IM.ApplySequenceOperation IM.CoalesceFrames,
            IM.ApplyImageOperation
                (IM.Resize (IM.FitWithin (IM.Size { width = 640, height = 640 }))),
            IM.ApplyImageOperation
                (IM.SetProperty "delay" "8"),
            IM.ApplySequenceOperation IM.OptimizeFrames
        ]
    in
        IM.render
            program
            (IM.Encoding {
                format = IM.Format { name = "gif" },
                mode = IM.AdjoinFrames,
                options = []
            });
