// Workflow: Combine one or more CAS-backed images into an animated GIF. It
// coalesces animation frames, fits each image within 640x640 pixels, assigns an
// 8-centisecond delay, and asks ImageMagick to optimize the frame sequence.
//
// Run from the workspace root. Import each source image with the same store:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import frame-01.png
//
// Put the resulting hashes in display order in inputs.json:
//   {"frames":["<frame-01-hash>","<frame-02-hash>"]}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/imagemagick/animated_gif.rex --inputs inputs.json
//
// On success the result is an ImageOutput containing a single Image whose
// content field is the CAS hash of the encoded, multi-frame GIF.
import std.artifacts (Image);
import tools.imagemagick as IM;

fn read_frame (hash: Hash) -> IM.ImageInstruction =
    IM.ImageInstruction.Read
        (IM.ImageSource.Stored (Image { content = hash }) IM.FrameSelection.All []);

fn main (frames: List Hash) -> Result IM.ImageOutput IM.ImageMagickError =
    let
        reads = map read_frame frames,
        program = reads + [
            IM.ImageInstruction.Sequence IM.CoalesceFrames,
            IM.ImageInstruction.Operation
                (IM.Resize (IM.FitWithin (IM.Size.Size { width = 640, height = 640 }))),
            IM.ImageInstruction.Operation
                (IM.SetProperty "delay" "8"),
            IM.ImageInstruction.Sequence IM.OptimizeFrames
        ]
    in
        IM.render
            program
            (IM.Encoding {
                format = IM.Format.Format { name = "gif" },
                mode = IM.OutputMode.Adjoin,
                options = []
            });
