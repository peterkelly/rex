import super.core.calc (double, triple as thr);

pub fn score x: i32 -> i32 = double x + thr x;
pub fn report x: i32 -> i32 = score x + 7;
