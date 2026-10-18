use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct ConstPool {
    pub strings: Vec<String>,
    /// Present when the stream was pool-sealed (bytecode 0x04): every
    /// `strings[i]` is a hex ciphertext keyed by `(key, i)`; consumers must
    /// go through `pool_seal::open_str`.
    pub key: Option<[u8; 16]>,
    /// O(1) dedup index for plaintext mode. Not serialized — rebuilt on load.
    #[doc(hidden)]
    pub index: HashMap<String, u32>,
}

impl ConstPool {
    pub fn new() -> Self {
        Self {
            strings: vec![],
            key: None,
            index: HashMap::new(),
        }
    }

    pub fn intern(&mut self, s: &str) -> u32 {
        if self.key.is_none() {
            if let Some(&idx) = self.index.get(s) {
                return idx;
            }
        }
        let idx = self.strings.len() as u32;
        let entry = match &self.key {
            Some(k) => crate::pool_seal::seal_str(k, idx, s),
            None => s.to_string(),
        };
        if self.key.is_none() {
            self.index.insert(entry.clone(), idx);
        }
        self.strings.push(entry);
        idx
    }

    pub fn get(&self, idx: u32) -> Option<&str> {
        self.strings.get(idx as usize).map(|s| s.as_str())
    }

    pub fn len(&self) -> usize {
        self.strings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }

    /// Rebuild the dedup index after deserialization.
    pub fn rebuild_index(&mut self) {
        if self.key.is_some() {
            return;
        }
        self.index.clear();
        self.index.reserve(self.strings.len());
        for (i, s) in self.strings.iter().enumerate() {
            self.index.entry(s.clone()).or_insert(i as u32);
        }
    }
}
