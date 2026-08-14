// Workflow: Extract a JPEG thumbnail at the ten-second point of a CAS-backed
// video. The frame is scaled to fit within 640x360 while preserving aspect ratio
// and is encoded with FFmpeg's MJPEG quality value 2.
//
// Run from the workspace root. Import a video at least ten seconds long:
//
//   cargo run -p rex-workflow -- --store-path ./store store import video.mp4
//
// Create inputs.json with the printed hash: {"input":"<video-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/ffmpeg/thumbnail.rex --inputs inputs.json
//
// On success the Media result's content field is the CAS hash of the extracted
// JPEG thumbnail.
import artifacts (Media);
import tools.ffmpeg as FF;

fn main (input: Hash) -> Result Media FF.FfmpegError =
    FF.thumbnail
        (Media { content = input })
        (FF.ThumbnailSpec {
            at = Some (FF.Time { seconds = 10.0 }),
            size = Some (FF.VideoSize { width = 640, height = 360 }),
            preserve_aspect_ratio = true
        })
        (FF.ImageEncoding {
            format = FF.ContainerFormat { name = "jpg" },
            video = FF.VideoEncoding {
                codec = FF.MjpegVideo,
                options = [FF.VideoQuality 2.0]
            }
        });
