use crate::{core, instruction};
use bitvec::array::BitArray;
use std::collections::HashMap;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum Kind {
    Sync,
    Arrive,
    Reduction,
}

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarrierSet {
    max_blocks_per_core: usize,
    max_barriers_per_block: usize,
    warp_size: usize,
    warps_per_block: HashMap<u64, core::WarpMask>,
    bar_id_to_warps: Box<[core::WarpMask]>,
    active_warps: core::WarpMask,
    warps_at_barrier: core::WarpMask,
}

#[derive(Debug, Clone)]
pub struct Builder {
    pub max_blocks_per_core: usize,
    pub max_barriers_per_block: usize,
    pub warp_size: usize,
}

impl Builder {
    #[must_use]
    pub fn build(self) -> BarrierSet {
        BarrierSet {
            max_blocks_per_core: self.max_blocks_per_core,
            max_barriers_per_block: self.max_barriers_per_block,
            warp_size: self.warp_size,
            active_warps: BitArray::ZERO,
            warps_at_barrier: BitArray::ZERO,
            warps_per_block: HashMap::new(),
            bar_id_to_warps: utils::box_slice![BitArray::ZERO; self.max_barriers_per_block],
        }
    }
}

pub trait Barrier: std::fmt::Debug + Sync + Send + 'static {
    /// Check whether warp is waiting for barrier
    #[must_use]
    fn is_waiting_at_barrier(&self, warp_id: usize) -> bool;

    /// Allocate a new barrier for a thread block.
    fn allocate(&mut self, block_id: u64, warps: core::WarpMask);

    /// Deallocate barrier for a thread block.
    ///
    /// This should be called once the block completes.
    fn deallocate(&mut self, block_id: u64);

    /// Warp exited and can unblock barrier.
    fn warp_exited(&mut self, warp_id: usize);

    /// Warp reached barrier.
    fn warp_reached_barrier(&mut self, block_id: u64, instr: &instruction::WarpInstruction);
}

impl Barrier for BarrierSet {
    fn is_waiting_at_barrier(&self, warp_id: usize) -> bool {
        self.warps_at_barrier[warp_id]
    }

    fn allocate(&mut self, block_id: u64, warps: core::WarpMask) {
        assert!(block_id < self.max_blocks_per_core as u64);
        assert!(
            !self.warps_per_block.contains_key(&block_id),
            "block should not already be active"
        );

        self.warps_per_block.insert(block_id, warps);
        assert!(
            self.warps_per_block.len() <= self.max_blocks_per_core,
            "no blocks that were not properly deallocated"
        );

        self.active_warps |= warps;
        self.warps_at_barrier &= !warps;
        for bar_id in 0..self.max_barriers_per_block {
            self.bar_id_to_warps[bar_id] &= !warps;
        }
    }

    fn deallocate(&mut self, block_id: u64) {
        let Some(warps_in_block) = self.warps_per_block.remove(&block_id) else {
            return;
        };

        let at_barrier = warps_in_block & self.warps_at_barrier;
        assert!(!at_barrier.any(), "no warps stuck at barrier");

        let active = warps_in_block & self.active_warps;

        assert!(!active.any(), "no warps in block are still running");
        self.active_warps &= !warps_in_block;
        self.warps_at_barrier &= !warps_in_block;

        for bar_id in 0..self.max_barriers_per_block {
            let at_a_specific_barrier = warps_in_block & self.bar_id_to_warps[bar_id];
            assert!(!at_a_specific_barrier.any(), "no warps stuck at barrier");
            self.bar_id_to_warps[bar_id] &= !warps_in_block;
        }
    }

    fn warp_exited(&mut self, warp_id: usize) {
        // caller needs to verify all threads in warp are done,
        // e.g., by checking PDOM stack to see it has only one
        // entry during exit_impl()
        self.active_warps.set(warp_id, false);

        // test for barrier release
        let Some(warps_in_block) = self.warps_per_block.values().find(|w| w[warp_id]) else {
            return;
        };
        let active = *warps_in_block & self.active_warps;

        for bar_id in 0..self.max_barriers_per_block {
            let at_a_specific_barrier = *warps_in_block & self.bar_id_to_warps[bar_id];
            if at_a_specific_barrier == active {
                // all warps have reached barrier,
                // so release waiting warps...
                self.bar_id_to_warps[bar_id] &= !at_a_specific_barrier;
                self.warps_at_barrier &= !at_a_specific_barrier;
            }
        }
    }

    fn warp_reached_barrier(&mut self, block_id: u64, instr: &instruction::WarpInstruction) {
        let warps_in_block = self
            .warps_per_block
            .get(&block_id)
            .copied()
            .expect("block not found in barrier set");

        assert!(warps_in_block[instr.warp_id], "warp is in the block");

        let Some(ref bar) = instr.barrier else {
            panic!("bar instruction has no barrier info");
        };

        self.bar_id_to_warps[bar.id].set(instr.warp_id, true);

        match bar.kind {
            Kind::Sync | Kind::Reduction => {
                self.warps_at_barrier.set(instr.warp_id, true);
            }
            Kind::Arrive => {}
        }

        let at_barrier = warps_in_block & self.bar_id_to_warps[bar.id];
        let active = warps_in_block & self.active_warps;
        match bar.count {
            Some(count) => {
                // TODO: check on the hardware if the count
                // should include warp that exited
                if at_barrier.count_ones() * self.warp_size == count {
                    // warps have reached barrier, so release waiting warps
                    self.bar_id_to_warps[bar.id] &= !at_barrier;
                    self.warps_at_barrier &= !at_barrier;
                    if bar.kind == Kind::Reduction {
                        todo!("bar reduciton");
                    }
                }
            }
            None => {
                if at_barrier == active {
                    // all warps have reached barrier,
                    // so release waiting warps...
                    self.bar_id_to_warps[bar.id] &= !at_barrier;
                    self.warps_at_barrier &= !at_barrier;
                    if bar.kind == Kind::Reduction {
                        todo!("bar reduciton");
                    }
                }
            }
        }
    }
}
