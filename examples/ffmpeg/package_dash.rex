// Workflow: Package CAS-backed media for MPEG-DASH. It encodes H.264/yuv420p
// video and 160-kbit/s AAC audio, creates four-second timeline/template-based
// segments, and retains the complete presentation rather than a sliding window.
//
// Run from the workspace root. Import media containing video and audio:
//
//   cargo run -p rex --bin rex -- --store-path ./store store import presentation.mp4
//
// Create inputs.json with the printed hash: {"input":"<media-hash>"}
// Then run:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/ffmpeg/package_dash.rex --inputs inputs.json
//
// On success the MediaPackage has kind PackageKind.Dash and a content hash naming a
// CAS tree containing the MPD manifest and all media segments. Export that tree
// to a new directory with:
//
//   cargo run -p rex --bin rex -- --store-path ./store store export \
//     <tree-hash> output-directory
//
import std.artifacts (Media);
import tools.ffmpeg as FF;

fn main (input: Hash) -> Result FF.MediaPackage FF.FfmpegError =
    FF.package_dash
        (FF.StoredMedia (Media { content = input }))
        []
        (FF.Encoding {
            format = FF.ContainerFormat { name = "dash" },
            video = Some (FF.VideoEncoding {
                codec = FF.H264,
                options = [FF.ConstantRateFactor 21.0, FF.PixelFormat "yuv420p"]
            }),
            audio = Some (FF.AudioEncoding { codec = FF.Aac, options = [FF.AudioEncodeOption.BitRate 160000] }),
            subtitle = None,
            options = [],
            metadata = dict_empty
        })
        (FF.DashOutput {
            segment_duration = FF.Time { seconds = 4.0 },
            window_size = 0,
            extra_window_size = 5,
            use_template = true,
            use_timeline = true
        });
