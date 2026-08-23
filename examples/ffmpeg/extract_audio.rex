// Workflow: Extract the audio stream from one CAS-backed media file, discard
// video, and encode the audio as 48-kHz, 128-kbit/s Opus in an Ogg container.
//
// Run from the workspace root. Import media containing an audio stream:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import video.mp4
//
// Create inputs.json with the printed hash: {"input":"<media-hash>"}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/ffmpeg/extract_audio.rex --inputs inputs.json
//
// On success the Media result's content field is the CAS hash of the audio-only
// Ogg file.
import std.artifacts (Media);
import tools.ffmpeg as FF;

fn main (input: Hash) -> Result Media FF.FfmpegError =
    FF.extract_audio
        (Media { content = input })
        (FF.AudioEncoding {
            codec = FF.Opus,
            options = [FF.AudioBitRate 128000, FF.AudioSampleRate 48000]
        })
        (FF.ContainerFormat { name = "ogg" });
