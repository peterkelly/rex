// Workflow: Synthesize three seconds of a 440-Hz sine wave at a 48-kHz sample
// rate. FFmpeg duplicates it to two channels and encodes signed 16-bit PCM in a
// WAV container; no source media is required.
//
// Run from the workspace root without an inputs JSON file:
//
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/ffmpeg/generate_sine_audio.rex
//
// On success the Media result's content field is the CAS hash of the generated
// stereo WAV file, ready to pass to another Rex workflow.
import tools.ffmpeg as FF;

FF.transcode
    (FF.SineAudio (FF.SineAudioSource {
        frequency = 440.0,
        sample_rate = 48000,
        duration = Some (FF.Time { seconds = 3.0 })
    }))
    []
    (FF.Encoding {
        format = FF.ContainerFormat { name = "wav" },
        video = None,
        audio = Some (FF.AudioEncoding {
            codec = FF.PcmS16Le,
            options = [FF.AudioChannels 2]
        }),
        subtitle = None,
        options = [],
        metadata = dict_empty
    })
