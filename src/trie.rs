use std::collections::HashMap;
use crate::types::Term;

/// Simplified trie keyed on sequences of `Term` (param_global_id, value_index).
/// Used by ExclusionDeriver to detect prefix containment efficiently.
#[derive(Default)]
struct TrieNode {
    children: HashMap<Term, Box<TrieNode>>,
    valid: bool,
}

pub struct Trie {
    root: TrieNode,
}

impl Default for Trie {
    fn default() -> Self {
        Self::new()
    }
}

impl Trie {
    pub fn new() -> Self {
        Self {
            root: TrieNode::default(),
        }
    }

    /// Insert a sequence of terms.
    pub fn insert(&mut self, terms: &[Term]) {
        let mut node = &mut self.root;
        for &t in terms {
            node = node.children.entry(t).or_insert_with(|| Box::new(TrieNode::default()));
        }
        node.valid = true;
    }

    /// Erase a sequence (marks node invalid; does not reclaim memory).
    pub fn erase(&mut self, terms: &[Term]) {
        if let Some(node) = self.pfind_mut(terms) {
            node.valid = false;
        }
    }

    /// Returns true if `terms` or any of its prefixes is in the trie.
    pub fn find_prefix(&self, terms: &[Term]) -> bool {
        let mut node = &self.root;
        for &t in terms {
            match node.children.get(&t) {
                None => return false,
                Some(child) => {
                    if child.valid {
                        return true;
                    }
                    node = child;
                }
            }
        }
        node.valid
    }

    fn pfind_mut(&mut self, terms: &[Term]) -> Option<&mut TrieNode> {
        let mut node = &mut self.root;
        for &t in terms {
            match node.children.get_mut(&t) {
                None => return None,
                Some(child) => node = child,
            }
        }
        Some(node)
    }
}
