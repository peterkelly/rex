// Workflow: Fully probe one CAS-backed media file with FFprobe, including format,
// streams, chapters, programs, tags, and counted frames and packets. It performs
// analysis only and does not transcode the source.
//
// Run from the workspace root. Import the media into the workflow store:
//
//   cargo run -p rex-workflow -- --store-path ./store store import media.mkv
//
// Create inputs.json with the printed hash: {"input":"<media-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/ffmpeg/probe.rex --inputs inputs.json
//
// On success the MediaInfo result contains optional container information plus
// lists of stream, chapter, and program records. No new CAS content is produced.
import artifacts (Media);
import tools.ffmpeg as FF;

fn main (input: Hash) -> Result FF.MediaInfo FF.FfmpegError =
    FF.probe
        (Media { content = input })
        FF.ProbeOptions {
            count_frames = true,
            count_packets = true
        };
