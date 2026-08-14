// Workflow: Extract a 12-second clip beginning 30 seconds into CAS-backed media.
// The excerpt is re-encoded as H.264 CRF 20 with the medium preset and 192-kbit/s
// AAC, then placed in a streaming-friendly MP4 container.
//
// Run from the workspace root. Import media extending beyond the desired range:
//
//   cargo run -p rex-workflow -- --store-path ./store store import source.mp4
//
// Create inputs.json with the printed hash: {"input":"<media-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/ffmpeg/trim_clip.rex --inputs inputs.json
//
// On success the Media result's content field is the CAS hash of the 12-second
// MP4 clip.
import artifacts (Media);
import tools.ffmpeg as FF;

fn main (input: Hash) -> Result Media FF.FfmpegError =
    FF.transcode
        (FF.StoredMedia (Media { content = input }))
        [
            FF.Trim (FF.TimeRange {
                start = Some (FF.Time { seconds = 30.0 }),
                duration = Some (FF.Time { seconds = 12.0 })
            })
        ]
        (FF.Encoding {
            format = FF.ContainerFormat { name = "mp4" },
            video = Some (FF.VideoEncoding {
                codec = FF.H264,
                options = [FF.ConstantRateFactor 20.0, FF.Preset "medium"]
            }),
            audio = Some (FF.AudioEncoding { codec = FF.Aac, options = [FF.AudioBitRate 192000] }),
            subtitle = None,
            options = [FF.MovFlags ["faststart"]],
            metadata = dict_empty
        });
