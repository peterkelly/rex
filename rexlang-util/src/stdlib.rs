pub fn stdlib_source(library: &str) -> Option<&'static str> {
    match library {
        "std.io" => Some(include_str!("../stdlib/std.io.rex")),
        "std.process" => Some(include_str!("../stdlib/std.process.rex")),
        "std.json" => Some(include_str!("../stdlib/std.json.rex")),
        _ => None,
    }
}
