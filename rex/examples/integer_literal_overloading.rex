(
  let x: u8 = 4 in x,
  let x: u16 = 4 in x,
  let x: u32 = 4 in x,
  let x: u64 = 4 in x,
  let x: i8 = 4 in x,
  let x: i16 = 4 in x,
  let x: i32 = 4 in x,
  let x: i64 = 4 in x,
  let x = 4 in (let f: u8 -> u8 = \n -> n in f x),
  let x = 4 in (let f: i64 -> i64 = \n -> n in f x)
)
