use std::{
    borrow::Borrow,
    collections::HashMap,
    fmt::{self, Display, Formatter},
    hash::{Hash, Hasher},
    sync::{Arc, Mutex, OnceLock, Weak},
};

// `Symbol` equality is allocation identity, while hashing and ordering remain
// text-shaped. That combination is sound because `Symbol::intern` guarantees
// that two live symbols with the same text share one allocation.
/// An interned Rex identifier.
///
/// Create symbols with [`Symbol::intern`]. Two `Symbol`s compare by interned
/// identity, so equality is a pointer check rather than repeated string
/// comparison. The interner guarantees that all live symbols with the same text
/// share one allocation.
///
/// Hashing, ordering, and borrowed lookup remain text-shaped, so `Symbol` works
/// naturally as a map key. Comparing a `Symbol` with an `&str` is also supported
/// as a convenience textual comparison.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(transparent)]
pub struct Symbol(Arc<str>);

impl Symbol {
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }

    pub fn intern(name: &str) -> Self {
        let mutex = INTERNER.get_or_init(|| Mutex::new(HashMap::new()));
        let mut table = match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        if let Some(existing) = table.get(name).and_then(Weak::upgrade) {
            return Symbol(existing);
        }

        prune_dead_symbols_if_needed(&mut table);
        if let Some(existing) = table.get(name).and_then(Weak::upgrade) {
            return Symbol(existing);
        }

        let sym = Symbol(Arc::from(name));
        table.insert(name.to_string(), Arc::downgrade(&sym.0));
        sym
    }
}

impl PartialEq for Symbol {
    fn eq(&self, other: &Self) -> bool {
        let same_allocation = Arc::ptr_eq(&self.0, &other.0);
        debug_assert!(
            same_allocation || self.as_ref() != other.as_ref(),
            "symbol interner invariant violated: duplicate live symbols for `{}`",
            self.as_ref()
        );
        same_allocation
    }
}

impl Eq for Symbol {}

impl PartialOrd for Symbol {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Symbol {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let ordering = self.as_ref().cmp(other.as_ref());
        debug_assert!(
            ordering != std::cmp::Ordering::Equal || Arc::ptr_eq(&self.0, &other.0),
            "symbol interner invariant violated: duplicate live symbols for `{}`",
            self.as_ref()
        );
        ordering
    }
}

impl Hash for Symbol {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_ref().hash(state);
    }
}

impl AsRef<str> for Symbol {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl PartialEq<&str> for Symbol {
    fn eq(&self, other: &&str) -> bool {
        self.as_ref() == *other
    }
}

impl PartialEq<Symbol> for &str {
    fn eq(&self, other: &Symbol) -> bool {
        *self == other.as_ref()
    }
}

impl Borrow<str> for Symbol {
    fn borrow(&self) -> &str {
        self.0.as_ref()
    }
}

impl Display for Symbol {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> serde::Deserialize<'de> for Symbol {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let name = <String as serde::Deserialize>::deserialize(deserializer)?;
        Ok(Symbol::intern(&name))
    }
}

// Global symbol interner.
//
// Design constraints:
// - Symbols wrap `Arc<str>` so cloning them is cheap.
// - The table stores weak references, so symbol payloads can be reclaimed after
//   the last live `Symbol` for a name is dropped.
// - The map keys are still strong `String`s, so dead keys are pruned
//   opportunistically as the table grows.
// - Interning holds the lock across lookup, weak upgrade, and replacement,
//   preserving the invariant that two live symbols with the same text share the
//   same allocation.
static INTERNER: OnceLock<Mutex<HashMap<String, Weak<str>>>> = OnceLock::new();

const PRUNE_DEAD_SYMBOLS_MIN_LEN: usize = 1024;

fn prune_dead_symbols_if_needed(table: &mut HashMap<String, Weak<str>>) {
    let len = table.len();
    if len >= PRUNE_DEAD_SYMBOLS_MIN_LEN && len.is_power_of_two() {
        table.retain(|_, symbol| symbol.strong_count() > 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::{BTreeMap, HashMap},
        hash::{DefaultHasher, Hash, Hasher},
        thread,
    };

    #[test]
    fn deserializing_symbol_interns_it() {
        let interned = Symbol::intern("alpha");
        let decoded: Symbol = serde_json::from_str("\"alpha\"").unwrap();

        assert!(Arc::ptr_eq(&interned.0, &decoded.0));
    }

    #[test]
    fn serializing_symbol_stays_string_shaped() {
        let encoded = serde_json::to_string(&Symbol::intern("alpha")).unwrap();

        assert_eq!(encoded, "\"alpha\"");
    }

    #[test]
    fn symbol_compares_with_str_refs_on_either_side() {
        let symbol = Symbol::intern("alpha");

        assert!(symbol == "alpha");
        assert!("alpha" == symbol);
    }

    #[test]
    fn symbol_hash_and_order_remain_text_shaped() {
        let alpha = Symbol::intern("alpha");
        let same_alpha = Symbol::intern("alpha");
        let beta = Symbol::intern("beta");

        assert_eq!(symbol_hash(&alpha), symbol_hash(&same_alpha));
        assert!(alpha < beta);

        let mut hash_map = HashMap::new();
        hash_map.insert(alpha.clone(), 1);
        assert_eq!(hash_map.get("alpha"), Some(&1));

        let mut tree_map = BTreeMap::new();
        tree_map.insert(alpha, 1);
        assert_eq!(tree_map.get("alpha"), Some(&1));
    }

    #[test]
    fn concurrent_interning_returns_one_live_allocation() {
        let symbols: Vec<_> = (0..32)
            .map(|_| thread::spawn(|| Symbol::intern("concurrent-symbol")))
            .map(|handle| handle.join().unwrap())
            .collect();

        let first = &symbols[0];
        for symbol in &symbols[1..] {
            assert_eq!(first, symbol);
            assert!(Arc::ptr_eq(&first.0, &symbol.0));
        }
    }

    #[test]
    fn weak_interner_releases_payload_after_last_symbol_drops() {
        let name = "weak-interner-releases-payload-after-last-symbol-drops";
        let old = {
            let symbol = Symbol::intern(name);
            let same_symbol = Symbol::intern(name);

            assert_eq!(symbol, same_symbol);
            assert!(Arc::ptr_eq(&symbol.0, &same_symbol.0));

            Arc::downgrade(&symbol.0)
        };

        assert!(old.upgrade().is_none());

        let symbol = Symbol::intern(name);
        let same_symbol = Symbol::intern(name);

        assert!(old.upgrade().is_none());
        assert_eq!(symbol, same_symbol);
        assert!(Arc::ptr_eq(&symbol.0, &same_symbol.0));
    }

    fn symbol_hash(symbol: &Symbol) -> u64 {
        let mut hasher = DefaultHasher::new();
        symbol.hash(&mut hasher);
        hasher.finish()
    }
}
