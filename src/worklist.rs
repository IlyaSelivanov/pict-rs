use std::collections::VecDeque;
use crate::types::ParamId;
use crate::parameter::Parameter;

/// Work queue of parameters that need a value assignment.
pub struct WorkList {
    rep: VecDeque<ParamId>,
}

impl Default for WorkList {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkList {
    pub fn new() -> Self {
        Self {
            rep: VecDeque::new(),
        }
    }

    /// Add `pid` to the worklist if not already pending.
    pub fn add_item(&mut self, params: &mut Vec<Parameter>, pid: ParamId) {
        if !params[pid].pending {
            debug_assert!(!params[pid].bound);
            self.rep.push_back(pid);
            params[pid].pending = true;
        }
    }

    pub fn get_item(&mut self) -> ParamId {
        self.rep.pop_front().expect("WorkList::get_item called on empty list")
    }

    pub fn is_empty(&self) -> bool {
        self.rep.is_empty()
    }
}
