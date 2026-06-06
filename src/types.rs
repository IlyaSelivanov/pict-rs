pub type ParamId = usize;
pub type CombId = usize;
pub type ExclusionId = u32;

/// Index 0 in combination bitvec is unused sentinel
pub const UNDEFINED_ID: u32 = 0;
/// Pseudo-parameters start their sequence counter here so they sort after real params
pub const PSEUDO_PARAM_SEQ_BASE: u32 = 1_000_000;
pub const MAX_ROWS_TO_GENERATE: i64 = 1_000_000;

pub const OPEN: u8 = 0x00;
pub const COVERED: u8 = 0x01;
pub const EXCLUDED: u8 = 0xFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComboStatus {
    Open,
    CoveredMatch,
    Excluded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationType {
    MixedOrder,
    FixedOrder,
    Full,
    Flat,
    Random,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationMode {
    Regular,
    Preview,
    Approximate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorType {
    GenerationCancelled,
    TooManyRows,
    GenerationFailure,
    OutOfMemory,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct GenerationError {
    pub error_type: ErrorType,
}

impl GenerationError {
    pub fn new(error_type: ErrorType) -> Self {
        Self { error_type }
    }
}

/// A single row of results — one value-index per parameter
pub type ResultRow = Vec<usize>;
pub type ResultCollection = Vec<ResultRow>;

/// A seed or exclusion term: (param_global_id, value_index)
pub type Term = (u32, i32);

/// A seed row: map from param_global_id to value_index
pub type RowSeed = Vec<Term>; // kept sorted by param_global_id
pub type RowSeedCollection = std::collections::VecDeque<RowSeed>;
