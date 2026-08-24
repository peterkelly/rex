// Workflow: Concatenate CAS-backed clips into one MP4 after normalizing them to
// 1280x720 video at 30 fps in yuv420p and 48-kHz stereo audio. The result uses
// H.264 CRF 21, 192-kbit/s AAC, and streaming-friendly MP4 layout.
//
// Run from the workspace root. Import at least two clips into the same store:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import clip-01.mp4
//
// Put the printed hashes in playback order in inputs.json:
//   {"inputs":["<clip-01-hash>","<clip-02-hash>"]}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/ffmpeg/concatenate.rex --inputs inputs.json
//
// Each clip must supply video and audio streams. On success the Media content
// field is the CAS hash of the combined MP4.
import std.artifacts (Media);
import tools.ffmpeg as FF;

fn to_media (hash: Hash) -> Media = Media { content = hash };

fn main (inputs: List Hash) -> Result Media FF.FfmpegError =
    FF.concatenate
        (map to_media inputs)
        (FF.ConcatSpec {
            video = true,
            audio = true,
            normalize_video = Some (FF.ScaleFilter {
                width = 1280,
                height = 720,
                algorithm = Some "bicubic",
                preserve_aspect_ratio = false,
                prevent_upscale = false
            }),
            normalize_video_frame_rate = Some (FF.Rational { numerator = 30, denominator = 1 }),
            normalize_video_pixel_format = Some "yuv420p",
            normalize_audio_rate = Some 48000,
            normalize_audio_channel_layout = Some "stereo"
        })
        (FF.Encoding {
            format = FF.ContainerFormat { name = "mp4" },
            video = Some (FF.VideoEncoding {
                codec = FF.H264,
                options = [FF.ConstantRateFactor 21.0, FF.PixelFormat "yuv420p"]
            }),
            audio = Some (FF.AudioEncoding { codec = FF.Aac, options = [FF.AudioEncodeOption.BitRate 192000] }),
            subtitle = None,
            options = [FF.MovFlags ["faststart"]],
            metadata = dict_empty
        });
