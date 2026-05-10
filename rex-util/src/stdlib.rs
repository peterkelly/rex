pub fn stdlib_source(module: &str) -> Option<&'static str> {
    match module {
        "std.json" => Some(include_str!("../stdlib/std.json.rex")),
        _ => None,
    }
}
