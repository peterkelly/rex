// Workflow: Composite one CAS-backed video over another. The overlay's first
// video stream is positioned 24 pixels from the background's bottom-right edge;
// output stops with the shorter stream and is encoded as video-only H.264 MP4.
//
// Run from the workspace root. Import both videos into the same store:
//
//   cargo run -p rex-workflow -- --store-path ./store store import background.mp4
//   cargo run -p rex-workflow -- --store-path ./store store import overlay.mov
//
// Create inputs.json with the printed hashes:
//   {"background":"<background-hash>","overlay":"<overlay-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/ffmpeg/overlay_video.rex --inputs inputs.json
//
// On success the one-element artifact list contains EncodedMedia whose Media
// content field is the composited MP4's CAS hash. No audio stream is included.
import tools.ffmpeg as FF;

fn main (background: Hash, overlay: Hash) -> Result (List FF.MediaArtifact) FF.FfmpegError =
    FF.render (FF.MediaProgram {
        inputs = [
            FF.MediaInput { source = FF.StoredMedia (FF.Media { content = background }), options = [] },
            FF.MediaInput { source = FF.StoredMedia (FF.Media { content = overlay }), options = [] }
        ],
        filters = FF.FilterGraph {
            chains = [
                FF.FilterChain {
                    inputs = [
                        FF.InputPad (FF.StreamRef { input = 0, kind = FF.VideoStream, index = Some 0 }),
                        FF.InputPad (FF.StreamRef { input = 1, kind = FF.VideoStream, index = Some 0 })
                    ],
                    filters = [
                        FF.Video (FF.Overlay (FF.OverlayFilter {
                            x = "main_w-overlay_w-24",
                            y = "main_h-overlay_h-24",
                            shortest = true,
                            repeat_last = false
                        }))
                    ],
                    outputs = ["composited"]
                }
            ]
        },
        outputs = [
            FF.MediaOutput {
                format = FF.ContainerFormat { name = "mp4" },
                mode = FF.SingleFile,
                streams = [
                    FF.OutputStream {
                        source = FF.FilterOutput "composited",
                        encoding = FF.EncodeVideo (FF.VideoEncoding {
                            codec = FF.H264,
                            options = [FF.ConstantRateFactor 20.0, FF.PixelFormat "yuv420p"]
                        }),
                        metadata = dict_empty,
                        dispositions = [FF.DefaultDisposition]
                    }
                ],
                options = [FF.MovFlags ["faststart"]],
                metadata = dict_empty
            }
        ]
    });
