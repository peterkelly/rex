// Workflow: Burn a separate subtitle file permanently into a CAS-backed video.
// Subtitles use a 22-point white style, video is encoded as H.264 CRF 20 with
// yuv420p pixels, audio as 160-kbit/s AAC, and the MP4 is optimized for streaming.
//
// Run from the workspace root. Import both files into the same store:
//
//   cargo run -p rex-workflow -- --store-path ./store store import video.mp4
//   cargo run -p rex-workflow -- --store-path ./store store import captions.srt
//
// Create inputs.json with the hashes printed by those commands:
//   {"video":"<video-hash>","subtitles":"<subtitles-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/ffmpeg/burn_subtitles.rex --inputs inputs.json
//
// On success the Media result's content field is the CAS hash of the subtitled
// MP4. The subtitle text is part of the video pixels, not a selectable track.
import std.artifacts (Media);
import tools.ffmpeg as FF;

fn main (video: Hash, subtitles: Hash) -> Result Media FF.FfmpegError =
    FF.transcode
        (FF.StoredMedia (Media { content = video }))
        [
            FF.VideoOperation
                (FF.BurnSubtitles (FF.SubtitleFilter {
                    subtitles = Media { content = subtitles },
                    stream_index = None,
                    fonts = [],
                    force_style = Some "FontSize=22,PrimaryColour=&H00FFFFFF"
                }))
        ]
        (FF.Encoding {
            format = FF.ContainerFormat { name = "mp4" },
            video = Some (FF.VideoEncoding {
                codec = FF.H264,
                options = [FF.ConstantRateFactor 20.0, FF.PixelFormat "yuv420p"]
            }),
            audio = Some (FF.AudioEncoding { codec = FF.Aac, options = [FF.AudioBitRate 160000] }),
            subtitle = None,
            options = [FF.MovFlags ["faststart"]],
            metadata = dict_empty
        });
