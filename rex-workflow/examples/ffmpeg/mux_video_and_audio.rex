// Workflow: Combine the first video stream of one CAS object with the first
// audio stream of another. Video is copied without re-encoding, audio is encoded
// as 192-kbit/s AAC, and the shortest stream determines the MP4's duration.
//
// Run from the workspace root. Import both files into the same store:
//
//   cargo run -p rex-workflow -- --store-path ./store store import video.mp4
//   cargo run -p rex-workflow -- --store-path ./store store import soundtrack.wav
//
// Create inputs.json with the printed hashes:
//   {"video":"<video-hash>","audio":"<audio-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/ffmpeg/mux_video_and_audio.rex --inputs inputs.json
//
// The copied video codec must be valid in MP4. On success the Media content
// field is the CAS hash of the newly muxed, streaming-friendly MP4.
import artifacts (Media);
import tools.ffmpeg as FF;

fn main (video: Hash, audio: Hash) -> Result Media FF.FfmpegError =
    FF.mux
        [Media { content = video }, Media { content = audio }]
        [
            FF.MuxMapping {
                input = 0,
                kind = FF.VideoStream,
                stream_index = Some 0,
                copy = true
            },
            FF.MuxMapping {
                input = 1,
                kind = FF.AudioStream,
                stream_index = Some 0,
                copy = false
            }
        ]
        (FF.Encoding {
            format = FF.ContainerFormat { name = "mp4" },
            video = None,
            audio = Some (FF.AudioEncoding { codec = FF.Aac, options = [FF.AudioBitRate 192000] }),
            subtitle = None,
            options = [FF.ShortestOutput, FF.MovFlags ["faststart"]],
            metadata = dict_empty
        });
