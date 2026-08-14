// Workflow: Encode the first video stream of one CAS-backed source into two
// video-only deliverables in a single FFmpeg invocation: H.264/yuv420p MP4 at
// CRF 22 and VP9 WebM at CRF 32. Source audio is intentionally omitted.
//
// Run from the workspace root. Import media containing a video stream:
//
//   cargo run -p rex-workflow -- --store-path ./store store import video.mov
//
// Create inputs.json with the printed hash: {"input":"<video-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/ffmpeg/multiple_outputs.rex --inputs inputs.json
//
// On success the result is a two-element list of EncodedMedia artifacts in the
// declared order. Their Media content hashes identify the MP4 and WebM bytes in
// the CAS, respectively.
import artifacts (Media);
import tools.ffmpeg as FF;

fn main (input: Hash) -> Result (List FF.MediaArtifact) FF.FfmpegError =
    FF.render (FF.MediaProgram {
        inputs = [
            FF.MediaInput { source = FF.StoredMedia (Media { content = input }), options = [] }
        ],
        filters = FF.FilterGraph {},
        outputs = [
            FF.MediaOutput {
                format = FF.ContainerFormat { name = "mp4" },
                mode = FF.SingleFile,
                streams = [
                    FF.OutputStream {
                        source = FF.InputStream
                            (FF.StreamRef { input = 0, kind = FF.VideoStream, index = Some 0 }),
                        encoding = FF.EncodeVideo (FF.VideoEncoding {
                            codec = FF.H264,
                            options = [FF.ConstantRateFactor 22.0, FF.PixelFormat "yuv420p"]
                        }),
                        metadata = dict_empty,
                        dispositions = [FF.DefaultDisposition]
                    }
                ],
                options = [FF.MovFlags ["faststart"]],
                metadata = dict_empty
            },
            FF.MediaOutput {
                format = FF.ContainerFormat { name = "webm" },
                mode = FF.SingleFile,
                streams = [
                    FF.OutputStream {
                        source = FF.InputStream
                            (FF.StreamRef { input = 0, kind = FF.VideoStream, index = Some 0 }),
                        encoding = FF.EncodeVideo (FF.VideoEncoding {
                            codec = FF.Vp9,
                            options = [FF.ConstantRateFactor 32.0, FF.VideoBitRate 0]
                        }),
                        metadata = dict_empty,
                        dispositions = []
                    }
                ],
                options = [],
                metadata = dict_empty
            }
        ]
    });
