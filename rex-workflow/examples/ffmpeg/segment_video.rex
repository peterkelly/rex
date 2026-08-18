// Workflow: Split CAS-backed media into a sequence of approximately ten-second
// Matroska files. It re-encodes video as H.264 CRF 21 and audio as 160-kbit/s
// AAC, numbers segments from zero, and resets timestamps in every segment.
//
// Run from the workspace root. Import media containing video and audio:
//
//   cargo run -p rex-workflow -- --store-path ./store store import long-video.mp4
//
// Create inputs.json with the printed hash: {"input":"<media-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/ffmpeg/segment_video.rex --inputs inputs.json
//
// On success the result is a chronological list of Media values. Each content
// field is the CAS hash of one independently stored MKV segment.
import std.artifacts (Media);
import tools.ffmpeg as FF;

fn main (input: Hash) -> Result (List Media) FF.FfmpegError =
    FF.segment
        (FF.StoredMedia (Media { content = input }))
        []
        (FF.Encoding {
            format = FF.ContainerFormat { name = "matroska" },
            video = Some (FF.VideoEncoding { codec = FF.H264, options = [FF.ConstantRateFactor 21.0] }),
            audio = Some (FF.AudioEncoding { codec = FF.Aac, options = [FF.AudioBitRate 160000] }),
            subtitle = None,
            options = [],
            metadata = dict_empty
        })
        (FF.SegmentOutput {
            segment_duration = FF.Time { seconds = 10.0 },
            reset_timestamps = true,
            start_number = 0
        });
