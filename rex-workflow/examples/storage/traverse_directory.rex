// cargo run --bin rex -- run --inputs inputs.json --raw-output rex-workflow/examples/storage/traverse_directory.rex
//
// inputs.json should look like:
//
// {
//     "root": "SOME_HASH"
// }

import std.storage (Blob, Tree);
import std.storage;

fn traverse (dir: Dict storage.Entry) -> (path: String) -> List String =
    let
        entries: List (List String) =
            map
            (transform_entry path)
            (dict_entries dir),
        add_lists: List String -> List String -> List String = \a b -> a + b
    in
        foldl add_lists [] entries;

fn transform_entry (path: String) -> (x: (String, storage.Entry)) -> List String =
    let
        (key, entry) = x
    in
        match entry.kind with {
            case Blob ->
                ["blob " + (show entry.hash) + " " + path + key];
            case Tree ->
                let
                    dir = storage.get_tree entry.hash,
                    child_path = path + key + "/",
                    line = "tree " + (show entry.hash) + " " + path + key
                in
                    [line] + (traverse dir child_path);
        };

fn main (root: Hash) -> String =
    let
        dir = storage.get_tree root
    in
        string_join "\n" (traverse dir "/");
