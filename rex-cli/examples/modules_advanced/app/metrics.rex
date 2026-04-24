import super.core.calc (double, triple as thr)

pub fn score : i32 -> i32 = \x -> double x + thr x
pub fn report : i32 -> i32 = \x -> score x + 7

