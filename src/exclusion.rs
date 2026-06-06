use std::collections::HashMap;
use crate::types::{ExclusionId, Term};

static EXCLUSION_COUNTER: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(1);

fn next_exclusion_id() -> ExclusionId {
    EXCLUSION_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A set of (param_global_id, value_index) terms.
/// Sorted by param_global_id ascending (mirrors C++ ExclusionTermCompare).
#[derive(Debug, Clone)]
pub struct Exclusion {
    pub id: ExclusionId,
    /// Sorted by term.0 (param_global_id)
    pub terms: Vec<Term>,
    /// Same content, list form kept for trie permutation lookups in the deriver
    pub terms_list: Vec<Term>,
    pub deleted: bool,
}

impl Exclusion {
    pub fn new() -> Self {
        Self {
            id: next_exclusion_id(),
            terms: Vec::new(),
            terms_list: Vec::new(),
            deleted: false,
        }
    }

    pub fn with_terms(terms: Vec<Term>) -> Self {
        let mut excl = Self::new();
        for t in terms {
            excl.insert(t);
        }
        excl
    }

    /// Insert a term; returns true if newly added (false if already present).
    pub fn insert(&mut self, term: Term) -> bool {
        match self.terms.binary_search_by_key(&term.0, |t| t.0) {
            Ok(_) => false,
            Err(pos) => {
                self.terms.insert(pos, term);
                self.terms_list.push(term);
                true
            }
        }
    }

    pub fn len(&self) -> usize {
        self.terms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    pub fn mark_deleted(&mut self) {
        self.deleted = true;
    }

    pub fn is_deleted(&self) -> bool {
        self.deleted
    }

    /// Count result-params in this exclusion.
    /// `is_result`: closure mapping param_global_id → bool
    pub fn result_param_count(&self, is_result: &dyn Fn(u32) -> bool) -> usize {
        self.terms.iter().filter(|t| is_result(t.0)).count()
    }
}

impl PartialEq for Exclusion {
    fn eq(&self, other: &Self) -> bool {
        self.terms == other.terms
    }
}

impl Eq for Exclusion {}

/// ExclusionSizeLess ordering: smaller first, then lexicographic by terms.
impl PartialOrd for Exclusion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Exclusion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match self.terms.len().cmp(&other.terms.len()) {
            Ordering::Equal => {
                for (a, b) in self.terms.iter().zip(other.terms.iter()) {
                    match a.0.cmp(&b.0) {
                        Ordering::Equal => match a.1.cmp(&b.1) {
                            Ordering::Equal => continue,
                            o => return o,
                        },
                        o => return o,
                    }
                }
                Ordering::Equal
            }
            o => o,
        }
    }
}

/// Returns true if every term of `a` appears in `b` (a ⊆ b).
pub fn contained(a: &Exclusion, b: &Exclusion) -> bool {
    if b.len() < a.len() {
        return false;
    }
    let mut b_idx = 0usize;
    for &(a_gid, a_val) in &a.terms {
        // advance b until we find the matching param
        loop {
            if b_idx >= b.terms.len() {
                return false;
            }
            let (b_gid, b_val) = b.terms[b_idx];
            if b_gid == a_gid {
                if b_val != a_val {
                    return false;
                }
                b_idx += 1;
                break;
            } else if b_gid > a_gid {
                return false;
            } else {
                b_idx += 1;
            }
        }
    }
    true
}

// ─── ExclusionCollection ────────────────────────────────────────────────────

/// A sorted set of exclusions (sorted by ExclusionSizeLess).
/// Exclusions are stored by stable ID so that back-references remain valid
/// across insertions.
pub struct ExclusionCollection {
    by_id: HashMap<ExclusionId, Exclusion>,
    /// IDs kept in ExclusionSizeLess order
    sorted_ids: Vec<ExclusionId>,
}

impl Default for ExclusionCollection {
    fn default() -> Self {
        Self::new()
    }
}

impl ExclusionCollection {
    pub fn new() -> Self {
        Self {
            by_id: HashMap::new(),
            sorted_ids: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Insert exclusion. Returns `(id, inserted)`.
    pub fn insert(&mut self, excl: Exclusion) -> (ExclusionId, bool) {
        // Check for exact duplicate
        for &existing_id in &self.sorted_ids {
            if let Some(e) = self.by_id.get(&existing_id) {
                if e == &excl {
                    return (existing_id, false);
                }
            }
        }
        let id = excl.id;
        // Find sorted position
        let pos = self.sorted_ids.partition_point(|&eid| {
            self.by_id[&eid] < excl
        });
        self.sorted_ids.insert(pos, id);
        self.by_id.insert(id, excl);
        (id, true)
    }

    pub fn remove(&mut self, id: ExclusionId) {
        self.by_id.remove(&id);
        if let Some(pos) = self.sorted_ids.iter().position(|&x| x == id) {
            self.sorted_ids.remove(pos);
        }
    }

    pub fn get(&self, id: ExclusionId) -> Option<&Exclusion> {
        self.by_id.get(&id)
    }

    pub fn get_mut(&mut self, id: ExclusionId) -> Option<&mut Exclusion> {
        self.by_id.get_mut(&id)
    }

    pub fn clear(&mut self) {
        self.by_id.clear();
        self.sorted_ids.clear();
    }

    /// Iterator over exclusions in sorted order
    pub fn iter(&self) -> impl Iterator<Item = &Exclusion> {
        self.sorted_ids.iter().map(|id| &self.by_id[id])
    }

    /// Iterator over exclusion IDs in sorted order
    pub fn ids(&self) -> impl Iterator<Item = ExclusionId> + '_ {
        self.sorted_ids.iter().copied()
    }

    /// Remove all exclusions marked as deleted; returns count removed.
    pub fn purge_deleted(&mut self) -> usize {
        let to_remove: Vec<ExclusionId> = self
            .by_id
            .iter()
            .filter(|(_, e)| e.deleted)
            .map(|(&id, _)| id)
            .collect();
        let count = to_remove.len();
        for id in to_remove {
            self.remove(id);
        }
        count
    }
}
