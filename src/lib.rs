#![allow(dead_code)]

mod types;
#[cfg(test)]
mod tests;
mod exclusion;
mod trie;
mod parameter;
mod combination;
mod worklist;
mod deriver;
mod model;
mod task;

use std::ffi::c_void;

use exclusion::Exclusion;
use model::Model;
use parameter::Parameter;
use task::Task;
use types::GenerationType;

// ── Public constants ──────────────────────────────────────────────────────────
pub const PICT_SUCCESS: u32 = 0x0000_0000;
pub const PICT_OUT_OF_MEMORY: u32 = 0xC000_0001;
pub const PICT_GENERATION_ERROR: u32 = 0xC000_0002;

pub const PICT_PAIRWISE_GENERATION: u32 = 2;
pub const PICT_DEFAULT_RANDOM_SEED: u32 = 0;

// ── Caller-facing item structs ────────────────────────────────────────────────

#[repr(C)]
pub struct PictExclusionItem {
    /// Parameter handle (the global_id cast to *const c_void)
    pub parameter: *const c_void,
    pub value_index: usize,
}

#[repr(C)]
pub struct PictSeedItem {
    pub parameter: *const c_void,
    pub value_index: usize,
}

// ── Handle helpers ────────────────────────────────────────────────────────────

/// The parameter handle is the parameter's globally-unique u32 ID, cast to a pointer.
/// This is stable regardless of Vec reallocations.
fn param_gid_from_handle(h: *const c_void) -> u32 {
    h as usize as u32
}

fn param_gid_to_handle(gid: u32) -> *mut c_void {
    gid as usize as *mut c_void
}

unsafe fn task_mut(h: *const c_void) -> &'static mut Task {
    &mut *(h as *mut Task)
}

unsafe fn model_mut(h: *const c_void) -> &'static mut Model {
    &mut *(h as *mut Model)
}

// ─────────────────────────────────────────────────────────────────────────────
// PictCreateTask
// ─────────────────────────────────────────────────────────────────────────────
#[no_mangle]
pub extern "C" fn PictCreateTask() -> *mut c_void {
    Box::into_raw(Box::new(Task::new())) as *mut c_void
}

// ─────────────────────────────────────────────────────────────────────────────
// PictSetRootModel
// ─────────────────────────────────────────────────────────────────────────────
#[no_mangle]
pub unsafe extern "C" fn PictSetRootModel(task: *const c_void, model: *const c_void) {
    let task = task_mut(task);
    let model = Box::from_raw(model as *mut Model);
    task.set_root_model(model);
}

// ─────────────────────────────────────────────────────────────────────────────
// PictAddExclusion
// ─────────────────────────────────────────────────────────────────────────────
#[no_mangle]
pub unsafe extern "C" fn PictAddExclusion(
    task: *const c_void,
    items: *const PictExclusionItem,
    item_count: usize,
) -> u32 {
    let task = task_mut(task);
    let items = std::slice::from_raw_parts(items, item_count);

    let mut excl = Exclusion::new();
    for item in items {
        let gid = param_gid_from_handle(item.parameter);
        excl.insert((gid, item.value_index as i32));
    }
    task.add_exclusion(excl);
    PICT_SUCCESS
}

// ─────────────────────────────────────────────────────────────────────────────
// PictAddSeed
// ─────────────────────────────────────────────────────────────────────────────
#[no_mangle]
pub unsafe extern "C" fn PictAddSeed(
    task: *const c_void,
    items: *const PictSeedItem,
    item_count: usize,
) -> u32 {
    let task = task_mut(task);
    let items = std::slice::from_raw_parts(items, item_count);

    let mut seed = types::RowSeed::new();
    for item in items {
        let gid = param_gid_from_handle(item.parameter);
        seed.push((gid, item.value_index as i32));
    }
    seed.sort_by_key(|t| t.0);
    task.add_row_seed(seed);
    PICT_SUCCESS
}

// ─────────────────────────────────────────────────────────────────────────────
// PictGenerate
// ─────────────────────────────────────────────────────────────────────────────
#[no_mangle]
pub unsafe extern "C" fn PictGenerate(task: *const c_void) -> u32 {
    let task = task_mut(task);
    match task.generate() {
        Ok(()) => PICT_SUCCESS,
        Err(e) => match e.error_type {
            types::ErrorType::OutOfMemory => PICT_OUT_OF_MEMORY,
            _ => PICT_GENERATION_ERROR,
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PictAllocateResultBuffer / PictFreeResultBuffer
// ─────────────────────────────────────────────────────────────────────────────

/// Buffer layout: [count, v0, v1, ..., v_{count-1}]
/// We store the count as the first element so PictFreeResultBuffer can recover it.
/// The returned pointer points to v0 (index 1 in the Vec).
#[no_mangle]
pub unsafe extern "C" fn PictAllocateResultBuffer(task: *const c_void) -> *mut usize {
    let task = task_mut(task);
    let count = task.total_parameter_count();
    if count == 0 {
        return std::ptr::null_mut();
    }
    let mut v: Vec<usize> = Vec::with_capacity(count + 1);
    v.push(count); // sentinel at index 0
    v.extend(std::iter::repeat(0).take(count));
    debug_assert_eq!(v.len(), count + 1);
    let ptr = v.as_mut_ptr();
    std::mem::forget(v);
    // Return pointer to first data element (skip sentinel)
    ptr.add(1)
}

#[no_mangle]
pub unsafe extern "C" fn PictFreeResultBuffer(buf: *mut usize) {
    if buf.is_null() {
        return;
    }
    // Recover the original Vec by stepping back one element to the sentinel
    let base = buf.sub(1);
    let count = *base;
    drop(Vec::from_raw_parts(base, count + 1, count + 1));
}

// ─────────────────────────────────────────────────────────────────────────────
// PictResetResultFetching
// ─────────────────────────────────────────────────────────────────────────────
#[no_mangle]
pub unsafe extern "C" fn PictResetResultFetching(task: *const c_void) {
    task_mut(task).reset_result_fetching();
}

// ─────────────────────────────────────────────────────────────────────────────
// PictGetNextResultRow
// ─────────────────────────────────────────────────────────────────────────────
#[no_mangle]
pub unsafe extern "C" fn PictGetNextResultRow(
    task: *const c_void,
    result_row: *mut usize,
) -> usize {
    let task = task_mut(task);
    let count = task.total_parameter_count();
    if count == 0 {
        return 0;
    }
    let buf = std::slice::from_raw_parts_mut(result_row, count);
    task.get_next_result_row(buf)
}

// ─────────────────────────────────────────────────────────────────────────────
// PictDeleteTask
// ─────────────────────────────────────────────────────────────────────────────
#[no_mangle]
pub unsafe extern "C" fn PictDeleteTask(task: *const c_void) {
    drop(Box::from_raw(task as *mut Task));
}

// ─────────────────────────────────────────────────────────────────────────────
// PictCreateModel
// ─────────────────────────────────────────────────────────────────────────────
#[no_mangle]
pub extern "C" fn PictCreateModel(random_seed: u32) -> *mut c_void {
    let model = Box::new(Model::new(
        String::new(),
        GenerationType::MixedOrder,
        0,
        random_seed as u64,
    ));
    Box::into_raw(model) as *mut c_void
}

// ─────────────────────────────────────────────────────────────────────────────
// PictAddParameter
// ─────────────────────────────────────────────────────────────────────────────
/// Returns the parameter's global_id cast to a pointer — this is the stable handle.
#[no_mangle]
pub unsafe extern "C" fn PictAddParameter(
    model: *const c_void,
    value_count: usize,
    order: u32,
    value_weights: *const u32,
) -> *mut c_void {
    let model = model_mut(model);
    let sequence = (model.params.len() + 1) as i32;
    let mut param = Parameter::new(order as i32, sequence, value_count as i32, String::new(), false);

    if !value_weights.is_null() {
        let ws = std::slice::from_raw_parts(value_weights, value_count);
        param.set_weights(ws.iter().map(|&w| w as i32).collect());
    }

    let gid = param.global_id;
    model.add_parameter(param);
    param_gid_to_handle(gid)
}

// ─────────────────────────────────────────────────────────────────────────────
// PictGetTotalParameterCount
// ─────────────────────────────────────────────────────────────────────────────
#[no_mangle]
pub unsafe extern "C" fn PictGetTotalParameterCount(task: *const c_void) -> usize {
    task_mut(task).total_parameter_count()
}

// ─────────────────────────────────────────────────────────────────────────────
// PictAttachChildModel
// ─────────────────────────────────────────────────────────────────────────────
#[no_mangle]
pub unsafe extern "C" fn PictAttachChildModel(
    parent: *const c_void,
    child: *const c_void,
    order: u32,
) -> u32 {
    let parent = model_mut(parent);
    let mut child_model = *Box::from_raw(child as *mut Model);
    child_model.set_order(order as i32);
    if parent.order < order as i32 {
        parent.set_order(order as i32);
    }
    parent.add_submodel(Box::new(child_model));
    PICT_SUCCESS
}

// ─────────────────────────────────────────────────────────────────────────────
// PictDeleteModel
// ─────────────────────────────────────────────────────────────────────────────
/// Deletes the model and all its submodels. Parameters do not need separate deletion
/// (they are owned by the model's params Vec).
#[no_mangle]
pub unsafe extern "C" fn PictDeleteModel(model: *const c_void) {
    drop(Box::from_raw(model as *mut Model));
}
