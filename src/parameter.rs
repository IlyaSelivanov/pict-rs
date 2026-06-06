use std::sync::atomic::{AtomicU32, Ordering};
use crate::types::{ExclusionId, CombId};

static PARAM_COUNTER: AtomicU32 = AtomicU32::new(1);

pub fn next_param_global_id() -> u32 {
    PARAM_COUNTER.fetch_add(1, Ordering::Relaxed)
}

pub fn reset_param_counter() {
    PARAM_COUNTER.store(1, Ordering::Relaxed);
}

/// A model parameter with its value count, ordering preference, and runtime binding state.
#[derive(Debug)]
pub struct Parameter {
    /// Globally unique ID; used as the "sequence number" in exclusion term comparison.
    pub global_id: u32,
    /// 1-based position in the owning model's parameter list (for result-row indexing).
    pub sequence: i32,
    pub name: String,
    pub order: i32,
    pub value_count: i32,
    pub is_result_param: bool,

    // ── per-iteration binding state ────────────────────────────────────────
    pub bound: bool,
    pub pending: bool,
    pub current_value: usize,

    // ── links set up during choose() ──────────────────────────────────────
    /// CombIds (indices into the local combo arena) this parameter belongs to.
    pub combinations: Vec<CombId>,

    // ── exclusion back-references ──────────────────────────────────────────
    /// IDs of exclusions (in the owning ExclusionCollection) that reference this parameter.
    pub exclusion_back_refs: Vec<ExclusionId>,
    pub avg_exclusion_size: f32,

    // ── accumulated results ────────────────────────────────────────────────
    pub temp_results: Vec<usize>,
    pub result_iter: usize,

    // ── optional weights ──────────────────────────────────────────────────
    pub weights: Vec<i32>,

    // ── pseudo-parameter support ──────────────────────────────────────────
    /// If Some(idx), this is a pseudo-parameter representing model.submodels[idx].
    pub submodel_idx: Option<usize>,
}

impl Parameter {
    pub fn new(order: i32, sequence: i32, value_count: i32, name: String, is_result_param: bool) -> Self {
        let global_id = next_param_global_id();
        let effective_order = if is_result_param { 1 } else { order };
        Self {
            global_id,
            sequence,
            name,
            order: effective_order,
            value_count,
            is_result_param,
            bound: false,
            pending: false,
            current_value: 0,
            combinations: Vec::new(),
            exclusion_back_refs: Vec::new(),
            avg_exclusion_size: 0.0,
            temp_results: Vec::new(),
            result_iter: 0,
            weights: Vec::new(),
            submodel_idx: None,
        }
    }

    pub fn new_pseudo(order: i32, sequence: u32, submodel_value_count: i32, submodel_idx: usize, name: String) -> Self {
        let global_id = next_param_global_id();
        Self {
            global_id,
            sequence: sequence as i32,
            name,
            order,
            value_count: submodel_value_count,
            is_result_param: false,
            bound: false,
            pending: false,
            current_value: 0,
            combinations: Vec::new(),
            exclusion_back_refs: Vec::new(),
            avg_exclusion_size: 0.0,
            temp_results: Vec::new(),
            result_iter: 0,
            weights: Vec::new(),
            submodel_idx: Some(submodel_idx),
        }
    }

    pub fn is_pseudo(&self) -> bool {
        self.submodel_idx.is_some()
    }

    // ── weights ────────────────────────────────────────────────────────────

    pub fn set_weights(&mut self, weights: Vec<i32>) {
        assert_eq!(weights.len(), self.value_count as usize);
        self.weights = weights;
    }

    pub fn weight(&self, n: i32) -> i32 {
        if n >= 0 && (n as usize) < self.weights.len() {
            self.weights[n as usize]
        } else {
            1
        }
    }

    // ── exclusion back-refs ────────────────────────────────────────────────

    pub fn link_exclusion(&mut self, excl_id: ExclusionId, excl_size: usize) {
        let prev_count = self.exclusion_back_refs.len() as f32;
        self.avg_exclusion_size = (self.avg_exclusion_size * prev_count + excl_size as f32)
            / (prev_count + 1.0);
        if !self.exclusion_back_refs.contains(&excl_id) {
            self.exclusion_back_refs.push(excl_id);
        }
    }

    pub fn unlink_exclusion(&mut self, excl_id: ExclusionId) {
        self.exclusion_back_refs.retain(|&id| id != excl_id);
    }

    pub fn clear_exclusions(&mut self) {
        self.exclusion_back_refs.clear();
        self.avg_exclusion_size = 0.0;
    }

    pub fn exclusion_count(&self) -> usize {
        self.exclusion_back_refs.len()
    }

    // ── binding ────────────────────────────────────────────────────────────

    pub fn init_binding(&mut self) {
        self.bound = false;
        self.pending = false;
    }

    pub fn mark_pending(&mut self) {
        self.pending = true;
    }

    // ── result iteration ───────────────────────────────────────────────────

    pub fn get_first(&mut self) {
        self.result_iter = 0;
    }

    pub fn get_next(&mut self) -> usize {
        let v = self.temp_results[self.result_iter];
        self.result_iter += 1;
        v
    }

    pub fn get_last(&self) -> usize {
        self.current_value
    }

    pub fn temp_result_count(&self) -> usize {
        self.temp_results.len()
    }

    pub fn clean_up(&mut self) {
        self.pending = false;
        self.bound = false;
        self.combinations.clear();
        self.temp_results.clear();
        self.exclusion_back_refs.clear();
    }

    // ── combination linking ────────────────────────────────────────────────

    pub fn link_combination(&mut self, comb_id: CombId) {
        self.combinations.push(comb_id);
    }

    pub fn sort_combinations(&mut self, combo_ids_by_id: &dyn Fn(CombId) -> u32) {
        self.combinations.sort_by_key(|&cid| combo_ids_by_id(cid));
    }
}

/// Undefined value sentinel used for result parameters
pub const UNDEFINED_VALUE: usize = 0xFFFF_FFFF;
