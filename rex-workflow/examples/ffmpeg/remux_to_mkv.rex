// Workflow: Repackage one CAS-backed media file as Matroska without re-encoding
// its streams. The source's global metadata and chapters are mapped into the new
// container, so this is fast and preserves the encoded audio/video exactly.
//
// Run from the workspace root. Import source media whose streams Matroska can
// contain:
//
//   cargo run -p rex-workflow -- --store-path ./store store import media.mp4
//
// Create inputs.json with the printed hash: {"input":"<media-hash>"}
// Then run:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/ffmpeg/remux_to_mkv.rex --inputs inputs.json
//
// On success the Media result's content field is the CAS hash of the remuxed MKV.
import std.artifacts (Media);
import tools.ffmpeg as FF;

fn main (input: Hash) -> Result Media FF.FfmpegError =
    FF.remux
        (Media { content = input })
        (FF.ContainerFormat { name = "matroska" })
        [FF.MapMetadataFrom (Some 0), FF.MapChaptersFrom (Some 0)];
