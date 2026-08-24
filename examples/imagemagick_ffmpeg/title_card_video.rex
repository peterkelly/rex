// Workflow: Generate a branded title card with ImageMagick, then turn it into a
// five-second video with FFmpeg. ImageMagick draws caller-supplied text over a
// 1280x720 navy-to-indigo gradient. FFmpeg loops that PNG at 30 fps, adds a
// generated 261.63-Hz tone, and encodes H.264/AAC MP4.
//
// Run from the workspace root. Import a TrueType or OpenType font:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import heading-font.ttf
//
// Create inputs.json with the printed hash and title to draw:
//   {"font":"<font-hash>","title":"Quarterly results"}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/imagemagick_ffmpeg/title_card_video.rex \
//     --inputs inputs.json
//
// On success the one-element MediaArtifact list contains the generated five-second
// MP4, whose Media content field is its CAS hash. The workflow reports ImageMagick
// and FFmpeg failures distinctly and rejects a non-single ImageMagick output.
import std.artifacts (Image, Media);
import tools.ffmpeg as FF;
import tools.imagemagick as IM;

type WorkflowError
    = FfmpegFailed FF.FfmpegError
    | ImageMagickFailed IM.ImageMagickError
    | UnexpectedImageOutput;

fn encode_title_card (title: String, card: Image)
    -> Result (List FF.MediaArtifact) WorkflowError =
    match FF.render (FF.MediaProgram {
        inputs = [
            FF.MediaInput {
                source = FF.StoredMedia (Media { content = card.content }),
                options = [
                    FF.InputOption.StreamLoop (-1),
                    FF.InputOption.FrameRate (FF.Rational { numerator = 30, denominator = 1 })
                ]
            },
            FF.MediaInput {
                source = FF.SineAudio (FF.SineAudioSource {
                    frequency = 261.63,
                    sample_rate = 48000,
                    duration = Some (FF.Time { seconds = 5.0 })
                }),
                options = []
            }
        ],
        filters = FF.FilterGraph {},
        outputs = [
            FF.MediaOutput {
                format = FF.ContainerFormat { name = "mp4" },
                mode = FF.SingleFile,
                streams = [
                    FF.OutputStream {
                        source = FF.StreamSource.Input
                            (FF.StreamRef {
                                input = 0,
                                kind = FF.MediaKind.Video,
                                index = Some 0
                            }),
                        encoding = FF.StreamEncoding.Video (FF.VideoEncoding {
                            codec = FF.H264,
                            options = [
                                FF.ConstantRateFactor 20.0,
                                FF.Preset "medium",
                                FF.PixelFormat "yuv420p",
                                FF.VideoEncodeOption.FrameRate
                                    (FF.Rational { numerator = 30, denominator = 1 })
                            ]
                        }),
                        metadata = dict_empty,
                        dispositions = [FF.StreamDisposition.Default]
                    },
                    FF.OutputStream {
                        source = FF.StreamSource.Input
                            (FF.StreamRef {
                                input = 1,
                                kind = FF.MediaKind.Audio,
                                index = Some 0
                            }),
                        encoding = FF.StreamEncoding.Audio (FF.AudioEncoding {
                            codec = FF.Aac,
                            options = [FF.AudioEncodeOption.BitRate 128000]
                        }),
                        metadata = dict_empty,
                        dispositions = [FF.StreamDisposition.Default]
                    }
                ],
                options = [
                    FF.MuxOption.Duration (FF.Time { seconds = 5.0 }),
                    FF.MovFlags ["faststart"]
                ],
                metadata = dict_singleton "title" title
            }
        ]
    }) with {
        case Err error -> Err (FfmpegFailed error);
        case Ok artifacts -> Ok artifacts;
    };

fn main (font: Hash, title: String)
    -> Result (List FF.MediaArtifact) WorkflowError =
    match IM.generate
        (IM.LinearGradient
            (IM.Size.Size { width = 1280, height = 720 })
            (IM.Color { value = "#0f172a" })
            (IM.Color { value = "#4338ca" }))
        [
            IM.Draw
                [
                    IM.DrawStyle.Fill (IM.Color { value = "white" }),
                    IM.DrawStyle.Font font,
                    IM.DrawStyle.PointSize 64.0,
                    IM.DrawStyle.Gravity IM.Gravity.Center
                ]
                [IM.DrawingPrimitive.Text (IM.Point.Point { x = 0.0, y = 0.0 }) title]
        ]
        (IM.Encoding {
            format = IM.Format.Format { name = "png" },
            mode = IM.OutputMode.Adjoin,
            options = [IM.WriteOption.StripMetadata]
        })
    with {
        case Err error -> Err (ImageMagickFailed error);
        case Ok output ->
            match output with {
                case IM.ImageOutput.Single card -> encode_title_card title card;
                case IM.ImageOutput.Multiple _ -> Err UnexpectedImageOutput;
            };
    };
