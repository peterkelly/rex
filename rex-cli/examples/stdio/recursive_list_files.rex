/* CLI example: recursively list files under the current directory

Run:
  cargo run -p rex-cli --bin rex -- rex-cli/examples/stdio/recursive_list_files.rex
*/

import std.io;

fn append_line acc: string -> line: string -> string =
  if acc == "" then line else acc + "\n" + line;

fn append_lines acc: string -> lines: string -> string =
  if lines == "" then acc else append_line acc lines;

fn walk path: string -> io.IO string =
  bind
    (\is_dir ->
      if is_dir then
        bind
          (\entries ->
            foldl
              (\acc_io entry ->
                bind
                  (\acc ->
                    bind
                      (\found -> pure (append_lines acc found))
                      (walk entry))
                  acc_io)
              (pure "")
              entries)
          (io.read_dir path)
      else
        pure path)
    (io.is_dir path);

bind walk io.current_dir
