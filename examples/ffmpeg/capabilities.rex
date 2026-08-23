// Workflow: Query the FFmpeg executable found on PATH for the complete set of
// encoders enabled in this installation. This lets a workflow or agent inspect
// the actual runtime before choosing a codec.
//
// Run from the workspace root without importing media or supplying inputs:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/ffmpeg/capabilities.rex
//
// On success the result is a list of Capability records. Each record contains
// FFmpeg's flags, encoder name, and human-readable description. No media is
// created or written to the CAS.
import tools.ffmpeg as FF;

FF.capabilities FF.Encoders
