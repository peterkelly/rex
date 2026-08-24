// Workflow: Composite one CAS-backed video over another. The overlay's first
// video stream is positioned 24 pixels from the background's bottom-right edge;
// output stops with the shorter stream and is encoded as video-only H.264 MP4.
//
// Run from the workspace root. Import both videos into the same store:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import background.mp4
//   cargo run -p rex --bin rex -- --store-path ./store store import overlay.mov
//
// Create inputs.json with the printed hashes:
//   {"background":"<background-hash>","overlay":"<overlay-hash>"}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/ffmpeg/overlay_video.rex --inputs inputs.json
//
// On success the one-element artifact list contains MediaArtifact.Encoded whose Media
// content field is the composited MP4's CAS hash. No audio stream is included.
import std.artifacts (Media);
import tools.ffmpeg as FF;

fn main (background: Hash, overlay: Hash) -> Result (List FF.MediaArtifact) FF.FfmpegError =
    FF.render (FF.MediaProgram {
        inputs = [
            FF.MediaInput { source = FF.StoredMedia (Media { content = background }), options = [] },
            FF.MediaInput { source = FF.StoredMedia (Media { content = overlay }), options = [] }
        ],
        filters = FF.FilterGraph {
            chains = [
                FF.FilterChain {
                    inputs = [
                        FF.FilterPad.Input (FF.StreamRef { input = 0, kind = FF.MediaKind.Video, index = Some 0 }),
                        FF.FilterPad.Input (FF.StreamRef { input = 1, kind = FF.MediaKind.Video, index = Some 0 })
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
                        source = FF.StreamSource.Filter "composited",
                        encoding = FF.StreamEncoding.Video (FF.VideoEncoding {
                            codec = FF.H264,
                            options = [FF.ConstantRateFactor 20.0, FF.PixelFormat "yuv420p"]
                        }),
                        metadata = dict_empty,
                        dispositions = [FF.StreamDisposition.Default]
                    }
                ],
                options = [FF.MovFlags ["faststart"]],
                metadata = dict_empty
            }
        ]
    });
