// Workflow: Sample one frame per second from a CAS-backed video and encode every
// sampled frame independently as PNG.
//
// Run from the workspace root. Import a source containing a video stream:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import video.mp4
//
// Create inputs.json with the printed hash: {"input":"<video-hash>"}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/ffmpeg/extract_frames.rex --inputs inputs.json
//
// On success the result is a time-ordered list of Media values, one for each
// sampled frame. Every content field is the CAS hash of an individual PNG.
import std.artifacts (Media);
import tools.ffmpeg as FF;

fn main (input: Hash) -> Result (List Media) FF.FfmpegError =
    FF.extract_frames
        (Media { content = input })
        (FF.FramesPerSecond 1.0)
        (FF.ImageEncoding {
            format = FF.ContainerFormat { name = "png" },
            video = FF.VideoEncoding { codec = FF.VideoCodec.Png, options = [] }
        });
