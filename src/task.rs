use crate::combination::reset_combo_counter;
use crate::deriver::ExclusionDeriver;
use crate::exclusion::{Exclusion, ExclusionCollection};
use crate::model::Model;
use crate::types::{
    GenerationError, ErrorType, GenerationMode, ParamId,
    ResultCollection, RowSeed, RowSeedCollection,
};

pub struct Task {
    pub root_model: Option<Box<Model>>,
    pub exclusions: ExclusionCollection,
    pub row_seeds: RowSeedCollection,
    pub generation_mode: GenerationMode,
    pub max_random_tries: usize,
    pub abort_callback: Option<fn() -> bool>,
    pub workbuf: Vec<i32>,
    current_result_row: usize,
}

impl Default for Task {
    fn default() -> Self {
        Self::new()
    }
}

impl Task {
    pub fn new() -> Self {
        reset_combo_counter();
        Self {
            root_model: None,
            exclusions: ExclusionCollection::new(),
            row_seeds: RowSeedCollection::new(),
            generation_mode: GenerationMode::Regular,
            max_random_tries: 1000,
            abort_callback: None,
            workbuf: Vec::new(),
            current_result_row: 0,
        }
    }

    pub fn set_root_model(&mut self, model: Box<Model>) {
        self.root_model = Some(model);
    }

    pub fn add_exclusion(&mut self, excl: Exclusion) -> bool {
        let (_, inserted) = self.exclusions.insert(excl);
        inserted
    }

    pub fn add_row_seed(&mut self, seed: RowSeed) {
        self.row_seeds.push_back(seed);
    }

    pub fn total_parameter_count(&self) -> usize {
        self.root_model
            .as_ref()
            .map(|m| m.total_param_count())
            .unwrap_or(0)
    }

    // ── PrepareForGeneration ──────────────────────────────────────────────

    pub fn prepare_for_generation(&mut self) -> Result<(), GenerationError> {
        if self.root_model.is_none() {
            return Err(GenerationError::new(ErrorType::GenerationFailure));
        }

        // 1. Derive task-level exclusions (needs &mut self)
        self.derive_exclusions()?;

        // 2. Propagate each exclusion to the most specific model in the hierarchy.
        //    Clone exclusions to avoid borrow conflict between self.exclusions and self.root_model.
        let excl_clones: Vec<Exclusion> = self.exclusions.iter().cloned().collect();
        if let Some(root) = &mut self.root_model {
            for excl in &excl_clones {
                add_exclusion_to_model(root, excl);
            }
        }

        // 3. Propagate row seeds
        let seeds: Vec<RowSeed> = self.row_seeds.iter().cloned().collect();
        if let Some(root) = &mut self.root_model {
            root.add_row_seeds(seeds.into_iter());
        }

        Ok(())
    }

    fn derive_exclusions(&mut self) -> Result<(), GenerationError> {
        let root = match &mut self.root_model {
            Some(m) => m,
            None => return Ok(()),
        };

        let param_ids: Vec<ParamId> = (0..root.params.len()).collect();
        let mut params = std::mem::take(&mut root.params);
        let mut excls = std::mem::take(&mut self.exclusions);

        {
            let mode = self.generation_mode;
            let abort = self.abort_callback;
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

        root.params = params;
        self.exclusions = excls;
        Ok(())
    }

    // ── Generate ──────────────────────────────────────────────────────────

    pub fn generate(&mut self) -> Result<(), GenerationError> {
        self.prepare_for_generation()?;

        let mode = self.generation_mode;
        let abort = self.abort_callback;

        if let Some(root) = &mut self.root_model {
            generate_recursive(root, &mut self.workbuf, mode, abort)?;
        }

        Ok(())
    }

    // ── Result access ─────────────────────────────────────────────────────

    pub fn results(&self) -> Option<&ResultCollection> {
        self.root_model.as_ref().map(|m| &m.results)
    }

    pub fn reset_result_fetching(&mut self) {
        self.current_result_row = 0;
    }

    /// Fill `buf` with the next result row. Returns the number of rows remaining
    /// *before* this call (matching the C++ semantics: non-zero means data was written).
    pub fn get_next_result_row(&mut self, buf: &mut [usize]) -> usize {
        let results = match &self.root_model {
            Some(m) => &m.results,
            None => return 0,
        };

        let remaining = results.len().saturating_sub(self.current_result_row);
        if self.current_result_row < results.len() {
            let row = &results[self.current_result_row];
            for (i, &v) in row.iter().enumerate() {
                if i < buf.len() {
                    buf[i] = v;
                }
            }
            self.current_result_row += 1;
        }
        remaining
    }
}

// ── Recursive generation ──────────────────────────────────────────────────────

pub fn generate_recursive(
    model: &mut Model,
    workbuf: &mut Vec<i32>,
    mode: GenerationMode,
    abort: Option<fn() -> bool>,
) -> Result<(), GenerationError> {
    // Generate all submodels first
    for i in 0..model.submodels.len() {
        let mut sub = std::mem::replace(
            &mut model.submodels[i],
            Box::new(Model::new(
                String::new(),
                crate::types::GenerationType::MixedOrder,
                0,
                0,
            )),
        );
        generate_recursive(&mut sub, workbuf, mode, abort)?;
        model.submodels[i] = sub;
    }
    model.generate(workbuf, mode, abort)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn find_param_in_subtree(gid: u32, model: &Model) -> bool {
    if model.params.iter().any(|p| p.global_id == gid) {
        return true;
    }
    model.submodels.iter().any(|sub| find_param_in_subtree(gid, sub))
}

fn add_exclusion_to_model(model: &mut Model, excl: &Exclusion) {
    // Try the deepest matching submodel first
    for sub in &mut model.submodels {
        let has_all = excl
            .terms
            .iter()
            .all(|&(gid, _)| find_param_in_subtree(gid, sub));
        if has_all {
            add_exclusion_to_model(sub, excl);
            return;
        }
    }
    model.add_exclusion(excl.clone());
}
