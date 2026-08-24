// Workflow: Extract a representative video frame with FFmpeg and turn it into a
// polished 1280x720 PNG with ImageMagick. The frame at 10 seconds is scaled and
// center-cropped, given extra contrast and sharpening, and finished with a dark
// footer containing caller-supplied title text.
//
// Run from the workspace root. Import a video at least 10 seconds long and a
// TrueType or OpenType font, using the same store for both commands:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import video.mp4
//   cargo run -p rex --bin rex -- --store-path ./store store import heading-font.ttf
//
// Create inputs.json with both hashes and the desired title:
//   {"video":"<video-hash>","font":"<font-hash>","title":"A video title"}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/imagemagick_ffmpeg/polished_thumbnail.rex \
//     --inputs inputs.json
//
// On success the ImageOutput contains one Image whose content field is the CAS
// hash of the finished PNG thumbnail. Either tool's expected process failure is
// preserved inside the corresponding workflow error constructor.
import std.artifacts (Image, Media);
import tools.ffmpeg as FF;
import tools.imagemagick as IM;

type WorkflowError
    = FfmpegFailed FF.FfmpegError
    | ImageMagickFailed IM.ImageMagickError;

fn main (video: Hash, font: Hash, title: String)
    -> Result IM.ImageOutput WorkflowError =
    match FF.thumbnail
        (Media { content = video })
        (FF.ThumbnailSpec {
            at = Some (FF.Time { seconds = 10.0 }),
            size = None,
            preserve_aspect_ratio = true
        })
        (FF.ImageEncoding {
            format = FF.ContainerFormat { name = "png" },
            video = FF.VideoEncoding { codec = FF.VideoCodec.Png, options = [] }
        })
    with {
        case Err error -> Err (FfmpegFailed error);
        case Ok frame ->
            match IM.transform
                (IM.ImageSource.Stored
                    (Image { content = frame.content })
                    IM.FrameSelection.All
                    [])
                [
                    IM.Resize (IM.FillArea (IM.Size.Size { width = 1280, height = 720 })),
                    IM.Extent
                        (IM.Rectangle.Rectangle { width = 1280, height = 720, x = 0, y = 0 })
                        IM.Gravity.Center
                        (IM.Color { value = "black" }),
                    IM.SigmoidalContrast IM.Enabled 4.0 50.0,
                    IM.Sharpen (IM.BlurGeometry { radius = 0.0, sigma = 0.7 }),
                    IM.Draw
                        [
                            IM.DrawStyle.Fill (IM.Color { value = "rgba(0,0,0,0.68)" }),
                            IM.DrawStyle.NoStroke
                        ]
                        [
                            IM.DrawingPrimitive.Rectangle
                                (IM.Rectangle.Rectangle { width = 1280, height = 104, x = 0, y = 616 })
                        ],
                    IM.Draw
                        [
                            IM.DrawStyle.Fill (IM.Color { value = "white" }),
                            IM.DrawStyle.Font font,
                            IM.DrawStyle.PointSize 40.0,
                            IM.DrawStyle.Gravity IM.Gravity.NorthWest
                        ]
                        [IM.DrawingPrimitive.Text (IM.Point.Point { x = 48.0, y = 682.0 }) title]
                ]
                (IM.Encoding {
                    format = IM.Format.Format { name = "png" },
                    mode = IM.OutputMode.Adjoin,
                    options = [IM.WriteOption.StripMetadata]
                })
            with {
                case Err error -> Err (ImageMagickFailed error);
                case Ok thumbnail -> Ok thumbnail;
            };
    };
