use crate::combination::{
    bind_param, pick_value, rng_rand, rng_rand_range, rng_srand, Combination,
};
use crate::deriver::ExclusionDeriver;
use crate::exclusion::{Exclusion, ExclusionCollection};
use crate::parameter::{Parameter, UNDEFINED_VALUE};
use crate::types::{
    ComboStatus, ErrorType, GenerationError, GenerationMode, GenerationType,
    ParamId, ResultCollection, ResultRow, RowSeed, RowSeedCollection,
    Term, COVERED, OPEN, MAX_ROWS_TO_GENERATE, PSEUDO_PARAM_SEQ_BASE,
};
use crate::worklist::WorkList;
use std::collections::HashSet;

pub struct Model {
    pub id: String,
    pub params: Vec<Parameter>,
    pub submodels: Vec<Box<Model>>,
    pub exclusions: ExclusionCollection,
    pub row_seeds: RowSeedCollection,
    pub results: ResultCollection,
    pub order: i32,
    pub max_rows: i64,
    pub generation_type: GenerationType,
    pub random_seed: u64,
    last_param_id: u32,
    pub total_combinations: i64,
    pub remaining_combinations: i64,
}

impl Model {
    pub fn new(id: String, gen_type: GenerationType, order: i32, seed: u64) -> Self {
        let mut m = Self {
            id,
            params: Vec::new(),
            submodels: Vec::new(),
            exclusions: ExclusionCollection::new(),
            row_seeds: RowSeedCollection::new(),
            results: Vec::new(),
            order,
            max_rows: 0,
            generation_type: gen_type,
            random_seed: seed,
            last_param_id: PSEUDO_PARAM_SEQ_BASE + 1_000_000,
            total_combinations: 0,
            remaining_combinations: 0,
        };
        m.set_random_seed(seed);
        m
    }

    pub fn set_random_seed(&mut self, seed: u64) {
        self.random_seed = seed;
        rng_srand(seed);
        for sub in &mut self.submodels {
            sub.set_random_seed(seed);
        }
    }

    pub fn set_order(&mut self, order: i32) {
        self.order = order;
    }

    pub fn add_parameter(&mut self, param: Parameter) {
        self.params.push(param);
    }

    pub fn add_submodel(&mut self, model: Box<Model>) {
        self.submodels.push(model);
    }

    pub fn add_exclusion(&mut self, excl: Exclusion) -> bool {
        let (_, inserted) = self.exclusions.insert(excl);
        inserted
    }

    pub fn add_row_seed(&mut self, seed: RowSeed) {
        self.row_seeds.push_back(seed.clone());
        for sub in &mut self.submodels {
            sub.add_row_seed(seed.clone());
        }
    }

    pub fn add_row_seeds<I: IntoIterator<Item = RowSeed>>(&mut self, seeds: I) {
        for s in seeds {
            self.add_row_seed(s);
        }
    }

    pub fn total_param_count(&self) -> usize {
        self.params.len()
    }

    pub fn result_count(&self) -> usize {
        self.results.len()
    }

    pub fn result_param_count(&self) -> usize {
        self.params.iter().filter(|p| p.is_result_param).count()
    }

    pub fn submodel_count(&self) -> usize {
        self.submodels.len()
    }

    // ── Main generation entry ─────────────────────────────────────────────

    pub fn generate(
        &mut self,
        workbuf: &mut Vec<i32>,
        generation_mode: GenerationMode,
        abort_callback: Option<fn() -> bool>,
    ) -> Result<(), GenerationError> {
        self.results.clear();

        match self.generation_type {
            GenerationType::MixedOrder => {
                self.generate_mixed_order(workbuf, generation_mode, abort_callback)?
            }
            GenerationType::FixedOrder => {
                self.generate_fixed_order(workbuf, generation_mode, abort_callback)?
            }
            GenerationType::Full => {
                self.generate_full(workbuf, generation_mode, abort_callback)?
            }
            GenerationType::Flat => {
                self.generate_flat(workbuf, generation_mode, abort_callback)?
            }
            GenerationType::Random => {
                self.generate_random(workbuf, generation_mode, abort_callback)?
            }
        }
        Ok(())
    }

    // ── generateMixedOrder ────────────────────────────────────────────────

    fn generate_mixed_order(
        &mut self,
        workbuf: &mut Vec<i32>,
        mode: GenerationMode,
        abort: Option<fn() -> bool>,
    ) -> Result<(), GenerationError> {
        for sub_idx in 0..self.submodels.len() {
            let value_count = self.submodels[sub_idx].result_count() as i32;
            let name = build_pseudo_param_name(&self.submodels[sub_idx].params);
            self.last_param_id += 1;
            let seq = self.last_param_id;
            let pseudo = Parameter::new_pseudo(self.order, seq, value_count, sub_idx, name);
            self.params.push(pseudo);
        }

        // Sort by decreasing order (C++ GreaterThanByOrder)
        self.params.sort_by(|a, b| b.order.cmp(&a.order));

        let r1 = self.map_exclusions_to_pseudo_params();
        let r2 = self.exclude_conflicting_param_values();
        if r1 || r2 {
            self.derive_submodel_exclusions(mode, abort);
        }
        self.map_row_seeds_to_pseudo_params();

        let mut combos: Vec<Combination> = Vec::new();
        let mut global_zeros = 0i32;

        // For each distinct order level, from highest to lowest, generate combinations
        let mut start = 0usize;
        while start < self.params.len() {
            let order = self.params[start].order;
            let mut end = start;
            while end < self.params.len() && self.params[end].order == order {
                end += 1;
            }
            let mut base: Vec<ParamId> = Vec::new();
            choose_and_link(
                &mut self.params,
                0,      // first: always start from beginning
                end,    // last: up to end of this order group
                order,
                order,  // real_order
                &mut base,
                &mut combos,
                &mut global_zeros,
            );
            start = end;
        }

        self.gcd(&mut combos, &mut global_zeros, workbuf, mode, abort)
    }

    // ── generateFixedOrder ────────────────────────────────────────────────

    fn generate_fixed_order(
        &mut self,
        workbuf: &mut Vec<i32>,
        mode: GenerationMode,
        abort: Option<fn() -> bool>,
    ) -> Result<(), GenerationError> {
        for sub_idx in 0..self.submodels.len() {
            let value_count = self.submodels[sub_idx].result_count() as i32;
            let name = build_pseudo_param_name(&self.submodels[sub_idx].params);
            self.last_param_id += 1;
            let seq = self.last_param_id;
            let pseudo = Parameter::new_pseudo(self.order, seq, value_count, sub_idx, name);
            self.params.push(pseudo);
        }

        let order = self.order;
        for p in &mut self.params {
            p.order = order;
        }

        let r1 = self.map_exclusions_to_pseudo_params();
        let r2 = self.exclude_conflicting_param_values();
        if r1 || r2 {
            self.derive_submodel_exclusions(mode, abort);
        }
        self.map_row_seeds_to_pseudo_params();

        let mut combos: Vec<Combination> = Vec::new();
        let mut global_zeros = 0i32;
        let n = self.params.len();
        let mut base: Vec<ParamId> = Vec::new();
        choose_and_link(&mut self.params, 0, n, order, order, &mut base, &mut combos, &mut global_zeros);

        self.gcd(&mut combos, &mut global_zeros, workbuf, mode, abort)
    }

    // ── generateFull ──────────────────────────────────────────────────────

    fn generate_full(
        &mut self,
        workbuf: &mut Vec<i32>,
        mode: GenerationMode,
        abort: Option<fn() -> bool>,
    ) -> Result<(), GenerationError> {
        debug_assert_eq!(self.result_param_count(), 0);

        for sub_idx in 0..self.submodels.len() {
            let value_count = self.submodels[sub_idx].result_count() as i32;
            let name = build_pseudo_param_name(&self.submodels[sub_idx].params);
            self.last_param_id += 1;
            let seq = self.last_param_id;
            let pseudo = Parameter::new_pseudo(2, seq, value_count, sub_idx, name);
            self.params.push(pseudo);
        }

        let n = self.params.len() as i32;
        let mut rows: i64 = 1;
        for p in &mut self.params {
            p.order = n;
            rows *= p.value_count as i64;
            if rows > MAX_ROWS_TO_GENERATE {
                return Err(GenerationError::new(ErrorType::TooManyRows));
            }
        }

        let r1 = self.map_exclusions_to_pseudo_params();
        let r2 = self.exclude_conflicting_param_values();
        if r1 || r2 {
            self.derive_submodel_exclusions(mode, abort);
        }
        self.map_row_seeds_to_pseudo_params();

        let mut combos: Vec<Combination> = Vec::new();
        let mut global_zeros = 0i32;
        let np = self.params.len();
        let mut base: Vec<ParamId> = Vec::new();
        choose_and_link(&mut self.params, 0, np, n, n, &mut base, &mut combos, &mut global_zeros);

        self.gcd(&mut combos, &mut global_zeros, workbuf, mode, abort)
    }

    // ── generateRandom ────────────────────────────────────────────────────

    fn generate_random(
        &mut self,
        workbuf: &mut Vec<i32>,
        mode: GenerationMode,
        abort: Option<fn() -> bool>,
    ) -> Result<(), GenerationError> {
        debug_assert_eq!(self.result_param_count(), 0);

        for sub_idx in 0..self.submodels.len() {
            let value_count = self.submodels[sub_idx].result_count() as i32;
            let name = build_pseudo_param_name(&self.submodels[sub_idx].params);
            self.last_param_id += 1;
            let seq = self.last_param_id;
            let pseudo = Parameter::new_pseudo(1, seq, value_count, sub_idx, name);
            self.params.push(pseudo);
        }

        for p in &mut self.params {
            p.order = 1;
        }

        let r1 = self.map_exclusions_to_pseudo_params();
        let r2 = self.exclude_conflicting_param_values();
        if r1 || r2 {
            self.derive_submodel_exclusions(mode, abort);
        }
        self.map_row_seeds_to_pseudo_params();

        let mut combos: Vec<Combination> = Vec::new();
        let mut global_zeros = 0i32;
        let n = self.params.len();
        let mut base: Vec<ParamId> = Vec::new();
        choose_and_link(&mut self.params, 0, n, 1, 1, &mut base, &mut combos, &mut global_zeros);

        self.gcd(&mut combos, &mut global_zeros, workbuf, mode, abort)?;

        if self.max_rows > 0 && self.max_rows < self.results.len() as i64 {
            self.results.truncate(self.max_rows as usize);
        }
        Ok(())
    }

    // ── generateFlat ──────────────────────────────────────────────────────

    fn generate_flat(
        &mut self,
        workbuf: &mut Vec<i32>,
        mode: GenerationMode,
        abort: Option<fn() -> bool>,
    ) -> Result<(), GenerationError> {
        debug_assert_eq!(self.result_param_count(), 0);
        debug_assert!(self.row_seeds.is_empty());

        let max_values = self.params.iter().map(|p| p.value_count).max().unwrap_or(0);

        for cur_val in 0..max_values {
            let seed: RowSeed = self
                .params
                .iter()
                .filter(|p| cur_val < p.value_count)
                .map(|p| (p.global_id, cur_val))
                .collect();
            self.row_seeds.push_back(seed);
        }

        self.order = 1;
        self.generate_fixed_order(workbuf, mode, abort)?;

        if self.max_rows > 0 && self.max_rows < self.results.len() as i64 {
            self.results.truncate(self.max_rows as usize);
        }
        Ok(())
    }

    // ── GenerateVirtualRoot ───────────────────────────────────────────────

    pub fn generate_virtual_root(
        &mut self,
        workbuf: &mut Vec<i32>,
        mode: GenerationMode,
        abort: Option<fn() -> bool>,
    ) -> Result<(), GenerationError> {
        debug_assert_eq!(self.result_param_count(), 0);

        let mut global_zeros = 0i32;

        // Gather unique param global-ids from all submodels
        let mut seen_gids: HashSet<u32> = HashSet::new();
        // Build list: (sub_idx, param_idx_in_sub)
        let mut sub_param_locs: Vec<(usize, usize)> = Vec::new();
        for (sub_idx, sub) in self.submodels.iter().enumerate() {
            for (pidx, p) in sub.params.iter().enumerate() {
                if seen_gids.insert(p.global_id) {
                    sub_param_locs.push((sub_idx, pidx));
                }
            }
        }

        // Clone those params into self.params (without binding state)
        for (sub_idx, pidx) in &sub_param_locs {
            let cloned = self.submodels[*sub_idx].params[*pidx].make_virtual_copy();
            self.params.push(cloned);
        }

        // Create one Combination per submodel; all covered except for output rows
        let mut combos: Vec<Combination> = Vec::new();

        for sub_idx in 0..self.submodels.len() {
            let mut combo = Combination::new();
            let cid = combos.len();

            // Link the submodel's params to this combo
            for sub_pidx in 0..self.submodels[sub_idx].params.len() {
                let sub_gid = self.submodels[sub_idx].params[sub_pidx].global_id;
                if let Some(local_pid) =
                    self.params.iter().position(|p| p.global_id == sub_gid)
                {
                    combo.push_param(local_pid);
                    self.params[local_pid].link_combination(cid);
                }
            }

            let size: i32 = combo.params.iter().map(|&pid| self.params[pid].value_count).product();
            combo.set_map_size(size, COVERED, &mut global_zeros);

            // Mark each submodel result row as OPEN
            for ridx in 0..self.submodels[sub_idx].results.len() {
                let result = &self.submodels[sub_idx].results[ridx];
                let mut index = 0usize;
                for (col, &pid) in combo.params.iter().enumerate() {
                    let local_gid = self.params[pid].global_id;
                    // Find the column in the submodel that corresponds to this param
                    if let Some(sub_col) = self.submodels[sub_idx]
                        .params
                        .iter()
                        .position(|p| p.global_id == local_gid)
                    {
                        let vc = self.params[pid].value_count as usize;
                        // Mixed-radix encoding: most-significant first
                        index = index * vc + result[sub_col];
                    }
                }
                combo.set_open(index as i32, &mut global_zeros);
            }

            combos.push(combo);
        }

        self.gcd(&mut combos, &mut global_zeros, workbuf, mode, abort)
    }

    // ── gcd ───────────────────────────────────────────────────────────────

    fn gcd(
        &mut self,
        combos: &mut Vec<Combination>,
        global_zeros: &mut i32,
        workbuf: &mut Vec<i32>,
        mode: GenerationMode,
        abort: Option<fn() -> bool>,
    ) -> Result<(), GenerationError> {
        if self.params.is_empty() {
            return Ok(());
        }

        self.process_exclusions(combos, global_zeros);
        self.fix_row_seeds();

        let max_range = combos.iter().map(|c| c.range).max().unwrap_or(0);
        if max_range <= 0 {
            return Ok(());
        }
        workbuf.resize(max_range as usize + 1, 0);

        self.total_combinations = *global_zeros as i64;

        while *global_zeros > 0 {
            if let Some(cb) = abort {
                if cb() {
                    return Err(GenerationError::new(ErrorType::GenerationCancelled));
                }
            }

            for p in &mut self.params {
                p.init_binding();
            }
            let mut unbound = self.params.len() as i32;

            for c in combos.iter_mut() {
                c.init_binding();
            }

            let mut worklist = WorkList::new();

            // Apply the front seed row if any
            if !self.row_seeds.is_empty() {
                let seed = self.row_seeds.pop_front().unwrap();
                // Mark all seed params as pending first
                for &(gid, _) in &seed {
                    if let Some(pid) = self.params.iter().position(|p| p.global_id == gid) {
                        self.params[pid].mark_pending();
                    }
                }
                for (gid, val) in seed {
                    if let Some(pid) = self.params.iter().position(|p| p.global_id == gid) {
                        bind_param(&mut self.params, combos, pid, val, &mut worklist, global_zeros);
                        unbound -= 1;
                    }
                }
            }

            if mode == GenerationMode::Approximate {
                let max_tries = 1000usize;
                let mut candidate = self.generate_random_row();
                let mut attempts = 0usize;
                while self.row_violates_exclusion_terms(&candidate) {
                    attempts += 1;
                    if attempts >= max_tries {
                        return Ok(());
                    }
                    candidate = self.generate_random_row();
                }
                for &(gid, _) in &candidate {
                    if let Some(pid) = self.params.iter().position(|p| p.global_id == gid) {
                        self.params[pid].mark_pending();
                    }
                }
                for (gid, val) in candidate {
                    if let Some(pid) = self.params.iter().position(|p| p.global_id == gid) {
                        bind_param(&mut self.params, combos, pid, val, &mut worklist, global_zeros);
                    }
                }
            } else {
                while unbound > 0 {
                    if worklist.is_empty() {
                        let choice = self.choose_combo_idx(combos)?;
                        let val = self.pick_combo_value(combos, choice, workbuf);

                        // Inline the bind: mark pending, then bind each param in reverse
                        let combo_params: Vec<ParamId> = combos[choice].params.clone();
                        for &pid in &combo_params {
                            self.params[pid].mark_pending();
                        }
                        let mut v = val;
                        let mut bound = 0i32;
                        for i in (0..combo_params.len()).rev() {
                            let pid = combo_params[i];
                            let vc = self.params[pid].value_count;
                            if !self.params[pid].bound {
                                bind_param(&mut self.params, combos, pid, v % vc, &mut worklist, global_zeros);
                                bound += 1;
                            }
                            v /= vc;
                        }
                        unbound -= bound;
                    } else {
                        let pid = worklist.get_item();
                        debug_assert!(!self.params[pid].bound);
                        let val = pick_value(&mut self.params, combos, pid, workbuf, mode)?;
                        bind_param(&mut self.params, combos, pid, val, &mut worklist, global_zeros);
                        unbound -= 1;
                    }
                }
            }
        }

        self.remaining_combinations = *global_zeros as i64;

        self.resolve_pseudo_params();

        // Restore original param sequence order
        self.params.sort_by_key(|p| p.sequence);

        // Collect result rows
        if !self.params.is_empty() {
            let result_count = self.params[0].temp_result_count();
            for p in &mut self.params {
                p.get_first();
            }
            for _ in 0..result_count {
                let mut row = ResultRow::new();
                for p in &mut self.params {
                    row.push(p.get_next());
                }
                if mode == GenerationMode::Preview && self.row_violates_exclusion_row(&row) {
                    continue;
                }
                self.results.push(row);
            }
        }

        for p in &mut self.params {
            p.clean_up();
        }

        if self.result_param_count() > 0 {
            self.mark_undefined_values_in_result_params();
        }

        Ok(())
    }

    // ── choose_combo_idx ──────────────────────────────────────────────────

    fn choose_combo_idx(&self, combos: &[Combination]) -> Result<usize, GenerationError> {
        let mut max_zeros = 0i32;
        let mut max_match = 0i32;
        let mut ties = 0i32;
        let mut choice: Option<usize> = None;

        for (cidx, combo) in combos.iter().enumerate() {
            if combo.is_fully_bound() {
                continue;
            }
            let mut open = 0i32;
            let mut matches = 0i32;
            for vidx in 0..combo.range {
                match combo.feasible_value(vidx, &self.params) {
                    ComboStatus::Open => open += 1,
                    ComboStatus::CoveredMatch => matches += 1,
                    ComboStatus::Excluded => {}
                }
            }

            if open > max_zeros {
                choice = Some(cidx);
                ties = 1;
                max_zeros = open;
            } else if open == max_zeros && max_zeros > 0 {
                ties += 1;
                if rng_rand_range(ties) == 0 {
                    choice = Some(cidx);
                }
            } else if max_zeros == 0 && matches > max_match {
                choice = Some(cidx);
                ties = 1;
                max_match = matches;
            } else if max_zeros == 0 && matches > 0 && matches == max_match {
                ties += 1;
                if rng_rand_range(ties) == 0 {
                    choice = Some(cidx);
                }
            }
        }

        choice.ok_or_else(|| GenerationError::new(ErrorType::GenerationFailure))
    }

    fn pick_combo_value(&self, combos: &[Combination], cidx: usize, workbuf: &mut Vec<i32>) -> i32 {
        let combo = &combos[cidx];
        let has_open = (0..combo.range)
            .any(|v| matches!(combo.feasible_value(v, &self.params), ComboStatus::Open));

        if !has_open {
            // No open: pick by weight using weighted random selection
            let mut total_weight = 0i32;
            let mut best = 0i32;
            for vidx in 0..combo.range {
                if combo.feasible_value(vidx, &self.params) != ComboStatus::Excluded {
                    let w = combo.weight(vidx, &self.params);
                    total_weight += w;
                    if total_weight > 0 && rng_rand() % total_weight < w {
                        best = vidx;
                    }
                }
            }
            best
        } else {
            let candidates: Vec<i32> = (0..combo.range)
                .filter(|&v| matches!(combo.feasible_value(v, &self.params), ComboStatus::Open))
                .collect();
            candidates[rng_rand_range(candidates.len() as i32) as usize]
        }
    }

    // ── processExclusions ────────────────────────────────────────────────

    fn process_exclusions(&mut self, combos: &mut Vec<Combination>, global_zeros: &mut i32) {
        // Sort each parameter's combination list by combination id
        let ids: Vec<u32> = combos.iter().map(|c| c.id).collect();
        for p in &mut self.params {
            p.sort_combinations(&|cid| ids[cid]);
        }

        let excl_ids: Vec<_> = self.exclusions.ids().collect();
        for eid in excl_ids {
            let excl = match self.exclusions.get(eid) {
                Some(e) if !e.is_empty() => e.clone(),
                _ => continue,
            };

            // Build intersection of combinations referencing all params in the exclusion
            let first_gid = excl.terms[0].0;
            let first_pid = match self.params.iter().position(|p| p.global_id == first_gid) {
                Some(pid) => pid,
                None => continue,
            };

            let mut intersection: Vec<usize> = self.params[first_pid].combinations.clone();
            for &(gid, _) in &excl.terms[1..] {
                match self.params.iter().position(|p| p.global_id == gid) {
                    Some(pid) => {
                        let other = self.params[pid].combinations.clone();
                        intersection.retain(|cid| other.contains(cid));
                    }
                    None => {
                        intersection.clear();
                        break;
                    }
                }
            }

            if intersection.is_empty() {
                // Create a new combination for this exclusion, fully covered
                let mut new_combo = Combination::new();
                let cid = combos.len();
                let mut size = 1i32;
                for &(gid, _) in &excl.terms {
                    if let Some(pid) = self.params.iter().position(|p| p.global_id == gid) {
                        new_combo.push_param(pid);
                        self.params[pid].link_combination(cid);
                        let updated_ids: Vec<u32> = combos.iter().map(|c| c.id).collect();
                        self.params[pid].sort_combinations(&|c| updated_ids[c]);
                        size *= self.params[pid].value_count;
                    }
                }
                new_combo.set_map_size(size, COVERED, global_zeros);
                intersection.push(cid);
                combos.push(new_combo);
            }

            // Apply all exclusions to each intersecting combo
            let all_excls: Vec<Exclusion> = self.exclusions.iter().cloned().collect();
            for &cid in &intersection {
                for ae in &all_excls {
                    let mut combo = std::mem::replace(
                        &mut combos[cid],
                        Combination::new(),
                    );
                    combo.apply_exclusion(ae, &self.params, global_zeros);
                    combos[cid] = combo;
                }
            }
        }
    }

    // ── generateRandomRow ─────────────────────────────────────────────────

    fn generate_random_row(&self) -> Vec<(u32, i32)> {
        let mut row = Vec::new();
        for p in &self.params {
            let total: i32 = (0..p.value_count).map(|i| p.weight(i)).sum();
            let total = if total == 0 { p.value_count } else { total };
            let idx = rng_rand() % total;
            let mut acc = 0i32;
            let mut val = 0i32;
            for n in 0..p.value_count {
                acc += p.weight(n);
                if acc > idx {
                    val = n;
                    break;
                }
                val = n;
            }
            row.push((p.global_id, val));
        }
        row
    }

    fn row_violates_exclusion_terms(&self, row: &[(u32, i32)]) -> bool {
        self.exclusions.iter().any(|excl| {
            excl.terms
                .iter()
                .all(|&(gid, val)| row.iter().any(|&(g, v)| g == gid && v == val))
        })
    }

    fn row_violates_exclusion_row(&self, row: &ResultRow) -> bool {
        self.exclusions.iter().any(|excl| {
            excl.terms.iter().all(|&(gid, val)| {
                if let Some(p) = self.params.iter().find(|p| p.global_id == gid) {
                    let col = (p.sequence - 1) as usize;
                    col < row.len() && row[col] == val as usize
                } else {
                    false
                }
            })
        })
    }

    // ── fixRowSeeds ───────────────────────────────────────────────────────

    fn fix_row_seeds(&mut self) {
        // Remove terms for params not in this model or out of range
        for seed in &mut self.row_seeds {
            seed.retain(|&(gid, val)| {
                self.params
                    .iter()
                    .any(|p| p.global_id == gid && val >= 0 && val < p.value_count)
            });
        }

        // Remove seeds contained within other seeds
        let mut i = 0;
        while i < self.row_seeds.len() {
            if self.row_seeds[i].is_empty() {
                self.row_seeds.remove(i);
                continue;
            }
            let seed_i = self.row_seeds[i].clone();
            let mut j = 0;
            while j < self.row_seeds.len() {
                if i != j && seed_contained(&seed_i, &self.row_seeds[j]) {
                    self.row_seeds.remove(j);
                    if j < i {
                        i -= 1;
                    }
                } else {
                    j += 1;
                }
            }
            i += 1;
        }

        // Remove seeds that violate exclusions
        let excls: Vec<Exclusion> = self.exclusions.iter().cloned().collect();
        self.row_seeds
            .retain(|seed| !excls.iter().any(|excl| seed_violates_exclusion(seed, excl)));
    }

    // ── resolvePseudoParams ───────────────────────────────────────────────

    fn resolve_pseudo_params(&mut self) {
        let mut i = self.params.len();
        while i > 0 {
            i -= 1;
            let sub_idx = match self.params[i].submodel_idx {
                Some(idx) => idx,
                None => continue,
            };

            let pseudo_results = self.params[i].temp_results.clone();
            let component_count = self.submodels[sub_idx].params.len();

            for comp_idx in 0..component_count {
                let comp_gid = self.submodels[sub_idx].params[comp_idx].global_id;

                // Skip if already present
                if self.params.iter().any(|p| p.global_id == comp_gid) {
                    continue;
                }

                let mut new_param =
                    self.submodels[sub_idx].params[comp_idx].make_virtual_copy();

                // Translate pseudo-results through sub-model result rows
                for &pseudo_val in &pseudo_results {
                    let real_val = self.submodels[sub_idx].results[pseudo_val][comp_idx];
                    new_param.temp_results.push(real_val);
                }

                self.params.push(new_param);
            }

            self.params.remove(i);
        }
    }

    // ── mapExclusionsToPseudoParams ───────────────────────────────────────

    fn map_exclusions_to_pseudo_params(&mut self) -> bool {
        let mut ret = false;
        let n_params = self.params.len();

        for param_idx in 0..n_params {
            let sub_idx = match self.params[param_idx].submodel_idx {
                Some(idx) => idx,
                None => continue,
            };
            let pseudo_gid = self.params[param_idx].global_id;
            let pseudo_vc = self.params[param_idx].value_count;

            let component_gids: Vec<u32> = self.submodels[sub_idx]
                .params
                .iter()
                .map(|p| p.global_id)
                .collect();

            let excl_ids: Vec<_> = self.exclusions.ids().collect();
            let mut new_excls: Vec<Exclusion> = Vec::new();
            let mut to_remove: Vec<crate::types::ExclusionId> = Vec::new();

            for eid in excl_ids {
                let excl = match self.exclusions.get(eid) {
                    Some(e) => e.clone(),
                    None => continue,
                };

                let mut related: Vec<Term> = Vec::new();
                let mut unrelated: Vec<Term> = Vec::new();
                for &term in &excl.terms {
                    if component_gids.contains(&term.0) {
                        related.push(term);
                    } else {
                        unrelated.push(term);
                    }
                }
                if related.is_empty() {
                    continue;
                }

                to_remove.push(eid);

                for vidx in 0..pseudo_vc {
                    let sub_row = &self.submodels[sub_idx].results[vidx as usize];
                    let all_match = related.iter().all(|&(gid, val)| {
                        self.submodels[sub_idx]
                            .params
                            .iter()
                            .position(|p| p.global_id == gid)
                            .map(|ci| sub_row[ci] == val as usize)
                            .unwrap_or(false)
                    });
                    if all_match {
                        let mut new_excl = Exclusion::with_terms(unrelated.clone());
                        new_excl.insert((pseudo_gid, vidx));
                        new_excls.push(new_excl);
                        ret = true;
                    }
                }
            }

            for eid in to_remove {
                self.exclusions.remove(eid);
            }
            for e in new_excls {
                self.exclusions.insert(e);
            }
        }
        ret
    }

    // ── excludeConflictingParamValues ─────────────────────────────────────

    fn exclude_conflicting_param_values(&mut self) -> bool {
        let mut ret = false;
        let n = self.params.len();

        for i in 0..n {
            let sub_i = match self.params[i].submodel_idx {
                Some(idx) => idx,
                None => continue,
            };
            let gid_i = self.params[i].global_id;
            let vc_i = self.params[i].value_count as usize;

            for j in (i + 1)..n {
                let sub_j = match self.params[j].submodel_idx {
                    Some(idx) => idx,
                    None => continue,
                };
                let gid_j = self.params[j].global_id;
                let vc_j = self.params[j].value_count as usize;

                let comps_i: Vec<u32> =
                    self.submodels[sub_i].params.iter().map(|p| p.global_id).collect();
                let comps_j: Vec<u32> =
                    self.submodels[sub_j].params.iter().map(|p| p.global_id).collect();

                for (ci, &shared_gid) in comps_i.iter().enumerate() {
                    let cj = match comps_j.iter().position(|&g| g == shared_gid) {
                        Some(pos) => pos,
                        None => continue,
                    };

                    for v1 in 0..vc_i {
                        for v2 in 0..vc_j {
                            if self.submodels[sub_i].results[v1][ci]
                                != self.submodels[sub_j].results[v2][cj]
                            {
                                let mut excl = Exclusion::new();
                                excl.insert((gid_i, v1 as i32));
                                excl.insert((gid_j, v2 as i32));
                                self.exclusions.insert(excl);
                                ret = true;
                            }
                        }
                    }
                }
            }
        }
        ret
    }

    // ── mapRowSeedsToPseudoParams ─────────────────────────────────────────

    fn map_row_seeds_to_pseudo_params(&mut self) {
        let n_params = self.params.len();
        for param_idx in 0..n_params {
            let sub_idx = match self.params[param_idx].submodel_idx {
                Some(idx) => idx,
                None => continue,
            };
            let pseudo_gid = self.params[param_idx].global_id;
            let pseudo_vc = self.params[param_idx].value_count;

            let component_gids: Vec<u32> = self.submodels[sub_idx]
                .params
                .iter()
                .map(|p| p.global_id)
                .collect();

            let mut new_seeds: Vec<RowSeed> = Vec::new();
            let mut to_remove: Vec<usize> = Vec::new();

            for si in 0..self.row_seeds.len() {
                let mut related: Vec<Term> = Vec::new();
                let mut unrelated: Vec<Term> = Vec::new();
                for &term in &self.row_seeds[si] {
                    if component_gids.contains(&term.0) {
                        related.push(term);
                    } else {
                        unrelated.push(term);
                    }
                }
                if related.is_empty() {
                    continue;
                }

                to_remove.push(si);
                for vidx in 0..pseudo_vc {
                    let sub_row = &self.submodels[sub_idx].results[vidx as usize];
                    let all_match = related.iter().all(|&(gid, val)| {
                        self.submodels[sub_idx]
                            .params
                            .iter()
                            .position(|p| p.global_id == gid)
                            .map(|ci| sub_row[ci] == val as usize)
                            .unwrap_or(false)
                    });
                    if all_match {
                        let mut seed = unrelated.clone();
                        seed.push((pseudo_gid, vidx));
                        new_seeds.push(seed);
                    }
                }
            }

            for idx in to_remove.into_iter().rev() {
                self.row_seeds.remove(idx);
            }
            for s in new_seeds {
                self.row_seeds.push_back(s);
            }
        }
    }

    // ── deriveSubmodelExclusions ──────────────────────────────────────────

    fn derive_submodel_exclusions(&mut self, mode: GenerationMode, abort: Option<fn() -> bool>) {
        let param_ids: Vec<ParamId> = (0..self.params.len()).collect();
        let mut params = std::mem::take(&mut self.params);
        let mut excls = std::mem::take(&mut self.exclusions);

        {
            let mut deriver = ExclusionDeriver::new(&mut params, mode, abort);
            for &pid in &param_ids {
                deriver.add_param(pid);
            }
            let excl_ids: Vec<_> = excls.ids().collect();
            for eid in excl_ids {
                if let Some(e) = excls.get(eid) {
                    let _ = deriver.add_exclusion(e.clone(), true);
                }
            }
            deriver.derive_exclusions();
            excls = deriver.take_exclusions();
        }

        self.params = params;
        self.exclusions = excls;
    }

    // ── markUndefinedValuesInResultParams ─────────────────────────────────

    fn mark_undefined_values_in_result_params(&mut self) {
        let result_cols: Vec<(usize, i32, u32)> = self
            .params
            .iter()
            .filter(|p| p.is_result_param)
            .map(|p| ((p.sequence - 1) as usize, p.value_count, p.global_id))
            .collect();

        if result_cols.is_empty() {
            return;
        }

        let n_rows = self.results.len();
        let mut excluded: Vec<Vec<HashSet<i32>>> =
            vec![vec![HashSet::new(); n_rows]; result_cols.len()];

        let excls: Vec<Exclusion> = self.exclusions.iter().cloned().collect();

        for (row_idx, row) in self.results.iter().enumerate() {
            for excl in &excls {
                let rc = excl.result_param_count(&|gid| {
                    self.params.iter().any(|p| p.global_id == gid && p.is_result_param)
                });
                if rc > 1 {
                    continue;
                }

                let matches = excl.terms.iter().all(|&(gid, val)| {
                    if let Some(p) = self.params.iter().find(|p| p.global_id == gid) {
                        if p.is_result_param {
                            return true;
                        }
                        let col = (p.sequence - 1) as usize;
                        col < row.len() && row[col] == val as usize
                    } else {
                        false
                    }
                });

                if matches {
                    for (rp_pos, &(_, _, rgid)) in result_cols.iter().enumerate() {
                        for &(gid, val) in &excl.terms {
                            if gid == rgid {
                                excluded[rp_pos][row_idx].insert(val);
                            }
                        }
                    }
                }
            }
        }

        for (rp_pos, &(col, vc, _)) in result_cols.iter().enumerate() {
            for row_idx in 0..n_rows {
                if excluded[rp_pos][row_idx].len() as i32 != vc - 1 {
                    self.results[row_idx][col] = UNDEFINED_VALUE;
                }
            }
        }
    }
}

// ── Parameter helper ──────────────────────────────────────────────────────────

impl Parameter {
    /// Create a copy without binding/combination state, for use in virtual root generation.
    pub fn make_virtual_copy(&self) -> Self {
        Parameter {
            global_id: self.global_id,
            sequence: self.sequence,
            name: self.name.clone(),
            order: self.order,
            value_count: self.value_count,
            is_result_param: self.is_result_param,
            bound: false,
            pending: false,
            current_value: 0,
            combinations: Vec::new(),
            exclusion_back_refs: Vec::new(),
            avg_exclusion_size: 0.0,
            temp_results: Vec::new(),
            result_iter: 0,
            weights: self.weights.clone(),
            submodel_idx: self.submodel_idx,
        }
    }
}

// ── choose_and_link ───────────────────────────────────────────────────────────

/// Recursively generates all N-way combinations from `params[first..last]`,
/// creating Combination objects and linking parameters to them.
///
/// Matches C++ `Model::choose`: at the top level (`order == real_order`) only
/// parameters whose `order` field equals `order` can start a combination;
/// once we recurse (`order < real_order`) all parameters are eligible.
pub fn choose_and_link(
    params: &mut Vec<Parameter>,
    first: usize,
    last: usize, // exclusive
    order: i32,
    real_order: i32,
    base: &mut Vec<ParamId>,
    combos: &mut Vec<Combination>,
    global_zeros: &mut i32,
) {
    debug_assert!(order >= 0 && order <= real_order);
    debug_assert!(real_order > 0);

    if order == 0 {
        let cid = combos.len();
        let mut combo = Combination::new();
        for &pid in base.iter() {
            combo.push_param(pid);
        }
        let size: i32 = base.iter().map(|&pid| params[pid].value_count).product();
        combo.set_map_size(size, OPEN, global_zeros);
        for &pid in base.iter() {
            params[pid].link_combination(cid);
        }
        combos.push(combo);
        return;
    }

    if (last as isize - first as isize) < order as isize {
        return;
    }

    // Mirror C++ while loop: decrement last before checking order condition
    let mut end = last;
    loop {
        let distance = end as isize - first as isize;
        if distance < order as isize {
            break;
        }
        end -= 1; // --last in C++

        // Condition: param's order must match, OR we're in a recursive call
        if params[end].order != order && order == real_order {
            // Top-level condition failed: stop iterating (C++ exits while loop)
            break;
        }

        base.push(end);
        choose_and_link(params, first, end, order - 1, real_order, base, combos, global_zeros);
        base.pop();
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn build_pseudo_param_name(params: &[Parameter]) -> String {
    params
        .iter()
        .map(|p| p.name.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn seed_contained(container: &RowSeed, containee: &RowSeed) -> bool {
    containee.iter().all(|t| container.contains(t))
}

fn seed_violates_exclusion(seed: &RowSeed, excl: &Exclusion) -> bool {
    excl.terms
        .iter()
        .all(|&(gid, val)| seed.iter().any(|&(sg, sv)| sg == gid && sv == val))
}
