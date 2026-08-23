// Workflow: Convert a four-second excerpt of a CAS-backed video into an animated
// GIF. The excerpt starts two seconds into the source, runs at 12 frames per
// second, and is scaled down with Lanczos filtering to at most 640 pixels wide
// while preserving aspect ratio and avoiding upscaling.
//
// Run from the workspace root. Import the source into the workflow store:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import video.mp4
//
// Create inputs.json with the printed hash: {"input":"<video-hash>"}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/ffmpeg/animated_gif.rex --inputs inputs.json
//
// On success the Media result's content field is the CAS hash of the encoded
// animated GIF.
import std.artifacts (Media);
import tools.ffmpeg as FF;

fn main (input: Hash) -> Result Media FF.FfmpegError =
    FF.transcode
        (FF.StoredMedia (Media { content = input }))
        [
            FF.Trim (FF.TimeRange {
                start = Some (FF.Time { seconds = 2.0 }),
                duration = Some (FF.Time { seconds = 4.0 })
            }),
            FF.VideoOperation (FF.FrameRate (FF.Rational { numerator = 12, denominator = 1 })),
            FF.VideoOperation (FF.Scale (FF.ScaleFilter {
                width = 640,
                height = -2,
                algorithm = Some "lanczos",
                preserve_aspect_ratio = true,
                prevent_upscale = true
            }))
        ]
        (FF.Encoding {
            format = FF.ContainerFormat { name = "gif" },
            video = Some (FF.VideoEncoding { codec = FF.GifVideo, options = [] }),
            audio = None,
            subtitle = None,
            options = [],
            metadata = dict_empty
        });
