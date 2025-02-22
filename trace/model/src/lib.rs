pub mod active_mask;
pub mod allocation;
pub mod command;
pub mod device;
pub mod dim;

pub use active_mask::{colorize_bits, ActiveMask, ToBitString};
pub use allocation::MemAllocation;
pub use command::Command;
pub use device::Properties as DeviceProperties;
pub use dim::{Dim, Point};

use serde::{Deserialize, Serialize};

/// Start address of the global heap on device.
///
/// Note that the precise value is not actually very important.
/// However, the goal is to split addresses into different memory spaces to avoid clashes.
/// Moreover, requests to zero addresses are treated as no request.
pub const DEVICE_GLOBAL_HEAP_START_ADDR: u64 = 0xC000_0000;

pub const KB: u64 = 1024;

pub const SHARED_MEM_SIZE_MAX: u64 = 96 * KB;
pub const MAX_SM_COUNT: u64 = 80;
pub const TOTAL_SHARED_MEM: u64 = MAX_SM_COUNT * SHARED_MEM_SIZE_MAX;
pub const DEVICE_SHARED_MEM_START_ADDR: u64 = DEVICE_GLOBAL_HEAP_START_ADDR - TOTAL_SHARED_MEM;

/// Warp size.
///
/// Number of threads per warp.
pub const WARP_SIZE: usize = 32;

/// An instruction operand predicate.
#[derive(
    Debug, Default, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct Predicate {
    /// Predicate number
    pub num: usize,
    /// Whether predicate is negated (i.e. @!P0).
    pub is_neg: bool,
    /// Whether predicate is uniform predicate (e.g., @UP0).
    pub is_uniform: bool,
}

/// Identifier of GPU memory space.
#[derive(
    Debug,
    Clone,
    Copy,
    Hash,
    strum::EnumCount,
    strum::EnumIter,
    strum::FromRepr,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
)]
#[repr(usize)]
pub enum MemorySpace {
    None,
    Local,
    Generic,
    Global,
    Shared,
    Constant,
    GlobalToShared,
    Surface,
    Texture,
}

impl MemorySpace {
    pub fn base_addr(&self) -> u64 {
        match self {
            Self::None => todo!(),
            Self::Local => todo!(),
            Self::Generic => todo!(),
            Self::Global => DEVICE_GLOBAL_HEAP_START_ADDR,
            Self::Shared => DEVICE_SHARED_MEM_START_ADDR,
            Self::Constant => todo!(),
            Self::GlobalToShared => todo!(),
            Self::Surface => todo!(),
            Self::Texture => todo!(),
        }
    }

    #[must_use]
    // #[inline]
    pub const fn count() -> usize {
        use strum::EnumCount;
        Self::COUNT
    }

    #[must_use]
    // #[inline]
    pub fn iter() -> <Self as strum::IntoEnumIterator>::Iterator {
        <Self as strum::IntoEnumIterator>::iter()
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct MemAccessTraceEntry {
    pub cuda_ctx: u64,
    pub device_id: u32,
    pub sm_id: u32,
    pub kernel_id: u64,
    pub block_id: Dim,
    pub warp_id_in_sm: u32,
    pub warp_id_in_block: u32,
    pub warp_size: u32,
    pub line_num: u32,
    #[deprecated]
    #[allow(dead_code)]
    pub instr_data_width: u32,
    pub instr_opcode: String,
    pub instr_offset: u32,
    pub instr_idx: u32,
    pub instr_predicate: Predicate,
    pub instr_mem_space: MemorySpace,
    pub instr_is_mem: bool,
    pub instr_is_load: bool,
    pub instr_is_store: bool,
    pub instr_is_extended: bool,
    pub dest_regs: [u32; 1],
    pub num_dest_regs: u32,
    pub src_regs: [u32; 5],
    pub num_src_regs: u32,
    // pub active_mask: u32,
    pub active_mask: ActiveMask,
    /// Accessed address per thread of a warp.
    ///
    /// We use u64 to capture the full 64bit addressing space.
    /// Instead of using i64 and encoding "no address" as -1, we encode as 0.
    /// We note that addresses are split into different memory adress spaces,
    /// which means that accesses to address 0 should generally not occur.
    pub addrs: [u64; 32],
    pub thread_indices: [(u32, u32, u32); 32],
}

impl MemAccessTraceEntry {
    #[allow(clippy::match_same_arms)]
    #[must_use]
    pub fn is_memory_instruction(&self) -> bool {
        let is_exit = self.instr_opcode.to_uppercase() == "EXIT";
        let is_barrier = self.instr_opcode.to_uppercase() == "MEMBAR";
        self.instr_is_mem || self.instr_is_store || self.instr_is_load || is_exit || is_barrier
    }

    pub fn valid_addresses(&self) -> impl Iterator<Item = u64> + '_ {
        self.addrs
            .iter()
            .enumerate()
            .filter_map(|(tid, addr)| {
                if self.is_memory_instruction() && self.active_mask[tid] {
                    Some(addr)
                } else {
                    None
                }
            })
            .copied()
    }

    pub fn source_registers(&self) -> &[u32] {
        &self.src_regs[0..self.num_src_regs as usize]
    }

    pub fn set_source_registers(&mut self, regs: impl IntoIterator<Item = u32>) {
        self.num_src_regs = 0;
        self.src_regs.fill(0);
        for (reg_id, reg) in regs.into_iter().enumerate() {
            self.src_regs[reg_id] = reg;
            self.num_src_regs += 1;
        }
    }

    pub fn dest_registers(&self) -> &[u32] {
        &self.dest_regs[0..self.num_dest_regs as usize]
    }

    pub fn set_dest_registers(&mut self, regs: impl IntoIterator<Item = u32>) {
        self.num_dest_regs = 0;
        self.dest_regs.fill(0);
        for (reg_id, reg) in regs.into_iter().enumerate() {
            self.dest_regs[reg_id] = reg;
            self.num_dest_regs += 1;
        }
    }
}

impl std::cmp::Ord for MemAccessTraceEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let key = (
            &self.cuda_ctx,
            &self.kernel_id,
            &self.block_id,
            &self.warp_id_in_block,
        );
        let other_key = (
            &other.cuda_ctx,
            &other.kernel_id,
            &other.block_id,
            &other.warp_id_in_block,
        );
        key.cmp(&other_key)
    }
}

impl std::cmp::PartialOrd for MemAccessTraceEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

pub type WarpTraces = indexmap::IndexMap<(Dim, u32), Vec<MemAccessTraceEntry>>;

#[derive(Debug, Default, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(transparent)]
pub struct MemAccessTrace(pub Vec<MemAccessTraceEntry>);

#[derive(thiserror::Error, Debug)]
pub enum ValidationError {
    #[error("duplicate blocks in trace: {0:?}")]
    DuplicateBlocks(Vec<(u64, dim::Dim)>),
    #[error("duplicate warp ids in trace: {0:?}")]
    DuplicateWarpIds(Vec<(u64, dim::Dim, u32)>),
}

impl MemAccessTrace {
    #[must_use]
    // #[inline]
    pub fn check_valid(&self) -> Result<(), ValidationError> {
        is_valid_trace(&self.0)
    }

    #[must_use]
    // #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    // #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    // #[inline]
    pub fn iter(&self) -> std::slice::Iter<MemAccessTraceEntry> {
        self.0.iter()
    }

    #[must_use]
    // #[inline]
    pub fn to_warp_traces(self) -> WarpTraces {
        let mut warp_traces = WarpTraces::new();
        for entry in self.0 {
            warp_traces
                .entry((entry.block_id.clone(), entry.warp_id_in_block))
                .or_default()
                .push(entry);
        }
        warp_traces
    }
}

impl std::ops::Deref for MemAccessTrace {
    type Target = Vec<MemAccessTraceEntry>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Sanity checks a trace for _any_ valid sorting.
///
/// In order to pass, a trace must not contain any non-consecutive duplicate block or warp ids.
#[must_use]
// #[inline]
pub fn is_valid_trace(trace: &[MemAccessTraceEntry]) -> Result<(), ValidationError> {
    use itertools::Itertools;

    let duplicate_blocks: Vec<_> = trace
        .iter()
        .map(|t| (t.kernel_id, t.block_id.clone()))
        .dedup()
        .duplicates()
        .collect();
    if !duplicate_blocks.is_empty() {
        return Err(ValidationError::DuplicateBlocks(duplicate_blocks));
    }
    let duplicate_warp_ids: Vec<_> = trace
        .iter()
        .map(|t| (t.kernel_id, t.block_id.clone(), t.warp_id_in_block))
        .dedup()
        .duplicates()
        .collect();

    if !duplicate_warp_ids.is_empty() {
        return Err(ValidationError::DuplicateWarpIds(duplicate_warp_ids));
    }
    Ok(())

    // assert_eq!(duplicate_blocks, 0);
    // assert_eq!(duplicate_warp_ids, 0);
    // duplicate_blocks == 0 && duplicate_warp_ids == 0
}
