// Workflow: Query the FFmpeg executable found on PATH and report the precise
// version, build configuration, and linked library versions. This is useful for
// recording or diagnosing the media environment used by a workflow.
//
// Run from the workspace root without importing media or supplying inputs:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/ffmpeg/version.rex
//
// On success the result is a VersionInfo record with version, configuration, and
// library fields. No media is created or written to the CAS.
import tools.ffmpeg as FF;

FF.version
