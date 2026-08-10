// Workflow: Read media directly from a network URL, keep at most its first 20
// seconds, and transcode it to streaming-friendly MP4 using veryfast H.264 and
// 128-kbit/s AAC. Unlike stored-media examples, its input is not a CAS hash.
//
// Run from the workspace root. Create inputs.json with a URL supported by this
// FFmpeg build, for example {"url":"https://example.test/media/input.m3u8"},
// then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/ffmpeg/network_input.rex --inputs inputs.json
//
// The host must allow network access, and the server/protocol may require its own
// credentials or options. On success the Media content field is the resulting
// MP4's CAS hash.
import tools.ffmpeg as FF;

fn main (url: String) -> Result FF.Media FF.FfmpegError =
    FF.transcode
        (FF.NetworkMedia url)
        [FF.Trim (FF.TimeRange { start = None, duration = Some (FF.Time { seconds = 20.0 }) })]
        (FF.Encoding {
            format = FF.ContainerFormat { name = "mp4" },
            video = Some (FF.VideoEncoding { codec = FF.H264, options = [FF.Preset "veryfast"] }),
            audio = Some (FF.AudioEncoding { codec = FF.Aac, options = [FF.AudioBitRate 128000] }),
            subtitle = None,
            options = [FF.MovFlags ["faststart"]],
            metadata = dict_empty
        });
