import super.core.calc (*)
import super.core.labels (annotate as with_tag)

pub fn run : i32 -> i32 = \x -> bump (double x)
pub fn describe : i32 -> i32 = \x -> with_tag 100 (run x)

