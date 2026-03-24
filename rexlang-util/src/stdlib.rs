pub fn stdlib_source(module: &str) -> Option<&'static str> {
    match module {
        "std.io" => Some(include_str!("../stdlib/std.io.rex")),
        "std.process" => Some(include_str!("../stdlib/std.process.rex")),
        "std.json" => Some(include_str!("../stdlib/std.json.rex")),
        _ => None,
    }
}
