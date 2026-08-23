// cargo run --bin rex -- run --inputs inputs.json --raw-output examples/storage/pretty_tree.rex
//
// inputs.json should look like:
//
// {
//     "root": "SOME_HASH"
// }

import std.storage;

fn traverse (dir: Dict storage.Entry) -> (prefix: String) -> (indent: String) -> List String =
    let
        entries: List (List String) =
            map
            (\x ->
                let
                    (i, (name, entry)) = x,
                    last = (i + 1 == (length dir)),
                    (child_prefix, child_indent) =
                        if last then
                            (indent + "└── ", indent + "    ")
                        else
                            (indent + "├── ", indent + "│   ")
                in
                    (transform_entry last name entry child_prefix child_indent))
            (enumerate (dict_entries dir)),
        add_lists: List String -> List String -> List String = \a b -> a + b
    in
        foldl add_lists [] entries;

fn enumerate<a> (items: List a) -> List (u64, a) =
    let rec
        loop : List a -> u64 -> List (u64, a) = \items n ->
            match items with {
                case List.Empty -> [];
                case x::xs -> (n, x) :: (loop xs (n + 1));
            }
    in
        loop items 0;

fn transform_entry (last: Bool) -> (key: String) -> (entry: storage.Entry) -> (prefix: String) -> (indent: String) -> List String =
    match entry.kind with {
        case storage.EntryKind.Blob -> [prefix + key];
        case storage.EntryKind.Tree ->
            let
                dir = storage.get_tree entry.hash,
                line = prefix + key
            in
                [line] + (traverse dir prefix indent);
    };

fn main (root: Hash) -> String =
    let
        dir = storage.get_tree root
    in
        string_join "\n" (traverse dir "" "");
