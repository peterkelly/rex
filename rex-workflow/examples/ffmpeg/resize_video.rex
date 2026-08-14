// Workflow: Scale CAS-backed video to fit within 1280x720 without changing its
// aspect ratio or enlarging smaller sources. It uses Lanczos scaling, H.264 CRF
// 21 in yuv420p, 160-kbit/s AAC, and a streaming-friendly MP4 layout.
//
// Run from the workspace root. Import media containing video (and normally
// audio):
//
//   cargo run -p rex-workflow -- --store-path ./store store import video.mov
//
// Create inputs.json with the printed hash: {"input":"<video-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/ffmpeg/resize_video.rex --inputs inputs.json
//
// On success the Media result's content field is the resized MP4's CAS hash.
import artifacts (Media);
import tools.ffmpeg as FF;

fn main (input: Hash) -> Result Media FF.FfmpegError =
    FF.transcode
        (FF.StoredMedia (Media { content = input }))
        [
            FF.VideoOperation
                (FF.Scale (FF.ScaleFilter {
                    width = 1280,
                    height = 720,
                    algorithm = Some "lanczos",
                    preserve_aspect_ratio = true,
                    prevent_upscale = true
                }))
        ]
        (FF.Encoding {
            format = FF.ContainerFormat { name = "mp4" },
            video = Some (FF.VideoEncoding {
                codec = FF.H264,
                options = [FF.ConstantRateFactor 21.0, FF.PixelFormat "yuv420p"]
            }),
            audio = Some (FF.AudioEncoding { codec = FF.Aac, options = [FF.AudioBitRate 160000] }),
            subtitle = None,
            options = [FF.MovFlags ["faststart"]],
            metadata = dict_empty
        });
