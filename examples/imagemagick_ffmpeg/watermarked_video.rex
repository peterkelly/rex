// Workflow: Prepare a CAS-backed watermark with ImageMagick and overlay it on a
// CAS-backed video with FFmpeg. The watermark is fitted inside 240x120 pixels,
// retained as transparent PNG, and placed 32 pixels from the video's lower-right
// edge. Video is encoded as H.264 and the source's first audio stream as AAC.
//
// Run from the workspace root. Import a video with video and audio streams and a
// watermark image, using the same store for both commands:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import video.mp4
//   cargo run -p rex --bin rex -- --store-path ./store store import watermark.png
//
// Create inputs.json with the printed hashes:
//   {"video":"<video-hash>","watermark":"<watermark-hash>"}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/imagemagick_ffmpeg/watermarked_video.rex \
//     --inputs inputs.json
//
// On success the one-element MediaArtifact list contains the streaming-friendly
// MP4 with composited video and encoded audio; its Media content field is the CAS
// hash. Image output-shape errors and failures from either tool use WorkflowError.
import std.artifacts (Image, Media);
import tools.ffmpeg as FF;
import tools.imagemagick as IM;

type WorkflowError
    = FfmpegFailed FF.FfmpegError
    | ImageMagickFailed IM.ImageMagickError
    | UnexpectedImageOutput;

fn overlay_watermark (video: Hash, watermark: Image)
    -> Result (List FF.MediaArtifact) WorkflowError =
    match FF.render (FF.MediaProgram {
        inputs = [
            FF.MediaInput {
                source = FF.StoredMedia (Media { content = video }),
                options = []
            },
            FF.MediaInput {
                source = FF.StoredMedia (Media { content = watermark.content }),
                options = []
            }
        ],
        filters = FF.FilterGraph {
            chains = [
                FF.FilterChain {
                    inputs = [
                        FF.FilterPad.Input
                            (FF.StreamRef {
                                input = 0,
                                kind = FF.MediaKind.Video,
                                index = Some 0
                            }),
                        FF.FilterPad.Input
                            (FF.StreamRef {
                                input = 1,
                                kind = FF.MediaKind.Video,
                                index = Some 0
                            })
                    ],
                    filters = [
                        FF.Video
                            (FF.Overlay (FF.OverlayFilter {
                                x = "main_w-overlay_w-32",
                                y = "main_h-overlay_h-32",
                                shortest = false,
                                repeat_last = true
                            }))
                    ],
                    outputs = ["watermarked"]
                }
            ]
        },
        outputs = [
            FF.MediaOutput {
                format = FF.ContainerFormat { name = "mp4" },
                mode = FF.SingleFile,
                streams = [
                    FF.OutputStream {
                        source = FF.StreamSource.Filter "watermarked",
                        encoding = FF.StreamEncoding.Video (FF.VideoEncoding {
                            codec = FF.H264,
                            options = [
                                FF.ConstantRateFactor 21.0,
                                FF.Preset "medium",
                                FF.PixelFormat "yuv420p"
                            ]
                        }),
                        metadata = dict_empty,
                        dispositions = [FF.StreamDisposition.Default]
                    },
                    FF.OutputStream {
                        source = FF.StreamSource.Input
                            (FF.StreamRef {
                                input = 0,
                                kind = FF.MediaKind.Audio,
                                index = Some 0
                            }),
                        encoding = FF.StreamEncoding.Audio (FF.AudioEncoding {
                            codec = FF.Aac,
                            options = [FF.AudioEncodeOption.BitRate 160000]
                        }),
                        metadata = dict_empty,
                        dispositions = [FF.StreamDisposition.Default]
                    }
                ],
                options = [FF.MovFlags ["faststart"]],
                metadata = dict_empty
            }
        ]
    }) with {
        case Err error -> Err (FfmpegFailed error);
        case Ok artifacts -> Ok artifacts;
    };

fn main (video: Hash, watermark: Hash)
    -> Result (List FF.MediaArtifact) WorkflowError =
    match IM.transform
        (IM.ImageSource.Stored
            (Image { content = watermark })
            IM.FrameSelection.All
            [])
        [
            IM.Resize (IM.FitWithin (IM.Size.Size { width = 240, height = 120 })),
            IM.StripMetadata
        ]
        (IM.Encoding {
            format = IM.Format.Format { name = "png" },
            mode = IM.OutputMode.Adjoin,
            options = []
        })
    with {
        case Err error -> Err (ImageMagickFailed error);
        case Ok output ->
            match output with {
                case IM.ImageOutput.Single image -> overlay_watermark video image;
                case IM.ImageOutput.Multiple _ -> Err UnexpectedImageOutput;
            };
    };
