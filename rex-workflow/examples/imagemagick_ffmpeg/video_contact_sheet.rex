// Workflow: Sample four frames from a CAS-backed video with FFmpeg and assemble
// them into a single contact sheet with ImageMagick. Frames are taken at 0, 5,
// 10, and 15 seconds, encoded as PNG, and arranged in a four-column JPEG sheet
// with 320x180 cells, a dark background, borders, and shadows.
//
// Run from the workspace root. Import a video at least 15 seconds long and a
// TrueType or OpenType font:
//
//   cargo run -p rex-workflow -- --store-path ./store store import video.mp4
//   cargo run -p rex-workflow -- --store-path ./store store import heading-font.ttf
//
// Create inputs.json with the printed hashes:
//   {"video":"<video-hash>","font":"<font-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/imagemagick_ffmpeg/video_contact_sheet.rex \
//     --inputs inputs.json
//
// On success the ImageOutput contains one Image whose content field is the CAS
// hash of the JPEG contact sheet. FFmpeg and ImageMagick failures are wrapped in
// the workflow's FfmpegFailed and ImageMagickFailed error constructors.
import std.artifacts (Image, Media);
import tools.ffmpeg as FF;
import tools.imagemagick as IM;

type WorkflowError
    = FfmpegFailed FF.FfmpegError
    | ImageMagickFailed IM.ImageMagickError;

fn frame_to_image (frame: Media) -> Image =
    Image { content = frame.content };

fn main (video: Hash, font: Hash) -> Result IM.ImageOutput WorkflowError =
    match FF.extract_frames
        (Media { content = video })
        (FF.AtTimes [
            FF.Time { seconds = 0.0 },
            FF.Time { seconds = 5.0 },
            FF.Time { seconds = 10.0 },
            FF.Time { seconds = 15.0 }
        ])
        (FF.ImageEncoding {
            format = FF.ContainerFormat { name = "png" },
            video = FF.VideoEncoding { codec = FF.PngVideo, options = [] }
        })
    with {
        case Err error -> Err (FfmpegFailed error);
        case Ok frames ->
            match IM.montage
                (map frame_to_image frames)
                (IM.MontageColumns 4)
                [
                    IM.MontageGeometry "320x180+12+12",
                    IM.MontageBackground (IM.Color { value = "#111827" }),
                    IM.MontageBorder 1 (IM.Color { value = "#4b5563" }),
                    IM.MontageShadow,
                    IM.MontageFont font
                ]
                (IM.Encoding {
                    format = IM.Format { name = "jpeg" },
                    mode = IM.AdjoinFrames,
                    options = [IM.WriteQuality 90, IM.WriteStripMetadata]
                })
            with {
                case Err error -> Err (ImageMagickFailed error);
                case Ok sheet -> Ok sheet;
            };
    };
