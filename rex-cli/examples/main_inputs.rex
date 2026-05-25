// Inspect manifest:
//
// rex --manifest main_inputs.rex
//
// Run with inputs:
//
// rex --inputs main_inputs.json main_inputs.rex

type SharedMeta = SharedMeta {
    label: string,
    weight: i32
};

type Measurement = Measurement {
    meta: SharedMeta,
    value: i32
};

type Threshold = Threshold {
    meta: SharedMeta,
    limit: i32
};

type Output = Output {
    total: i32,
    measurement_label: string,
    threshold_label: string,
    combined_weight: i32
};

fn main scale: i32 -> measurement: Measurement -> threshold: Threshold -> Output =
    let
        measurement_meta = measurement.meta,
        threshold_meta = threshold.meta,
        total = (measurement.value * scale)
            + measurement_meta.weight
            + threshold.limit
            + threshold_meta.weight
    in
        Output {
            total = total,
            measurement_label = measurement_meta.label,
            threshold_label = threshold_meta.label,
            combined_weight = measurement_meta.weight + threshold_meta.weight
        };
