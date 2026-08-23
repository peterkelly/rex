// Workflow: Generate a new 1200x630 PNG containing a linear gradient from dark
// navy (#0b1020) to indigo (#4f46e5). It has no source-file inputs.
//
// Run from the workspace root without an inputs JSON file:
//
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/imagemagick/generate_gradient.rex
//
// On success the ImageOutput contains a single Image whose content field is the
// CAS hash of the generated PNG. That hash can be passed directly to later Rex
// workflows as an image input.
import tools.imagemagick as IM;

IM.generate
    (IM.LinearGradient
        (IM.Size { width = 1200, height = 630 })
        (IM.Color { value = "#0b1020" })
        (IM.Color { value = "#4f46e5" }))
    []
    (IM.Encoding {
        format = IM.Format { name = "png" },
        mode = IM.AdjoinFrames,
        options = []
    })
