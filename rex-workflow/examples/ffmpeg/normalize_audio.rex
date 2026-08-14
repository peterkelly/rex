// Workflow: Produce an audio-only, loudness-normalized version of CAS-backed
// media. Video is discarded; EBU-style loudness targets are -16 LUFS integrated,
// 11 LU loudness range, and -1.5 dB true peak, followed by 192-kbit/s AAC in M4A.
//
// Run from the workspace root. Import media containing an audio stream:
//
//   cargo run -p rex-workflow -- --store-path ./store store import recording.wav
//
// Create inputs.json with the printed hash: {"input":"<media-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/ffmpeg/normalize_audio.rex --inputs inputs.json
//
// On success the Media result's content field is the CAS hash of the normalized
// M4A audio file.
import artifacts (Media);
import tools.ffmpeg as FF;

fn main (input: Hash) -> Result Media FF.FfmpegError =
    FF.transcode
        (FF.StoredMedia (Media { content = input }))
        [
            FF.DropVideo,
            FF.AudioOperation
                (FF.NormalizeLoudness (FF.LoudnessNormalization {
                    integrated_loudness = -16.0,
                    loudness_range = 11.0,
                    true_peak = -1.5
                }))
        ]
        (FF.Encoding {
            format = FF.ContainerFormat { name = "m4a" },
            video = None,
            audio = Some (FF.AudioEncoding {
                codec = FF.Aac,
                options = [FF.AudioBitRate 192000]
            }),
            subtitle = None,
            options = [],
            metadata = dict_empty
        });
