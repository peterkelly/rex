// Workflow: Transcode CAS-backed media into a compatible, streaming-friendly
// MP4. Video uses H.264 at CRF 22 with the slow preset and yuv420p pixels; audio
// uses 192-kbit/s AAC.
//
// Run from the workspace root. Import media containing video and audio:
//
//   cargo run -p rex-workflow -- --store-path ./store store import source.mkv
//
// Create inputs.json with the printed hash: {"input":"<media-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/ffmpeg/transcode_h264.rex --inputs inputs.json
//
// On success the Media result's content field is the CAS hash of the encoded MP4.
import tools.ffmpeg as FF;

fn main (input: Hash) -> Result FF.Media FF.FfmpegError =
    FF.transcode
        (FF.StoredMedia (FF.Media { content = input }))
        []
        (FF.Encoding {
            format = FF.ContainerFormat { name = "mp4" },
            video = Some (FF.VideoEncoding {
                codec = FF.H264,
                options = [
                    FF.ConstantRateFactor 22.0,
                    FF.Preset "slow",
                    FF.PixelFormat "yuv420p"
                ]
            }),
            audio = Some (FF.AudioEncoding {
                codec = FF.Aac,
                options = [FF.AudioBitRate 192000]
            }),
            subtitle = None,
            options = [FF.MovFlags ["faststart"]],
            metadata = dict_empty
        });
