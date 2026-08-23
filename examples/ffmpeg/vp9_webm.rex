// Workflow: Transcode CAS-backed media to WebM using constant-quality VP9 video
// at CRF 31 with no target bit rate and 128-kbit/s Opus audio.
//
// Run from the workspace root. Import media containing video and audio:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import source.mp4
//
// Create inputs.json with the printed hash: {"input":"<media-hash>"}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/ffmpeg/vp9_webm.rex --inputs inputs.json
//
// On success the Media result's content field is the CAS hash of the encoded
// VP9/Opus WebM file.
import std.artifacts (Media);
import tools.ffmpeg as FF;

fn main (input: Hash) -> Result Media FF.FfmpegError =
    FF.transcode
        (FF.StoredMedia (Media { content = input }))
        []
        (FF.Encoding {
            format = FF.ContainerFormat { name = "webm" },
            video = Some (FF.VideoEncoding {
                codec = FF.Vp9,
                options = [FF.ConstantRateFactor 31.0, FF.VideoBitRate 0]
            }),
            audio = Some (FF.AudioEncoding { codec = FF.Opus, options = [FF.AudioBitRate 128000] }),
            subtitle = None,
            options = [],
            metadata = dict_empty
        });
