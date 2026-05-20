import super.core.calc (*);
import super.core.labels (annotate as with_tag);

pub fn run x: i32 -> i32 = bump (double x);
pub fn describe x: i32 -> i32 = with_tag 100 (run x);
