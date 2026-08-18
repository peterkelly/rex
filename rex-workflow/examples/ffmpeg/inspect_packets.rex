// Workflow: Inspect packet-level timing and size data for the first video stream
// of a CAS-backed media file. The query covers the first five seconds and asks
// for PTS, DTS, duration, byte size, and packet flags without creating media.
//
// Run from the workspace root. Import media containing a video stream:
//
//   cargo run -p rex-workflow -- --store-path ./store store import video.mp4
//
// Create inputs.json with the printed hash: {"input":"<video-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/ffmpeg/inspect_packets.rex --inputs inputs.json
//
// On success the result is a packet-ordered list of InspectionRecord values.
// Each record's fields dictionary contains the values FFprobe reported; no new
// CAS media object is produced.
import std.artifacts (Media);
import tools.ffmpeg as FF;

fn main (input: Hash) -> Result (List FF.InspectionRecord) FF.FfmpegError =
    FF.inspect
        (Media { content = input })
        (FF.InspectionQuery {
            kind = FF.InspectPackets,
            stream = Some (FF.StreamRef { input = 0, kind = FF.VideoStream, index = Some 0 }),
            read_intervals = Some "%+5",
            entries = ["pts_time", "dts_time", "duration_time", "size", "flags"]
        });
