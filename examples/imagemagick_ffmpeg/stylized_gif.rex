// Workflow: Sample four frames from a CAS-backed video with FFmpeg, then use
// ImageMagick to create a compact stylized animated GIF. Frames are extracted at
// 0, 2, 4, and 6 seconds, fitted within 640x640 pixels, converted to high-contrast
// Rec.709-luminance grayscale, assigned a 20-centisecond delay, and optimized.
//
// Run from the workspace root. Import a video at least six seconds long:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import video.mp4
//
// Create inputs.json with the printed hash: {"video":"<video-hash>"}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/imagemagick_ffmpeg/stylized_gif.rex \
//     --inputs inputs.json
//
// On success the ImageOutput contains one Image whose content field is the CAS
// hash of the multi-frame GIF. Failures retain whether FFmpeg frame extraction or
// ImageMagick rendering failed.
import std.artifacts (Image, Media);
import tools.ffmpeg as FF;
import tools.imagemagick as IM;

type WorkflowError
    = FfmpegFailed FF.FfmpegError
    | ImageMagickFailed IM.ImageMagickError;

fn read_frame (frame: Media) -> IM.ImageInstruction =
    IM.ImageInstruction.Read
        (IM.ImageSource.Stored
            (Image { content = frame.content })
            IM.FrameSelection.All
            []);

fn render_gif (frames: List Media) -> Result IM.ImageOutput WorkflowError =
    let
        reads = map read_frame frames,
        program = reads + [
            IM.ImageInstruction.Operation
                (IM.Resize (IM.FitWithin (IM.Size.Size { width = 640, height = 640 }))),
            IM.ImageInstruction.Operation
                (IM.Grayscale IM.IntensityMethod.Rec709Luminance),
            IM.ImageInstruction.Operation (IM.SigmoidalContrast IM.Enabled 5.0 50.0),
            IM.ImageInstruction.Operation (IM.SetProperty "delay" "20"),
            IM.ImageInstruction.Sequence IM.OptimizeFrames
        ]
    in
        match IM.render
            program
            (IM.Encoding {
                format = IM.Format.Format { name = "gif" },
                mode = IM.OutputMode.Adjoin,
                options = [IM.WriteOption.StripMetadata]
            })
        with {
            case Err error -> Err (ImageMagickFailed error);
            case Ok animation -> Ok animation;
        };

fn main (video: Hash) -> Result IM.ImageOutput WorkflowError =
    match FF.extract_frames
        (Media { content = video })
        (FF.AtTimes [
            FF.Time { seconds = 0.0 },
            FF.Time { seconds = 2.0 },
            FF.Time { seconds = 4.0 },
            FF.Time { seconds = 6.0 }
        ])
        (FF.ImageEncoding {
            format = FF.ContainerFormat { name = "png" },
            video = FF.VideoEncoding { codec = FF.VideoCodec.Png, options = [] }
        })
    with {
        case Err error -> Err (FfmpegFailed error);
        case Ok frames -> render_gif frames;
    };
