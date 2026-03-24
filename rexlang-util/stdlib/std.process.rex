
pub type Subprocess = Subprocess { id: uuid }

pub declare fn spawn (opts: { cmd: string, args: List string }) -> Subprocess
pub declare fn wait (p: Subprocess) -> i32
pub declare fn stdout (p: Subprocess) -> Array u8
pub declare fn stderr (p: Subprocess) -> Array u8
