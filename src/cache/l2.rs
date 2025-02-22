use crate::sync::Arc;
use crate::{address, cache, config, interconn as ic, mem_fetch};
use color_eyre::eyre;
use mem_fetch::access::Kind as AccessKind;
use std::collections::VecDeque;

pub struct L2DataCacheController<MC> {
    accelsim_compat: bool,
    mem_controller: Arc<MC>,
    linear_set_index_function: cache::set_index::linear::SetIndex,
    ipoly_set_index_function: cache::set_index::ipoly::SetIndex,
    pseudo_random_set_index_function: cache::set_index::pascal::L2PseudoRandomSetIndex,
    cache_config: cache::Config,
}

/// Clone derive macro does not work with structs containing generic
/// types that don't implement clone themselves
/// https://github.com/rust-lang/rust/issues/41481
impl<MC> Clone for L2DataCacheController<MC> {
    fn clone(&self) -> L2DataCacheController<MC> {
        L2DataCacheController {
            accelsim_compat: self.accelsim_compat,
            mem_controller: self.mem_controller.clone(),
            linear_set_index_function: self.linear_set_index_function.clone(),
            ipoly_set_index_function: self.ipoly_set_index_function.clone(),
            pseudo_random_set_index_function: self.pseudo_random_set_index_function.clone(),
            cache_config: self.cache_config.clone(),
        }
    }
}

impl<MC> L2DataCacheController<MC> {
    pub fn new(config: &config::Cache, mem_controller: Arc<MC>, accelsim_compat: bool) -> Self {
        let cache_config = cache::Config::new(config, accelsim_compat);
        let linear_set_index_function = cache::set_index::linear::SetIndex::new(
            cache_config.num_sets,
            cache_config.line_size as usize,
        );
        let ipoly_set_index_function = cache::set_index::ipoly::SetIndex::new(
            cache_config.num_sets,
            cache_config.line_size as usize,
        );

        let pseudo_random_set_index_function =
            cache::set_index::pascal::L2PseudoRandomSetIndex::default();

        Self {
            accelsim_compat,
            mem_controller,
            cache_config,
            pseudo_random_set_index_function,
            ipoly_set_index_function,
            linear_set_index_function,
        }
    }
}

impl<MC> cache::CacheController for L2DataCacheController<MC>
where
    MC: crate::mcu::MemoryController,
{
    fn tag(&self, addr: address) -> address {
        // For generality, the tag includes both index and tag.
        // This allows for more complex set index calculations that
        // can result in different indexes mapping to the same set,
        // thus the full tag + index is required to check for hit/miss.
        // Tag is now identical to the block address.

        // return addr >> (m_line_sz_log2+m_nset_log2);
        // return addr & ~(new_addr_type)(m_line_sz - 1);

        // The tag lookup is at line size (128B) granularity.
        // clear the last log2(line_size = 128) bits
        addr & !address::from(self.cache_config.line_size - 1)

        // self.cache_controller.tag(addr)
    }

    fn block_addr(&self, addr: address) -> address {
        self.tag(addr)
        // self.cache_controller.block_addr(addr)
    }

    fn set_index(&self, addr: address) -> u64 {
        use cache::set_index::SetIndexer;
        if self.accelsim_compat {
            let partition_addr = self.mem_controller.memory_partition_address(addr);
            self.ipoly_set_index_function
                .compute_set_index(partition_addr)
            // self.linear_set_index_function
            //     .compute_set_index(partition_addr)
        } else {
            // let partition_addr = self.mem_controller.memory_partition_address(addr);

            // let set_index = self
            //     .pseudo_random_set_index_function
            //     .compute_set_index(addr);

            // let sub_partition = self.mem_controller.to_physical_address(addr);
            // let sub_partition_size = self.cache_config.associativity*16
            let partition_addr = self.mem_controller.memory_partition_address(addr);
            // let set_index = self
            //     .linear_set_index_function
            //     .compute_set_index(partition_addr);
            let set_index = self
                .ipoly_set_index_function
                .compute_set_index(partition_addr);

            // let partition_addr = addr;
            // let set_index = self
            //     .pseudo_random_set_index_function
            //     .compute_set_index(partition_addr);

            debug_assert!(set_index < self.cache_config.num_sets as u64);
            set_index
        }

        // let partition_addr = if true || self.accelsim_compat {
        //     self.memory_controller.memory_partition_address(addr)
        // } else {
        //     addr
        // };
        // // println!("partition address for addr {} is {}", addr, partition_addr);
        // self.cache_controller.set_index(partition_addr)
    }

    fn set_bank(&self, _addr: address) -> u64 {
        // not banked
        0
    }

    fn mshr_addr(&self, addr: address) -> address {
        // TODO: ROMAN changed to block size as well, such that we maybe overfetch
        // addr & !address::from(self.cache_config.line_size - 1)
        addr & !address::from(self.cache_config.atom_size - 1)
        // self.cache_controller.mshr_addr(addr)
    }
}

/// Generic data cache.
#[allow(clippy::module_name_repetitions)]
pub struct DataL2<MC> {
    pub cache_config: Arc<config::L2DCache>,
    pub inner: super::data::Data<MC, L2DataCacheController<MC>>,
}

impl<MC> DataL2<MC>
where
    MC: crate::mcu::MemoryController,
{
    pub fn new(
        name: String,
        sub_partition_id: usize,
        config: Arc<config::GPU>,
        mem_controller: Arc<MC>,
        l2_cache_config: Arc<config::L2DCache>,
    ) -> Self {
        let cache_controller = L2DataCacheController::new(
            l2_cache_config.inner.as_ref(),
            mem_controller.clone(),
            config.accelsim_compat,
        );
        let inner = super::data::Builder {
            name,
            id: sub_partition_id,
            kind: cache::base::Kind::OffChip,
            config,
            cache_controller,
            mem_controller,
            cache_config: l2_cache_config.inner.clone(),
            write_alloc_type: AccessKind::L2_WR_ALLOC_R,
            write_back_type: AccessKind::L2_WRBK_ACC,
        }
        .build();
        Self {
            inner,
            cache_config: l2_cache_config,
        }
    }
}

impl<MC> super::Cache for DataL2<MC>
where
    MC: crate::mcu::MemoryController,
{
    fn cycle(
        &mut self,
        top_port: &mut dyn ic::Connection<ic::Packet<mem_fetch::MemFetch>>,
        cycle: u64,
    ) {
        self.inner.cycle(top_port, cycle);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn controller(&self) -> &dyn cache::CacheController {
        &self.inner.inner.cache_controller
    }

    fn line_size(&self) -> usize {
        self.inner.inner.cache_config.line_size as usize
    }

    fn write_state(
        &self,
        csv_writer: &mut csv::Writer<std::io::BufWriter<std::fs::File>>,
    ) -> eyre::Result<()> {
        self.inner.inner.tag_array.write_state(csv_writer)
    }

    fn write_allocate_policy(&self) -> cache::config::WriteAllocatePolicy {
        self.inner.write_allocate_policy()
    }

    fn has_ready_accesses(&self) -> bool {
        self.inner.has_ready_accesses()
    }

    fn ready_accesses(&self) -> Option<&VecDeque<mem_fetch::MemFetch>> {
        self.inner.ready_accesses()
    }

    fn pop_next_ready_access(&mut self) -> Option<mem_fetch::MemFetch> {
        self.inner.pop_next_ready_access()
    }

    /// Access read only cache.
    ///
    /// returns `RequestStatus::RESERVATION_FAIL` if
    /// request could not be accepted (for any reason)
    fn access(
        &mut self,
        addr: address,
        fetch: mem_fetch::MemFetch,
        events: &mut Vec<super::event::Event>,
        time: u64,
    ) -> super::RequestStatus {
        self.inner.access(addr, fetch, events, time)
    }

    fn waiting_for_fill(&self, fetch: &mem_fetch::MemFetch) -> bool {
        self.inner.waiting_for_fill(fetch)
    }

    fn fill(&mut self, fetch: mem_fetch::MemFetch, time: u64) {
        self.inner.fill(fetch, time);
    }

    fn flush(&mut self) -> usize {
        self.inner.flush()
    }

    fn invalidate(&mut self) {
        self.inner.invalidate();
    }

    fn invalidate_addr(&mut self, addr: address) {
        self.inner.invalidate_addr(addr);
    }

    fn num_used_lines(&self) -> usize {
        self.inner.inner.tag_array.num_used_lines()
    }

    fn num_used_bytes(&self) -> u64 {
        self.inner.inner.tag_array.num_used_lines() as u64
            * self.inner.inner.cache_config.line_size as u64
    }

    fn num_total_lines(&self) -> usize {
        self.inner.inner.tag_array.num_total_lines()
    }
}

impl<MC> cache::ComputeStats for DataL2<MC> {
    fn per_kernel_stats(&self) -> &stats::cache::PerKernel {
        &self.inner.inner.stats
    }

    fn per_kernel_stats_mut(&mut self) -> &mut stats::cache::PerKernel {
        &mut self.inner.inner.stats
    }
}

impl<MC> super::Bandwidth for DataL2<MC> {
    fn has_free_data_port(&self) -> bool {
        self.inner.has_free_data_port()
    }

    fn has_free_fill_port(&self) -> bool {
        self.inner.has_free_fill_port()
    }
}

#[cfg(test)]
mod tests {
    // use crate::cache::CacheController;
    // use crate::sync::Arc;
    // use color_eyre::eyre;
    //
    // #[test]
    // fn test_l2d_set_index() -> eyre::Result<()> {
    //     let accelsim_compat = false;
    //     let config = crate::config::GPU::default();
    //     let l2_cache_config = &config.data_cache_l2.as_ref().unwrap().inner;
    //
    //     // create l2 data cache controller
    //     let memory_controller = Arc::new(crate::mcu::MemoryControllerUnit::new(&config)?);
    //     // let cache_controller = crate::cache::controller::pascal::DataCacheController::new(
    //     //     crate::cache::Config::new(l2_cache_config.as_ref(), accelsim_compat),
    //     // );
    //     let cache_config = crate::cache::Config::new(l2_cache_config.as_ref(), accelsim_compat);
    //     let pseudo_random_set_index_function =
    //         crate::cache::set_index::pascal::L2PseudoRandomSetIndex::default();
    //
    //     let linear_set_index_function = crate::cache::set_index::linear::SetIndex::new(
    //         cache_config.num_sets,
    //         cache_config.line_size as usize,
    //     );
    //     let linear_set_index_function = crate::cache::set_index::linear::SetIndex::new(
    //         cache_config.num_sets,
    //         cache_config.line_size as usize,
    //     );
    //
    //
    //     let l2_cache_controller = super::L2DataCacheController {
    //         accelsim_compat: false,
    //         memory_controller,
    //         cache_config,
    //         linear_set_index_function,
    //         ipoly_set_index_function,
    //         pseudo_random_set_index_function,
    //     };
    //
    //     let block_addr = 34_887_082_112;
    //     assert_eq!(l2_cache_controller.set_index(block_addr), 1);
    //     Ok(())
    // }
}
