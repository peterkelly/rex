// Workflow: Generate five seconds of FFmpeg's testsrc2 pattern at 1280x720 and
// 30 fps. It encodes video-only H.264 at CRF 20 with the medium preset and
// yuv420p pixels, writes streaming-friendly MP4, and adds a title tag.
//
// Run from the workspace root without importing media or supplying inputs:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/ffmpeg/generate_test_video.rex
//
// On success the Media result's content field is the CAS hash of the generated
// five-second MP4 test video.
import tools.ffmpeg as FF;

FF.transcode
    (FF.TestVideo (FF.TestVideoSource {
        pattern = FF.TestSource2,
        size = FF.VideoSize { width = 1280, height = 720 },
        frame_rate = FF.Rational { numerator = 30, denominator = 1 },
        duration = Some (FF.Time { seconds = 5.0 })
    }))
    []
    (FF.Encoding {
        format = FF.ContainerFormat { name = "mp4" },
        video = Some (FF.VideoEncoding {
            codec = FF.H264,
            options = [
                FF.ConstantRateFactor 20.0,
                FF.Preset "medium",
                FF.PixelFormat "yuv420p"
            ]
        }),
        audio = None,
        subtitle = None,
        options = [FF.MovFlags ["faststart"]],
        metadata = dict_singleton "title" "Rex test pattern"
    })
