// Workflow: Package CAS-backed media as a complete HLS presentation. It encodes
// H.264/yuv420p video with a 120-frame GOP and 160-kbit/s AAC audio, then writes
// four-second MPEG-TS segments and a media playlist with independent segments.
//
// Run from the workspace root. Import media containing video and audio:
//
//   cargo run -p rex-workflow -- --store-path ./store store import presentation.mp4
//
// Create inputs.json with the printed hash: {"input":"<media-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/ffmpeg/package_hls.rex --inputs inputs.json
//
// On success the MediaPackage has kind HlsPackage and a content hash naming a
// CAS tree containing the playlist and every segment. Export that tree to a new
// directory with:
//
//   cargo run -p rex-workflow -- --store-path ./store store export \
//     <tree-hash> output-directory
//
import artifacts (Media);
import tools.ffmpeg as FF;

fn main (input: Hash) -> Result FF.MediaPackage FF.FfmpegError =
    FF.package_hls
        (FF.StoredMedia (Media { content = input }))
        []
        (FF.Encoding {
            format = FF.ContainerFormat { name = "mpegts" },
            video = Some (FF.VideoEncoding {
                codec = FF.H264,
                options = [
                    FF.ConstantRateFactor 21.0,
                    FF.GroupOfPictures 120,
                    FF.PixelFormat "yuv420p"
                ]
            }),
            audio = Some (FF.AudioEncoding { codec = FF.Aac, options = [FF.AudioBitRate 160000] }),
            subtitle = None,
            options = [],
            metadata = dict_empty
        })
        (FF.HlsOutput {
            segment_duration = FF.Time { seconds = 4.0 },
            playlist_size = 0,
            segment_format = "mpegts",
            flags = ["independent_segments", "temp_file"],
            master_playlist = false
        });
