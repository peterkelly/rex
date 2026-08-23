// Workflow: Query the ImageMagick executable found on PATH and report its build
// and version information. This is useful for recording which external tool a
// workflow environment will use.
//
// Run from the workspace root without importing a file or supplying inputs:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/imagemagick/version.rex
//
// On success the result is a VersionInfo record containing ImageMagick's version
// text and reported features/delegates. No content is written to the CAS.
import tools.imagemagick as IM;

IM.version
