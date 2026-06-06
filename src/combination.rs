use std::sync::atomic::{AtomicU32, Ordering};
use crate::types::{ComboStatus, ParamId, OPEN, COVERED, EXCLUDED};
use crate::exclusion::Exclusion;
use crate::parameter::Parameter;

static COMBO_COUNTER: AtomicU32 = AtomicU32::new(1);

pub fn reset_combo_counter() {
    COMBO_COUNTER.store(1, Ordering::Relaxed);
}

fn next_combo_id() -> u32 {
    COMBO_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// A single N-way combination of parameters, tracking which (param-value) tuples
/// have been covered.  The bitvec index encodes a mixed-radix value over the
/// parameters in `params` order (most-significant param first).
#[derive(Debug)]
pub struct Combination {
    pub id: u32,
    /// Indices into the model's `params` Vec
    pub params: Vec<ParamId>,
    /// Bitvec: OPEN / COVERED / EXCLUDED for each possible value tuple
    pub bitvec: Vec<u8>,
    pub range: i32,
    pub open_count: i32,
    pub bound_count: i32,
}

impl Combination {
    pub fn new() -> Self {
        Self {
            id: next_combo_id(),
            params: Vec::new(),
            bitvec: Vec::new(),
            range: 0,
            open_count: 0,
            bound_count: 0,
        }
    }

    pub fn push_param(&mut self, pid: ParamId) {
        self.params.push(pid);
    }

    pub fn pop_param(&mut self) {
        self.params.pop();
    }

    /// Assign another combination's shape (not the bitvec).
    pub fn assign_shape(&mut self, other: &Combination) {
        self.params = other.params.clone();
        self.range = other.range;
        self.open_count = other.open_count;
        self.bound_count = other.bound_count;
    }

    /// Allocate and initialise the bitvec.
    /// Returns the number of OPEN slots added to global_zeros.
    pub fn set_map_size(&mut self, size: i32, fill: u8, global_zeros: &mut i32) {
        self.range = size;
        self.bitvec = vec![fill; size as usize];
        if fill == OPEN {
            self.open_count = size;
            *global_zeros += size;
        } else {
            self.open_count = 0;
        }
    }

    pub fn is_fully_bound(&self) -> bool {
        self.bound_count == self.params.len() as i32
    }

    pub fn init_binding(&mut self) {
        self.bound_count = 0;
    }

    // ── Bind ──────────────────────────────────────────────────────────────

    /// Bind the combination's parameters to the values encoded in `value`.
    /// Returns the number of parameters newly bound.
    pub fn bind(
        &self,
        value: i32,
        params: &mut Vec<Parameter>,
        combos: &mut Vec<Combination>,
        worklist: &mut crate::worklist::WorkList,
        global_zeros: &mut i32,
    ) -> i32 {
        let mut bound = 0i32;
        // Mark all as pending first to avoid double-adding to worklist
        for &pid in &self.params {
            params[pid].mark_pending();
        }
        let mut v = value;
        for i in (0..self.params.len()).rev() {
            let pid = self.params[i];
            let vc = params[pid].value_count;
            if !params[pid].bound {
                // bind_param returns the number of combos that became fully bound
                bind_param(params, combos, pid, v % vc, worklist, global_zeros);
                bound += 1;
            }
            v /= vc;
        }
        bound
    }

    // ── AddBinding (called when one parameter gets bound) ─────────────────

    /// Called by `bind_param` after a single parameter is bound.
    /// Returns the new bound_count.
    pub fn add_binding(&mut self, params: &[Parameter], global_zeros: &mut i32) -> i32 {
        self.bound_count += 1;
        if self.bound_count == self.params.len() as i32 {
            // Compute the bitvec index for the current bound values
            let mut idx = 0usize;
            for &pid in &self.params {
                idx = idx * params[pid].value_count as usize + params[pid].current_value;
            }
            if self.bitvec[idx] == OPEN {
                self.bitvec[idx] = COVERED;
                self.open_count -= 1;
                *global_zeros -= 1;
            }
        }
        self.bound_count
    }

    // ── Feasible (single value) ────────────────────────────────────────────

    pub fn feasible_value(&self, value: i32, params: &[Parameter]) -> ComboStatus {
        if self.bitvec[value as usize] == EXCLUDED {
            return ComboStatus::Excluded;
        }
        let status = if self.bitvec[value as usize] == COVERED {
            ComboStatus::CoveredMatch
        } else {
            ComboStatus::Open
        };
        let mut v = value;
        for i in (0..self.params.len()).rev() {
            let pid = self.params[i];
            let vc = params[pid].value_count;
            if params[pid].bound
                && params[pid].current_value as i32 != v % vc
            {
                return ComboStatus::Excluded;
            }
            v /= vc;
        }
        status
    }

    // ── Feasible (count zeros given current partial binding) ──────────────

    pub fn feasible_count(&self, params: &[Parameter], workbuf: &mut Vec<i32>) -> i32 {
        let mut n_work = 1usize;
        workbuf.resize(self.range as usize + 1, 0);
        workbuf[0] = 0;

        for &pid in &self.params {
            let vc = params[pid].value_count as usize;
            if params[pid].bound {
                let cur = params[pid].current_value as i32;
                for i in 0..n_work {
                    workbuf[i] = workbuf[i] * vc as i32 + cur;
                }
            } else {
                // fan out: each existing slot becomes `vc` slots
                for i in (0..n_work).rev() {
                    let base = workbuf[i] * vc as i32;
                    workbuf[i] = base; // addend 0
                    for addend in 1..vc {
                        workbuf[i + n_work * addend] = base + addend as i32;
                    }
                }
                n_work *= vc;
            }
        }

        let mut zeros = 0i32;
        for i in 0..n_work {
            if self.bitvec[workbuf[i] as usize] == OPEN {
                zeros += 1;
            }
        }
        zeros
    }

    // ── Weight ────────────────────────────────────────────────────────────

    pub fn weight(&self, value: i32, params: &[Parameter]) -> i32 {
        let mut v = value;
        let mut total = 0i32;
        for i in (0..self.params.len()).rev() {
            let pid = self.params[i];
            let vc = params[pid].value_count;
            total += params[pid].weight(v % vc);
            v /= vc;
        }
        total
    }

    // ── SetOpen ───────────────────────────────────────────────────────────

    pub fn set_open(&mut self, index: i32, global_zeros: &mut i32) {
        debug_assert!(index < self.range);
        if self.bitvec[index as usize] != OPEN {
            self.bitvec[index as usize] = OPEN;
            self.open_count += 1;
            *global_zeros += 1;
        }
    }

    // ── ViolatesExclusion ─────────────────────────────────────────────────

    pub fn violates_exclusion(&self, params: &[Parameter]) -> bool {
        let mut key = 0usize;
        for &pid in &self.params {
            key = key * params[pid].value_count as usize + params[pid].current_value;
        }
        self.bitvec[key] == EXCLUDED
    }

    // ── ApplyExclusion ────────────────────────────────────────────────────

    pub fn apply_exclusion(&mut self, excl: &Exclusion, params: &[Parameter], global_zeros: &mut i32) {
        // Only apply if every exclusion term's parameter is in this combination
        for &(gid, _) in &excl.terms {
            if !self.params.iter().any(|&pid| params[pid].global_id == gid) {
                return;
            }
        }
        self.apply_exclusion_inner(excl, 0, 0, params, global_zeros);
    }

    fn apply_exclusion_inner(
        &mut self,
        excl: &Exclusion,
        param_pos: usize,
        index: i32,
        params: &[Parameter],
        global_zeros: &mut i32,
    ) {
        if param_pos == self.params.len() {
            debug_assert!(index < self.range);
            if self.bitvec[index as usize] == OPEN {
                self.open_count -= 1;
                *global_zeros -= 1;
            }
            self.bitvec[index as usize] = EXCLUDED;
        } else {
            let pid = self.params[param_pos];
            let vc = params[pid].value_count;
            let gid = params[pid].global_id;

            // Is this param bound to a specific value in the exclusion?
            if let Some(&(_, excl_val)) = excl.terms.iter().find(|&&(g, _)| g == gid) {
                self.apply_exclusion_inner(
                    excl,
                    param_pos + 1,
                    index * vc + excl_val,
                    params,
                    global_zeros,
                );
            } else {
                for n in 0..vc {
                    self.apply_exclusion_inner(
                        excl,
                        param_pos + 1,
                        index * vc + n,
                        params,
                        global_zeros,
                    );
                }
            }
        }
    }
}

// ── Free function: bind a single parameter ────────────────────────────────────

/// Bind parameter `pid` to `value`, updating all linked combinations.
pub fn bind_param(
    params: &mut Vec<Parameter>,
    combos: &mut Vec<Combination>,
    pid: ParamId,
    value: i32,
    worklist: &mut crate::worklist::WorkList,
    global_zeros: &mut i32,
) {
    debug_assert!(!params[pid].bound);
    debug_assert!(value < params[pid].value_count);

    params[pid].temp_results.push(value as usize);
    params[pid].current_value = value as usize;
    params[pid].bound = true;

    let combo_ids = params[pid].combinations.clone();
    for cid in combo_ids {
        // Need to compute add_binding without holding a borrow on params
        let combo_param_count = combos[cid].params.len() as i32;
        let new_bound = {
            combos[cid].bound_count += 1;
            combos[cid].bound_count
        };

        if new_bound == combo_param_count {
            // fully bound — compute index and mark covered
            let mut idx = 0usize;
            for &p in &combos[cid].params {
                idx = idx * params[p].value_count as usize + params[p].current_value;
            }
            if combos[cid].bitvec[idx] == OPEN {
                combos[cid].bitvec[idx] = COVERED;
                combos[cid].open_count -= 1;
                *global_zeros -= 1;
            }
        }

        if new_bound == combo_param_count - 1 {
            // one away from fully bound — add unbound params to worklist
            let pids: Vec<ParamId> = combos[cid].params.clone();
            for other_pid in pids {
                if !params[other_pid].bound {
                    worklist.add_item(params, other_pid);
                }
            }
        }
    }
}

/// Pick the best value for parameter `pid` given the current partial binding.
/// Returns the value index.
pub fn pick_value(
    params: &mut Vec<Parameter>,
    combos: &mut Vec<Combination>,
    pid: ParamId,
    workbuf: &mut Vec<i32>,
    mode: crate::types::GenerationMode,
) -> Result<i32, crate::types::GenerationError> {
    use crate::types::{GenerationError, ErrorType};

    debug_assert!(!params[pid].bound);

    let vc = params[pid].value_count;
    let mut max_complete = 0i32;
    let mut max_total = 0i32;
    let mut best_value = 0i32;
    let mut best_count = 0i32;

    // Temporarily pretend we're bound so feasible_count works correctly
    params[pid].bound = true;

    for value in 0..vc {
        params[pid].current_value = value as usize;

        let mut total_zeros = 0i32;
        let mut complete = 0i32;
        let mut excluded = false;

        let combo_ids = params[pid].combinations.clone();
        for cid in combo_ids {
            let zeros = combos[cid].feasible_count(params, workbuf);
            total_zeros += zeros;
            if combos[cid].bound_count >= combos[cid].params.len() as i32 - 1 {
                if zeros > 0 {
                    complete += 1;
                }
                if combos[cid].violates_exclusion(params) {
                    total_zeros = -1;
                    complete = -1;
                    excluded = true;
                    break;
                }
            }
        }

        if excluded {
            continue;
        }

        if complete > max_complete {
            max_complete = complete;
            best_value = value;
            max_total = total_zeros;
            best_count = 1;
        } else if complete == max_complete {
            if total_zeros > max_total {
                best_value = value;
                max_total = total_zeros;
                best_count = 1;
            } else if !params[pid].weights.is_empty()
                && total_zeros == 0
                && max_total == 0
            {
                let w = params[pid].weight(value);
                best_count += w;
                if rng_rand_range(best_count) < w {
                    best_value = value;
                }
            } else if total_zeros == max_total {
                best_count += 1;
                if rng_rand_range(best_count) == 0 {
                    best_value = value;
                    max_total = total_zeros;
                }
            }
        }
    }

    params[pid].bound = false;

    if best_count <= 0 && mode != crate::types::GenerationMode::Preview {
        return Err(GenerationError::new(ErrorType::GenerationFailure));
    }

    Ok(best_value)
}

// Simple thread-local LCG matching POSIX rand() interface
thread_local! {
    static RNG_STATE: std::cell::Cell<u64> = std::cell::Cell::new(1);
}

pub fn rng_srand(seed: u64) {
    RNG_STATE.with(|s| s.set(if seed == 0 { 1 } else { seed }));
}

pub fn rng_rand() -> i32 {
    RNG_STATE.with(|s| {
        let mut state = s.get();
        state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        s.set(state);
        ((state >> 16) & 0x7FFF) as i32
    })
}

/// Returns a random value in [0, n)
pub fn rng_rand_range(n: i32) -> i32 {
    if n <= 1 {
        return 0;
    }
    rng_rand() % n
}
