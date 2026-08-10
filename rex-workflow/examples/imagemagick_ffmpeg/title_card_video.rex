// Workflow: Generate a branded title card with ImageMagick, then turn it into a
// five-second video with FFmpeg. ImageMagick draws caller-supplied text over a
// 1280x720 navy-to-indigo gradient. FFmpeg loops that PNG at 30 fps, adds a
// generated 261.63-Hz tone, and encodes H.264/AAC MP4.
//
// Run from the workspace root. Import a TrueType or OpenType font:
//
//   cargo run -p rex-workflow -- --store-path ./store store import heading-font.ttf
//
// Create inputs.json with the printed hash and title to draw:
//   {"font":"<font-hash>","title":"Quarterly results"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/imagemagick_ffmpeg/title_card_video.rex \
//     --inputs inputs.json
//
// On success the one-element MediaArtifact list contains the generated five-second
// MP4, whose Media content field is its CAS hash. The workflow reports ImageMagick
// and FFmpeg failures distinctly and rejects a non-single ImageMagick output.
import tools.ffmpeg as FF;
import tools.imagemagick as IM;
import tools.imagemagick (SingleImage, MultipleImages);

type WorkflowError
    = FfmpegFailed FF.FfmpegError
    | ImageMagickFailed IM.ImageMagickError
    | UnexpectedImageOutput;

fn encode_title_card (title: String, card: IM.Image)
    -> Result (List FF.MediaArtifact) WorkflowError =
    match FF.render (FF.MediaProgram {
        inputs = [
            FF.MediaInput {
                source = FF.StoredMedia (FF.Media { content = card.content }),
                options = [
                    FF.InputStreamLoop (-1),
                    FF.InputFrameRate (FF.Rational { numerator = 30, denominator = 1 })
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
        filters = FF.FilterGraph { chains = [] },
        outputs = [
            FF.MediaOutput {
                format = FF.ContainerFormat { name = "mp4" },
                mode = FF.SingleFile,
                streams = [
                    FF.OutputStream {
                        source = FF.InputStream
                            (FF.StreamRef {
                                input = 0,
                                kind = FF.VideoStream,
                                index = Some 0
                            }),
                        encoding = FF.EncodeVideo (FF.VideoEncoding {
                            codec = FF.H264,
                            options = [
                                FF.ConstantRateFactor 20.0,
                                FF.Preset "medium",
                                FF.PixelFormat "yuv420p",
                                FF.VideoFrameRate
                                    (FF.Rational { numerator = 30, denominator = 1 })
                            ]
                        }),
                        metadata = dict_empty,
                        dispositions = [FF.DefaultDisposition]
                    },
                    FF.OutputStream {
                        source = FF.InputStream
                            (FF.StreamRef {
                                input = 1,
                                kind = FF.AudioStream,
                                index = Some 0
                            }),
                        encoding = FF.EncodeAudio (FF.AudioEncoding {
                            codec = FF.Aac,
                            options = [FF.AudioBitRate 128000]
                        }),
                        metadata = dict_empty,
                        dispositions = [FF.DefaultDisposition]
                    }
                ],
                options = [
                    FF.OutputDuration (FF.Time { seconds = 5.0 }),
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
            (IM.Size { width = 1280, height = 720 })
            (IM.Color { value = "#0f172a" })
            (IM.Color { value = "#4338ca" }))
        [
            IM.Draw
                [
                    IM.DrawFill (IM.Color { value = "white" }),
                    IM.DrawFont font,
                    IM.DrawPointSize 64.0,
                    IM.DrawGravity IM.GravityCenter
                ]
                [IM.DrawText (IM.Point { x = 0.0, y = 0.0 }) title]
        ]
        (IM.Encoding {
            format = IM.Format { name = "png" },
            mode = IM.AdjoinFrames,
            options = [IM.WriteStripMetadata]
        })
    with {
        case Err error -> Err (ImageMagickFailed error);
        case Ok output ->
            match output with {
                case SingleImage card -> encode_title_card title card;
                case MultipleImages _ -> Err UnexpectedImageOutput;
            };
    };
