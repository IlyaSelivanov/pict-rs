use std::collections::VecDeque;
use crate::types::{ExclusionId, GenerationMode, ParamId, Term};
use crate::exclusion::{contained, Exclusion, ExclusionCollection};
use crate::parameter::Parameter;
use crate::trie::Trie;

pub struct ExclusionDeriver<'a> {
    params: Vec<ParamId>,
    exclusions: ExclusionCollection,
    current_param: Option<ParamId>,
    generation_mode: GenerationMode,
    /// End iterator for buckets during buildExclusion recursion
    bucket_end: usize,
    worklist: VecDeque<ParamId>,
    lookup: Trie,
    deleted_at_least_one: bool,
    /// Reference to the owning model's parameter arena
    param_arena: &'a mut Vec<Parameter>,
    abort_callback: Option<fn() -> bool>,
}

impl<'a> ExclusionDeriver<'a> {
    pub fn new(
        param_arena: &'a mut Vec<Parameter>,
        generation_mode: GenerationMode,
        abort_callback: Option<fn() -> bool>,
    ) -> Self {
        Self {
            params: Vec::new(),
            exclusions: ExclusionCollection::new(),
            current_param: None,
            generation_mode,
            bucket_end: 0,
            worklist: VecDeque::new(),
            lookup: Trie::new(),
            deleted_at_least_one: false,
            param_arena,
            abort_callback,
        }
    }

    pub fn add_param(&mut self, pid: ParamId) {
        self.params.push(pid);
    }

    /// Insert an exclusion into the deriver's collection.
    /// Returns `(exclusion_id, was_inserted)`.
    pub fn add_exclusion(&mut self, excl: Exclusion, check_if_in_collection: bool) -> (ExclusionId, bool) {
        if check_if_in_collection && self.already_in_collection(&excl) {
            return (0, false);
        }
        let mut excl = excl;
        let (id, inserted) = self.exclusions.insert(excl.clone());
        if inserted {
            // Keep the list sorted for trie insertion
            let mut list = excl.terms_list.clone();
            list.sort();
            self.lookup.insert(&list);
        }
        (id, inserted)
    }

    pub fn get_exclusions_mut(&mut self) -> &mut ExclusionCollection {
        &mut self.exclusions
    }

    pub fn take_exclusions(self) -> ExclusionCollection {
        self.exclusions
    }

    // ── DeriveExclusions ──────────────────────────────────────────────────

    pub fn derive_exclusions(&mut self) {
        if self.generation_mode != GenerationMode::Regular {
            return;
        }

        // Add back-pointers from exclusions to their parameters
        let ids: Vec<ExclusionId> = self.exclusions.ids().collect();
        for eid in ids {
            self.add_exclusion_param_back_ptrs(eid);
        }

        // Seed the worklist
        for i in 0..self.params.len() {
            let pid = self.params[i];
            let ec = self.param_arena[pid].exclusion_count();
            let vc = self.param_arena[pid].value_count as usize;
            if ec >= vc {
                self.worklist.push_back(pid);
            }
        }

        while let Some(current_pid) = self.worklist.pop_front() {
            self.current_param = Some(current_pid);

            let vc = self.param_arena[current_pid].value_count as usize;

            // Build buckets: one per value of current_param
            let mut buckets: Vec<Vec<ExclusionId>> = vec![Vec::new(); vc];

            let back_refs = self.param_arena[current_pid].exclusion_back_refs.clone();
            for eid in back_refs {
                if let Some(excl) = self.exclusions.get(eid) {
                    if excl.is_deleted() {
                        continue;
                    }
                    // Find this param's value in the exclusion
                    let gid = self.param_arena[current_pid].global_id;
                    if let Some(&(_, val)) = excl.terms.iter().find(|&&(g, _)| g == gid) {
                        if (val as usize) < buckets.len() {
                            buckets[val as usize].push(eid);
                        }
                    }
                }
            }

            // All buckets must be non-empty for derivation to proceed
            if buckets.iter().any(|b| b.is_empty()) {
                continue;
            }

            self.bucket_end = buckets.len();
            let implied = Exclusion::new();
            self.build_exclusion(implied, &buckets, 0);

            self.perform_delete();
        }
    }

    // ── buildExclusion ────────────────────────────────────────────────────

    fn build_exclusion(
        &mut self,
        implied: Exclusion,
        buckets: &[Vec<ExclusionId>],
        bucket_idx: usize,
    ) {
        if let Some(cb) = self.abort_callback {
            if cb() {
                // propagate abort
                return;
            }
        }

        if bucket_idx < self.bucket_end {
            let bucket = buckets[bucket_idx].clone();
            for eid in bucket {
                let excl = match self.exclusions.get(eid) {
                    Some(e) if !e.is_deleted() => e.clone(),
                    _ => continue,
                };

                if !self.consistent(&implied, &excl) {
                    continue;
                }

                // Merge terms (excluding current_param's term)
                let current_gid = self.current_param
                    .map(|pid| self.param_arena[pid].global_id);
                let mut merged = implied.clone();
                for &term in &excl.terms {
                    if Some(term.0) != current_gid {
                        merged.insert(term);
                    }
                }

                if self.already_in_collection(&merged) {
                    continue;
                }

                self.build_exclusion(merged, buckets, bucket_idx + 1);
            }
        } else {
            // Leaf: add new derived exclusion
            let (eid, inserted) = self.add_exclusion(implied.clone(), false);
            if !inserted {
                return;
            }

            self.mark_obsolete(eid);
            self.add_exclusion_param_back_ptrs(eid);

            // Check if new exclusion pushes any param's exclusion count over its value count
            let terms_copy: Vec<Term> = implied.terms.clone();
            for (gid, _) in terms_copy {
                if let Some(&pid) = self.params.iter().find(|&&p| self.param_arena[p].global_id == gid) {
                    let ec = self.param_arena[pid].exclusion_count();
                    let vc = self.param_arena[pid].value_count as usize;
                    if ec >= vc && !self.worklist.contains(&pid) {
                        self.worklist.push_back(pid);
                    }
                }
            }
        }
    }

    // ── consistent ────────────────────────────────────────────────────────

    fn consistent(&self, a: &Exclusion, b: &Exclusion) -> bool {
        let current_gid = self.current_param
            .map(|pid| self.param_arena[pid].global_id);

        let (small, large) = if a.len() < b.len() { (a, b) } else { (b, a) };

        let mut large_idx = 0usize;
        for &(a_gid, a_val) in &small.terms {
            if Some(a_gid) == current_gid {
                continue;
            }
            // advance large_idx
            loop {
                if large_idx >= large.terms.len() {
                    return true;
                }
                let (l_gid, l_val) = large.terms[large_idx];
                if l_gid == a_gid {
                    if l_val != a_val {
                        return false;
                    }
                    large_idx += 1;
                    break;
                } else if l_gid > a_gid {
                    break; // this param not in large, ok
                } else {
                    large_idx += 1;
                }
            }
        }
        true
    }

    // ── alreadyInCollection ───────────────────────────────────────────────

    fn already_in_collection(&mut self, excl: &Exclusion) -> bool {
        let mut list = excl.terms_list.clone();
        list.sort();
        loop {
            if self.lookup.find_prefix(&list) {
                return true;
            }
            if !next_permutation(&mut list) {
                break;
            }
        }
        false
    }

    // ── markObsolete / markDeleted / performDelete ────────────────────────

    fn mark_obsolete(&mut self, new_id: ExclusionId) {
        let new_excl = match self.exclusions.get(new_id) {
            Some(e) => e.clone(),
            None => return,
        };
        if new_excl.is_empty() {
            return;
        }

        let ids: Vec<ExclusionId> = self.exclusions.ids().collect();
        for eid in ids {
            if eid == new_id {
                continue;
            }
            let existing = match self.exclusions.get(eid) {
                Some(e) if !e.is_deleted() => e.clone(),
                _ => continue,
            };
            if contained(&new_excl, &existing) {
                if let Some(e) = self.exclusions.get_mut(eid) {
                    e.mark_deleted();
                }
                self.deleted_at_least_one = true;
            }
        }
    }

    fn perform_delete(&mut self) {
        if !self.deleted_at_least_one {
            return;
        }

        // Remove deleted exclusions and rebuild lookup trie and param back-refs
        self.exclusions.purge_deleted();

        // Rebuild lookup trie from scratch
        self.lookup = Trie::new();
        let all_excls: Vec<Exclusion> = self.exclusions.iter().cloned().collect();
        for excl in &all_excls {
            let mut list = excl.terms_list.clone();
            list.sort();
            self.lookup.insert(&list);
        }

        // Clear and rebuild param back-refs
        for &pid in &self.params {
            self.param_arena[pid].clear_exclusions();
        }
        let ids: Vec<ExclusionId> = self.exclusions.ids().collect();
        for eid in ids {
            self.add_exclusion_param_back_ptrs(eid);
        }

        self.deleted_at_least_one = false;
    }

    fn add_exclusion_param_back_ptrs(&mut self, eid: ExclusionId) {
        let excl = match self.exclusions.get(eid) {
            Some(e) => e.clone(),
            None => return,
        };
        let size = excl.len();
        for &(gid, _) in &excl.terms {
            if let Some(&pid) = self.params.iter().find(|&&p| self.param_arena[p].global_id == gid) {
                self.param_arena[pid].link_exclusion(eid, size);
            }
        }
    }
}

// ── next_permutation (for alreadyInCollection) ────────────────────────────────

fn next_permutation(v: &mut Vec<Term>) -> bool {
    let n = v.len();
    if n < 2 {
        return false;
    }
    let mut i = n - 1;
    while i > 0 && v[i - 1] >= v[i] {
        i -= 1;
    }
    if i == 0 {
        return false;
    }
    let mut j = n - 1;
    while v[j] <= v[i - 1] {
        j -= 1;
    }
    v.swap(i - 1, j);
    v[i..].reverse();
    true
}
