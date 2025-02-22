use crate::{
    address, barrier,
    cache::{self, Cache, ComputeStats},
    config, func_unit as fu,
    func_unit::SimdFunctionUnit,
    instruction::WarpInstruction,
    interconn as ic,
    kernel::Kernel,
    mem_fetch, opcodes,
    operand_collector::{self, RegisterFileUnit, Writeback},
    register_set, scheduler,
    scheduler::ScheduleWarps,
    scoreboard::{self, Access as ScoreboardAccess},
    warp, WARP_SIZE,
};

use barrier::Barrier;
use bitvec::{array::BitArray, BitArr};
use color_eyre::eyre;
use console::style;
use ic::SharedConnection;
use mem_fetch::access::Kind as AccessKind;
use register_set::Access as RegisterSetAccess;
use smallvec::SmallVec;
use std::collections::VecDeque;
use std::sync::{atomic, Arc};
use strum::IntoEnumIterator;
use trace_model::ToBitString;

pub type WarpMask = BitArr!(for crate::MAX_WARPS_PER_CTA);

// pub mod debug {
//     // use crate::sync::Mutex;
//
//     // pub static NUM_ISSUE_BLOCK: once_cell::sync::Lazy<Mutex<usize>> =
//     //     once_cell::sync::Lazy::new(|| Mutex::new(0));
//
//     //     use std::collections::HashMap;
//     //
//     //     pub struct CompletedBlock {
//     //         pub global_core_id: usize,
//     //         pub kernel_id: u64,
//     //         pub block: trace_model::Point,
//     //     }
//     //
//     //     pub static COMPLETED_BLOCKS: once_cell::sync::Lazy<Mutex<Vec<CompletedBlock>>> =
//     //         once_cell::sync::Lazy::new(|| Mutex::new(Vec::new()));
//     //
//     //     pub static ACCESSES: once_cell::sync::Lazy<
//     //         Mutex<HashMap<(usize, stats::mem::AccessKind), u64>>,
//     //     > = once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));
// }

#[derive(Debug)]
pub struct ThreadState {
    pub active: bool,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum InstructionFetchBufferState {
    Valid { warp_id: usize },
    Invalid,
}

impl InstructionFetchBufferState {
    pub fn is_valid(&self) -> bool {
        matches!(self, Self::Valid { .. })
    }

    pub fn set_invalid(&mut self) {
        *self = InstructionFetchBufferState::Invalid;
    }

    pub fn set_valid(&mut self, warp_id: usize) {
        *self = InstructionFetchBufferState::Valid { warp_id };
    }
}

type ResultBus = BitArr!(for fu::MAX_ALU_LATENCY);

pub trait WarpIssuer: std::fmt::Debug {
    fn issue_warp(
        &mut self,
        stage: PipelineStage,
        warp: &mut warp::Warp,
        scheduler_id: usize,
        cycle: u64,
    ) -> eyre::Result<()>;

    fn has_free_register(&self, stage: PipelineStage, scheduler_id: usize) -> bool;

    fn has_collision(&self, warp_id: usize, instr: &WarpInstruction) -> bool;

    #[must_use]
    fn warp_waiting_at_barrier(&self, warp_id: usize) -> bool;

    #[must_use]
    fn warp_waiting_at_mem_barrier(&self, warp_id: &warp::Warp) -> bool;
}

#[derive()]
pub struct CoreIssuer<'a, S> {
    pub config: &'a config::GPU,
    pub pipeline_reg: &'a mut [register_set::RegisterSet],
    pub warp_instruction_unique_uid: &'a atomic::AtomicU64,
    pub allocations: &'a super::allocation::Allocations,
    pub stats: &'a mut stats::PerKernel,
    pub global_core_id: usize,
    pub thread_block_size: usize,
    pub max_blocks_per_core: usize,
    pub scoreboard: &'a mut S,
    pub barriers: &'a mut barrier::BarrierSet,
}

impl<'a, S> std::fmt::Debug for CoreIssuer<'a, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreIssuer").finish()
    }
}

impl<'a, S> WarpIssuer for CoreIssuer<'a, S>
where
    S: scoreboard::Access,
{
    fn has_free_register(&self, stage: PipelineStage, scheduler_id: usize) -> bool {
        let pipeline_stage = &self.pipeline_reg[stage as usize];

        if self.config.sub_core_model {
            pipeline_stage
                .get(scheduler_id)
                .map(Option::as_ref)
                .flatten()
                .is_none()
        } else {
            pipeline_stage.has_free()
        }
    }

    fn has_collision(&self, warp_id: usize, instr: &WarpInstruction) -> bool {
        self.scoreboard.has_collision(warp_id, instr)
    }

    #[tracing::instrument(name = "core_issue_warp")]
    fn issue_warp(
        &mut self,
        stage: PipelineStage,
        warp: &mut warp::Warp,
        scheduler_id: usize,
        cycle: u64,
    ) -> eyre::Result<()> {
        let pipeline_stage = &mut self.pipeline_reg[stage as usize];
        let pipeline_stage_copy = pipeline_stage.clone();
        let free = if self.config.sub_core_model {
            pipeline_stage.get_free_sub_core_mut(scheduler_id)
        } else {
            pipeline_stage.get_free_mut()
        };
        let (reg_idx, pipe_reg) = free.ok_or(eyre::eyre!("no free register"))?;

        let mut next_instr = warp.instr_buffer.take().unwrap();
        warp.instr_buffer.step();

        log::debug!(
            "{} by scheduler {} to pipeline[{:?}][{}] {}",
            style(format!(
                "cycle {:02} issue {} for warp {}",
                cycle, next_instr, warp.warp_id
            ))
            .yellow(),
            scheduler_id,
            stage,
            reg_idx,
            pipeline_stage_copy,
        );

        // this sets all the info for the warp instruction in pipe reg
        next_instr.uid = self
            .warp_instruction_unique_uid
            .fetch_add(1, atomic::Ordering::SeqCst);

        next_instr.warp_id = warp.warp_id;
        next_instr.issue_cycle = Some(cycle);
        next_instr.dispatch_delay_cycles = next_instr.initiation_interval;
        next_instr.scheduler_id = Some(scheduler_id);

        let mut pipe_reg_mut = next_instr;

        debug_assert_eq!(warp.warp_id, pipe_reg_mut.warp_id);

        for t in 0..self.config.warp_size {
            if pipe_reg_mut.active_mask[t] {
                let warp_id = pipe_reg_mut.warp_id;
                let thread_id = self.config.warp_size * warp_id + t;

                if pipe_reg_mut.is_atomic() {
                    todo!("atomics");
                    // warp.inc_n_atomic();
                }

                if pipe_reg_mut.memory_space == Some(super::instruction::MemorySpace::Local)
                    && (pipe_reg_mut.is_load() || pipe_reg_mut.is_store())
                {
                    let total_cores =
                        self.config.num_simt_clusters * self.config.num_cores_per_simt_cluster;

                    let translated_local_addresses = translate_local_memaddr(
                        pipe_reg_mut.threads[t].mem_req_addr[0],
                        self.global_core_id,
                        thread_id,
                        total_cores,
                        pipe_reg_mut.data_size,
                        self.thread_block_size,
                        self.max_blocks_per_core,
                        &self.config,
                    );

                    debug_assert!(
                        translated_local_addresses.len()
                            < super::instruction::MAX_ACCESSES_PER_THREAD_INSTRUCTION
                    );
                    pipe_reg_mut.set_addresses(t, translated_local_addresses);
                }

                if pipe_reg_mut.opcode.category == opcodes::ArchOp::EXIT_OPS {
                    warp.set_thread_completed(t);
                }
            }
        }

        // here, we generate memory acessess
        if pipe_reg_mut.is_load() || pipe_reg_mut.is_store() {
            crate::timeit!(
                "core::issue::generate_mem_accesses",
                pipe_reg_mut.generate_mem_accesses(&self.config, &self.allocations)
            );
            // pipe_reg_mut.mem_access_queue.extend(accesses.into_iter());

            // if let Some(accesses) = crate::timeit!(
            //     "core::issue::generate_mem_accesses",
            //     pipe_reg_mut.generate_mem_accesses(&self.config, &self.allocations)
            // ) {
            //     for access in accesses {
            //         if let AccessKind::LOCAL_ACC_W | AccessKind::LOCAL_ACC_R = access.kind {
            //             panic!("have local access!");
            //         }
            //         if matches!(
            //             access.kind,
            //             AccessKind::L1_WRBK_ACC
            //                 | AccessKind::L2_WRBK_ACC
            //                 | AccessKind::INST_ACC_R
            //                 | AccessKind::L1_WR_ALLOC_R
            //                 | AccessKind::L2_WR_ALLOC_R
            //         ) {
            //             panic!(
            //                 "generated {:?} access from instruction {}",
            //                 &access.kind, &pipe_reg_mut
            //             );
            //         }
            //
            //         log::trace!(
            //             "generate_mem_accesses: adding access {} to instruction {}",
            //             &access,
            //             &pipe_reg_mut
            //         );
            //         pipe_reg_mut.mem_access_queue.push_back(access);
            //     }
            // }

            log::trace!(
                "generated mem accesses: {:?}",
                pipe_reg_mut
                    .mem_access_queue
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            );
        }

        let pipe_reg_ref = pipe_reg_mut;

        log::debug!(
            "{} (done={} ({}/{}), functional done={}, hardware done={}, stores done={} ({} stores), instr in pipeline = {}, active_threads={})",
            style(format!("checking if warp {} did exit", warp.warp_id)).yellow(),
            warp.done(),
            warp.trace_pc,
            warp.instruction_count(),
            warp.functional_done(),
            warp.hardware_done(),
            warp.stores_done(),
            warp.num_outstanding_stores,
            warp.num_instr_in_pipeline,
            warp.active_mask.count_ones(),
        );

        if warp.done() && warp.functional_done() {
            warp.num_instr_in_pipeline -= warp.instr_buffer.flush();
            self.barriers.warp_exited(pipe_reg_ref.warp_id);
        }

        if pipe_reg_ref.opcode.category == opcodes::ArchOp::BARRIER_OP {
            self.barriers
                .warp_reached_barrier(warp.block_id, &pipe_reg_ref);
        } else if pipe_reg_ref.opcode.category == opcodes::ArchOp::MEMORY_BARRIER_OP {
            warp.waiting_for_memory_barrier = true;
        }

        log::debug!(
            "{} ({:?}) for instr {}",
            style(format!(
                "reserving {} registers",
                pipe_reg_ref.outputs().count()
            ))
            .yellow(),
            pipe_reg_ref.outputs().collect::<Vec<_>>(),
            pipe_reg_ref
        );

        self.scoreboard.reserve_all(&pipe_reg_ref);

        let kernel_stats = self.stats.get_mut(Some(pipe_reg_ref.kernel_launch_id));
        *kernel_stats
            .instructions
            .opcodes
            .entry(pipe_reg_ref.opcode.to_string())
            .or_insert(0) += 1;

        *pipe_reg = Some(pipe_reg_ref);

        // log::debug!(
        //     "post issue register set of {:?} pipeline: {}",
        //     stage,
        //     pipeline_stage
        // );
        Ok(())
    }

    #[must_use]
    fn warp_waiting_at_barrier(&self, warp_id: usize) -> bool {
        self.barriers.is_waiting_at_barrier(warp_id)
    }

    #[must_use]
    fn warp_waiting_at_mem_barrier(&self, warp: &warp::Warp) -> bool {
        if !warp.waiting_for_memory_barrier {
            return false;
        }
        let has_pending_writes = !self.scoreboard.pending_writes(warp.warp_id).is_empty();

        // warp.waiting_for_memory_barrier = has_pending_writes;
        has_pending_writes

        // TODO: PERF roman this requires mutable access but should be fast
        // if has_pending_writes {
        //     true
        // } else {
        //     warp.waiting_for_memory_barrier = false;
        //     if self.config.flush_l1_cache {
        //         // Mahmoud fixed this on Nov 2019
        //         // Invalidate L1 cache
        //         // Based on Nvidia Doc, at MEM barrier, we have to
        //         //(1) wait for all pending writes till they are acked
        //         //(2) invalidate L1 cache to ensure coherence and avoid reading stall data
        //         *self.need_l1_flush.lock() = true;
        //         // todo!("cache invalidate");
        //         // self.cache_invalidate();
        //         // TO DO: you need to stall the SM for 5k cycles.
        //     }
        //     false
        // }
    }
}

#[derive(strum::EnumIter, strum::EnumCount, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum PipelineStage {
    /// Instruction Decode -> Operand Collector stage for single precision unit
    ID_OC_SP = 0,
    /// Instruction Decode -> Operand Collector stage for double precision unit
    ID_OC_DP = 1,
    /// Instruction Decode -> Operand Collector stage for integer unit
    ID_OC_INT = 2,
    /// Instruction Decode -> Operand Collector stage for special function unit
    ID_OC_SFU = 3,
    /// Instruction Decode -> Operand Collector stage for load store unit
    ID_OC_MEM = 4,
    /// Operand Collector -> Execution stage for single precision unit
    OC_EX_SP = 5,
    /// Operand Collector -> Execution stage for double precision unit
    OC_EX_DP = 6,
    /// Operand Collector -> Execution stage for integer precision unit
    OC_EX_INT = 7,
    /// Operand Collector -> Execution stage for special function unit
    OC_EX_SFU = 8,
    /// Operand Collector -> Execution stage for load store unit
    OC_EX_MEM = 9,
    /// Execution -> Writeback stage
    EX_WB = 10,
    /// Instruction Decode -> Operand Collector stage for tensor unit
    ID_OC_TENSOR_CORE = 11,
    /// Operand Collector -> Execution stage for tensor unit
    OC_EX_TENSOR_CORE = 12,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FetchResponseTarget {
    LoadStoreUnit,
    ICache,
}

type InterconnBuffer<T> = VecDeque<ic::Packet<(usize, T, u32)>>;

#[allow(clippy::module_name_repetitions)]
pub struct CoreMemoryConnection<C> {
    pub config: Arc<config::GPU>,
    pub stats: stats::PerKernel,
    pub cluster_id: usize,
    pub buffer: C,
}

#[allow(clippy::missing_fields_in_debug)]
impl<C> std::fmt::Debug for CoreMemoryConnection<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreMemoryConnection").finish()
    }
}

impl<C> ic::Connection<ic::Packet<mem_fetch::MemFetch>> for CoreMemoryConnection<C>
where
    C: ic::BufferedConnection<ic::Packet<(usize, mem_fetch::MemFetch, u32)>>,
{
    fn can_send(&self, packets: &[u32]) -> bool {
        // let request_size: u32 = packets
        //     .iter()
        //     .map(|fetch| {
        //         if fetch.is_write() {
        //             fetch.size()
        //         } else {
        //             u32::from(mem_fetch::READ_PACKET_SIZE)
        //         }
        //     })
        //     .sum();
        // true
        self.buffer.can_send(packets)
        // self.interconn.has_buffer(self.cluster_id, request_size)
    }

    fn send(&mut self, packet: ic::Packet<mem_fetch::MemFetch>) {
        let ic::Packet { mut fetch, time } = packet;

        let access_kind = fetch.access_kind();
        debug_assert_eq!(fetch.is_write(), access_kind.is_write());
        let kernel_stats = self.stats.get_mut(fetch.kernel_launch_id());
        kernel_stats
            .accesses
            .inc(fetch.allocation_id(), access_kind, 1);

        let dest_sub_partition_id = fetch.sub_partition_id();
        let mem_dest = self.config.mem_id_to_device_id(dest_sub_partition_id);

        log::debug!(
            "cluster {} icnt_inject_request_packet({}) dest sub partition id={} dest mem node={}",
            self.cluster_id,
            fetch,
            dest_sub_partition_id,
            mem_dest
        );

        // The packet size varies depending on the type of request:
        // - For write request and atomic request, packet contains the data
        // - For read request (i.e. not write nor atomic), packet only has control metadata
        let packet_size = if fetch.is_write() || fetch.is_atomic() {
            fetch.size()
        } else {
            fetch.control_size()
        };
        // m_stats->m_outgoing_traffic_stats->record_traffic(mf, packet_size);
        fetch.status = mem_fetch::Status::IN_ICNT_TO_MEM;

        fetch.inject_cycle.get_or_insert(time);

        self.buffer.send(ic::Packet {
            fetch: (mem_dest, fetch, packet_size),
            time,
        });
    }

    fn receive(&mut self) -> Option<ic::Packet<mem_fetch::MemFetch>> {
        let ic::Packet {
            fetch: (_, fetch, _),
            time,
        } = self.buffer.receive()?;
        Some(ic::Packet { fetch, time })
    }
}

/// SIMT Core.
#[derive()]
pub struct Core<I, MC> {
    pub global_core_id: usize,
    pub local_core_id: usize,
    pub cluster_id: usize,
    pub config: Arc<config::GPU>,

    /// Core statistics per kernel.
    ///
    /// Stats are private so that consumers use the stats() method,
    /// which aggregates stats for the functional units and mem port.
    stats: stats::PerKernel,

    // state
    pub current_kernel: Option<Arc<dyn Kernel>>,
    pub warps: Box<[warp::Warp]>,
    pub instr_fetch_buffer_state: InstructionFetchBufferState,
    pub instr_fetch_response_queue: super::cluster::ResponseQueue,
    pub active_threads_per_hardware_block: Box<[usize]>,
    pub block_ids_per_hardware_block: Box<[Option<trace_model::Point>]>,

    pub active_thread_mask: BitArr!(for crate::MAX_THREADS_PER_SM),
    pub occupied_hw_thread_ids: BitArr!(for crate::MAX_THREADS_PER_SM),
    pub num_active_blocks: usize,
    pub num_active_warps: usize,
    pub num_active_threads: usize,
    pub num_occupied_threads: usize,
    pub last_warp_fetched: usize,

    // per core unique warp id
    pub dynamic_warp_id: usize,
    pub warp_instruction_unique_uid: Arc<atomic::AtomicU64>,

    // memory and caches
    pub allocations: Arc<super::allocation::Allocations>,
    pub instr_l1_cache: cache::ReadOnly,
    pub load_store_unit: fu::LoadStoreUnit<MC>,
    pub mem_port: CoreMemoryConnection<InterconnBuffer<mem_fetch::MemFetch>>,

    // components
    pub interconn: Arc<I>,
    pub scoreboard: scoreboard::Scoreboard,
    pub mem_controller: Arc<MC>,
    pub barriers: barrier::BarrierSet,
    pub register_file: Box<dyn operand_collector::RegisterFileUnitTrait>,
    // pub register_file: RegisterFileUnit,
    /// The register set for each
    pub pipeline_reg: Box<[register_set::RegisterSet]>,
    pub result_busses: Box<[ResultBus]>,
    pub functional_units: Vec<Box<dyn fu::SimdFunctionUnit>>,

    pub schedulers: Box<[scheduler::gto::Scheduler]>,
    pub scheduler_issue_priority: usize,
    // Custom callback handler that is called when a fetch is returned to its issuer.
    // pub fetch_return_callback: Option<Box<dyn Fn(u64, &mem_fetch::MemFetch) + Send + Sync>>,
}

#[allow(clippy::missing_fields_in_debug)]
impl<I, MC> std::fmt::Debug for Core<I, MC> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Core")
            .field("global_core_id", &self.global_core_id)
            .field("cluster_id", &self.cluster_id)
            .finish()
    }
}

impl<I, MC> Core<I, MC>
where
    I: ic::Interconnect<ic::Packet<mem_fetch::MemFetch>>,
    MC: crate::mcu::MemoryController,
{
    pub fn new(
        global_core_id: usize,
        local_core_id: usize,
        cluster_id: usize,
        instr_fetch_response_queue: super::cluster::ResponseQueue,
        load_store_response_queue: super::cluster::ResponseQueue,
        allocations: Arc<super::allocation::Allocations>,
        warp_instruction_unique_uid: Arc<atomic::AtomicU64>,
        interconn: Arc<I>,
        config: Arc<config::GPU>,
        mem_controller: Arc<MC>,
    ) -> Self {
        assert_eq!(config.max_threads_per_core, crate::MAX_THREADS_PER_SM);

        let stats = stats::PerKernel::new(config.as_ref().into());

        let warps: Box<[_]> = (0..config.max_warps_per_core())
            .map(|_| warp::Warp::default())
            .collect();

        // assert!(std::mem::size_of::<warp::Warp>() <= 256);
        // panic!(
        //     "size of warp: {} bytes -> alignment is {} bytes",
        //     std::mem::size_of::<warp::Warp>(),
        //     std::mem::align_of_val(&warps[0])
        // );

        let mem_port = CoreMemoryConnection {
            cluster_id,
            stats: stats.clone(),
            config: Arc::clone(&config),
            buffer: InterconnBuffer::<mem_fetch::MemFetch>::new(),
        };
        // let mem_port = Arc::new(Mutex::new(mem_port));

        let instr_l1_cache = cache::ReadOnly::new(
            global_core_id,
            format!(
                "core-{}-{}-{}",
                cluster_id,
                global_core_id,
                style("READONLY-INSTR-CACHE").green(),
            ),
            cache::base::Kind::OnChip,
            config.inst_cache_l1.as_ref().unwrap().clone(),
            config.accelsim_compat,
        );
        // instr_l1_cache.set_top_port(mem_port.clone());
        // let instr_l1_cache = Box::new(instr_l1_cache);

        debug_assert_eq!(config.max_warps_per_core(), warps.len());
        let scoreboard = scoreboard::Scoreboard::new(&scoreboard::Config {
            global_core_id,
            cluster_id,
            max_warps: warps.len(),
        });
        // let scoreboard = Box::new(scoreboard);

        // pipeline_stages is the sum of normal pipeline stages
        // and specialized_unit stages * 2 (for ID and EX)
        // let total_pipeline_stages = PipelineStage::COUNT + config.num_specialized_unit.len() * 2;;
        // let pipeline_reg = (0..total_pipeline_stages)

        let pipeline_reg: Box<[_]> = PipelineStage::iter()
            .map(|stage| {
                let pipeline_width = config.pipeline_widths.get(&stage).copied().unwrap_or(0);
                register_set::RegisterSet::new(stage, pipeline_width, stage as usize)
            })
            .collect();

        if config.sub_core_model {
            // in subcore model, each scheduler should has its own
            // issue register, so ensure num scheduler = reg width
            let id_oc_sp_width = pipeline_reg[PipelineStage::ID_OC_SP as usize].size();
            assert_eq!(
                config.num_schedulers_per_core,
                id_oc_sp_width ,
                "number of schedulers ({}) and ID_OC_SP pipeline register width ({}) does not match", config.num_schedulers_per_core, id_oc_sp_width 
            );
            let id_oc_sfu_width = pipeline_reg[PipelineStage::ID_OC_SFU as usize].size();
            assert_eq!(
                config.num_schedulers_per_core,
                id_oc_sfu_width,
                "number of schedulers ({}) and ID_OC_SFU pipeline register width ({}) does not match", config.num_schedulers_per_core, id_oc_sfu_width 
            );
            let id_oc_mem_width = pipeline_reg[PipelineStage::ID_OC_MEM as usize].size();
            assert_eq!(
                config.num_schedulers_per_core,
                id_oc_mem_width,
                "number of schedulers ({}) and ID_OC_SFU pipeline register width ({}) does not match", config.num_schedulers_per_core, id_oc_mem_width 
            );
            // this is not sufficient
            assert!(
                config.num_sp_units % config.num_schedulers_per_core == 0,
                "number of SP execution units ({}) must be multiple of number of schedulers ({})",
                config.num_sp_units,
                config.num_schedulers_per_core,
            );
            assert!(
                config.num_dp_units % config.num_schedulers_per_core == 0,
                "number of DP execution units ({}) must be multiple of number of schedulers ({})",
                config.num_dp_units,
                config.num_schedulers_per_core,
            );
            assert!(
                config.num_sfu_units % config.num_schedulers_per_core == 0,
                "number of SFU execution units ({}) must be multiple of number of schedulers ({})",
                config.num_sfu_units,
                config.num_schedulers_per_core,
            );

            // this is also required
            if config.num_sp_units > 0 {
                assert_eq!(
                    config.num_sp_units, config.num_schedulers_per_core,
                    "number of SP execution units ({}) must be multiple of number of schedulers ({})",
                    config.num_sp_units, config.num_schedulers_per_core,
                );
            }
            if config.num_dp_units > 0 {
                assert_eq!(
                    config.num_dp_units, config.num_schedulers_per_core,
                    "number of DP execution units ({}) must be multiple of number of schedulers ({})",
                    config.num_dp_units,
                    config.num_schedulers_per_core,
                );
            }
            if config.num_sfu_units > 0 {
                assert_eq!(
                    config.num_sfu_units, config.num_schedulers_per_core,
                    "number of SFU execution units ({}) must be multiple of number of schedulers ({})",
                    config.num_sfu_units,
                    config.num_schedulers_per_core,
                );
            }

            // writeback stage for sp and sfu is shared
            let ex_wb_width = pipeline_reg[PipelineStage::EX_WB as usize].size();
            assert_eq!(
                config.num_sp_units + config.num_sfu_units,
                ex_wb_width,
                "sum of sp and sfu units ({} + {}) and EX_WB pipeline register width ({}) does not match", config.num_sp_units, config.num_sfu_units, id_oc_mem_width 
            );
        }

        // there are as many result buses as the width of the EX_WB stage
        let result_busses: Box<[_]> = (0..pipeline_reg[PipelineStage::EX_WB as usize].size())
            .map(|_| BitArray::ZERO)
            .collect();

        let mut register_file = RegisterFileUnit::new(config.clone());
        // configure generic collectors
        Self::init_operand_collector(&mut register_file, &config);

        let register_file = Box::new(register_file);

        let load_store_unit = fu::LoadStoreUnit::new(
            global_core_id, // is the core id for now
            global_core_id,
            cluster_id,
            load_store_response_queue,
            config.clone(),
            mem_controller.clone(),
        );

        // let scheduler_kind = config::SchedulerKind::GTO;
        // let schedulers: Box<[Box<dyn scheduler::Scheduler>]> = (0..config.num_schedulers_per_core)

        let schedulers: Box<[_]> = (0..config.num_schedulers_per_core)
            .map(|sched_id| {
                let scheduler = scheduler::gto::Scheduler::new(
                    sched_id,
                    global_core_id,
                    cluster_id,
                    config.clone(),
                );

                // let scheduler: Box<dyn scheduler::Scheduler> = match scheduler_kind {
                //     config::SchedulerKind::GTO => {
                //         let gto = scheduler::gto::Scheduler::new(
                //             sched_id,
                //             global_core_id,
                //             cluster_id,
                //             config.clone(),
                //         );
                //         Box::new(gto)
                //     }
                //     scheduler_kind => unimplemented!("scheduler: {:?}", &scheduler_kind),
                // };
                scheduler
            })
            .collect();

        let mut functional_units: Vec<Box<dyn fu::SimdFunctionUnit>> = Vec::new();

        // single precision units
        for issue_reg_id in 0..config.num_sp_units {
            functional_units.push(Box::new(fu::sp::SPUnit::new(
                issue_reg_id,
                Arc::clone(&config),
                issue_reg_id,
            )));
        }

        // double precision units
        for issue_reg_id in 0..config.num_dp_units {
            functional_units.push(Box::new(fu::DPUnit::new(
                issue_reg_id,
                Arc::clone(&config),
                issue_reg_id,
            )));
        }

        // integer units
        for issue_reg_id in 0..config.num_int_units {
            functional_units.push(Box::new(fu::IntUnit::new(
                issue_reg_id,
                Arc::clone(&config),
                issue_reg_id,
            )));
        }

        // special function units
        for issue_reg_id in 0..config.num_sfu_units {
            functional_units.push(Box::new(fu::SFU::new(
                issue_reg_id,
                Arc::clone(&config),
                issue_reg_id,
            )));
        }

        let barriers = barrier::Builder {
            // config.max_warps_per_core(),
            max_blocks_per_core: config.max_concurrent_blocks_per_core,
            max_barriers_per_block: config.max_barriers_per_block,
            warp_size: config.warp_size,
        }
        .build();

        // let instr_fetch_response_queue = Fifo::new(None);
        // let instr_fetch_response_queue = Arc::new(Mutex::new(instr_fetch_response_queue));

        // let load_store_response_queue = Fifo::new(None);
        // let load_store_response_queue = Arc::new(Mutex::new(load_store_response_queue));

        assert_eq!(config.max_concurrent_blocks_per_core, 32);
        let max_blocks_per_core = config.max_concurrent_blocks_per_core;

        let active_threads_per_hardware_block = utils::box_slice![0; max_blocks_per_core];
        let block_ids_per_hardware_block = utils::box_slice![None; max_blocks_per_core];

        Self {
            global_core_id,
            local_core_id,
            cluster_id,
            warp_instruction_unique_uid,
            stats,
            allocations,
            config,
            mem_controller,
            current_kernel: None,
            last_warp_fetched: 0,
            active_thread_mask: BitArray::ZERO,
            occupied_hw_thread_ids: BitArray::ZERO,
            dynamic_warp_id: 0,
            num_active_blocks: 0,
            num_active_warps: 0,
            num_active_threads: 0,
            num_occupied_threads: 0,
            active_threads_per_hardware_block,
            block_ids_per_hardware_block,
            instr_l1_cache,
            instr_fetch_response_queue,
            instr_fetch_buffer_state: InstructionFetchBufferState::Invalid,
            interconn,
            mem_port,
            load_store_unit,
            warps,
            pipeline_reg,
            result_busses,
            scoreboard,
            barriers,
            register_file,
            schedulers,
            scheduler_issue_priority: 0,
            functional_units,
            // fetch_return_callback: None,
        }
    }
}

impl<I, MC> Core<I, MC>
where
    MC: crate::mcu::MemoryController,
{
    fn fetch_instruction_from_instruction_cache<C>(
        instr: &WarpInstruction,
        inst_cache: &mut C,
        mem_controller: &MC,
        global_core_id: usize,
        cluster_id: usize,
        cycle: u64,
    ) -> cache::RequestStatus
    where
        C: cache::Cache,
    {
        let pc = instr.pc;
        let ppc = pc + crate::PROGRAM_MEM_START;

        let mut num_bytes = 16;
        let line_size = inst_cache.line_size();
        let offset_in_block = pc & (line_size - 1);
        if offset_in_block + num_bytes > line_size {
            num_bytes = line_size - offset_in_block;
        }
        let inst_alloc = &*crate::PROGRAM_MEM_ALLOC;
        let access = mem_fetch::access::Builder {
            kind: AccessKind::INST_ACC_R,
            addr: ppc as u64,
            kernel_launch_id: Some(instr.kernel_launch_id),
            allocation: Some(inst_alloc.clone()),
            req_size_bytes: num_bytes as u32,
            is_write: false,
            warp_active_mask: instr.active_mask,
            // warp_active_mask: warp::ActiveMask::all_ones(),
            // we dont need to set the sector and bit
            // masks, because they will be computed
            // when inserted into the memory sub partition
            byte_mask: !mem_fetch::ByteMask::ZERO,
            sector_mask: !mem_fetch::SectorMask::ZERO,
        }
        .build();

        let physical_addr = mem_controller.to_physical_address(access.addr);

        let fetch = mem_fetch::Builder {
            instr: None,
            access,
            warp_id: instr.warp_id,
            global_core_id: Some(global_core_id),
            cluster_id: Some(cluster_id),
            physical_addr,
        }
        .build();

        let mut events = Vec::new();
        inst_cache.access(ppc as address, fetch.clone(), &mut events, cycle)
    }

    fn find_active_warp_and_fetch_instructions(&mut self, cycle: u64) {
        let max_warps = self.max_warps_per_core() as usize;

        // if false {
        //     for warp_id in 0..max_warps {
        //         // let warp = self.warps[warp_id].try_borrow().unwrap();
        //         let warp = self.warps[warp_id].try_lock();
        //         if warp.instruction_count() == 0 {
        //             // consider empty
        //             continue;
        //         }
        //         debug_assert_eq!(warp.warp_id, warp_id);
        //
        //         let sb = self.scoreboard.try_read();
        //         let pending_writes = sb.pending_writes(warp_id);
        //
        //         // if warp.functional_done() && warp.hardware_done() && warp.done_exit() {
        //         //     continue;
        //         // }
        //         log::debug!(
        //         "checking warp_id = {} dyn warp id = {} (instruction count={}, trace pc={} hardware_done={}, functional_done={}, instr in pipe={}, stores={}, done_exit={}, pending writes={:?})",
        //         &warp_id,
        //         warp.dynamic_warp_id(),
        //         warp.instruction_count(),
        //         warp.trace_pc,
        //         warp.hardware_done(),
        //         warp.functional_done(),
        //         warp.num_instr_in_pipeline,
        //         warp.num_outstanding_stores,
        //         warp.done_exit(),
        //         pending_writes.iter().sorted().collect::<Vec<_>>()
        //     );
        //     }
        // }

        let mut num_checked = 0;

        // find an active warp with space in instruction buffer that is
        // not already waiting on a cache miss and get next 1-2 instructions
        // from instruction cache
        for i in 0..max_warps {
            num_checked += 1;
            let warp_id = (self.last_warp_fetched + 1 + i) % max_warps;
            // crate::timeit!(
            //     "core::fetch::check_warp_completed",
            //     self.check_for_completed_threads(warp_id)
            // );

            // {
            let warp = &self.warps[warp_id];

            #[cfg(debug_assertions)]
            if warp.warp_id != u32::MAX as usize {
                debug_assert_eq!(warp.warp_id, warp_id);
            }

            let kernel_id = warp.kernel_id;
            let block_hw_id = warp.block_id as usize;
            debug_assert!(block_hw_id < self.active_threads_per_hardware_block.len());

            // TODO: lets maybe move all these more expensive checks
            // into the issue stage?
            // using the scoreboard here just for that does not really make sense...
            let has_pending_writes = !self.scoreboard.pending_writes(warp_id).is_empty();

            let warp_completed = warp.hardware_done() && !has_pending_writes && !warp.done_exit();

            // check if this warp has finished executing and can be reclaimed.

            // let mut did_exit = false;
            if warp_completed {
                log::debug!("\tchecking if warp_id = {} did complete", warp_id);

                // let warp_thread_states = &self.thread_state
                //     [warp_id * self.config.warp_size..(warp_id + 1) * self.config.warp_size];

                debug_assert_eq!(self.config.warp_size, WARP_SIZE);
                let start_tid = warp_id * WARP_SIZE;
                let end_tid = start_tid + WARP_SIZE;

                let active_threads_in_warp =
                    self.active_thread_mask[start_tid..end_tid].count_ones();
                assert!(active_threads_in_warp <= WARP_SIZE);
                self.num_active_threads -= active_threads_in_warp;

                // is this fine? or only when there was at least one thread
                let warp = &mut self.warps[warp_id];
                // if active_threads_in_warp > 0 {
                warp.done_exit = true;
                // }

                self.active_thread_mask[start_tid..end_tid].fill(false);

                // for _ in 0..active_threads_in_warp {
                crate::timeit!(
                    "core::fetch::thread_exited",
                    self.register_threads_in_block_exited(
                        block_hw_id,
                        kernel_id,
                        active_threads_in_warp,
                    )
                );
                // }

                // for tid in start_tid..start_tid + self.config.warp_size {
                //     todo!();
                //     let Some(state) = &mut self.thread_state[tid] else {
                //         continue;
                //     };
                //     if state.active {
                //         state.active = false;
                //         //     log::debug!(
                //         //     "thread {} of hw block {} completed ({} active in core, {} active in block {})",
                //         //     tid,
                //         //     block_hw_id,
                //         //     self.active_thread_mask.count_ones() - 1,
                //         //     self.active_threads_per_hardware_block[block_hw_id],
                //         //     block_hw_id,
                //         // );
                //         self.num_active_threads -= 1;
                //         self.active_thread_mask.set(tid, false);
                //
                //         let warp = &mut self.warps[warp_id];
                //         warp.done_exit = true;
                //
                //         crate::timeit!(
                //             "core::fetch::thread_exited",
                //             self.register_thread_in_block_exited(block_hw_id, kernel_id)
                //         );
                //
                //         // did_exit = true;
                //     }
                // }

                // for t in 0..self.config.warp_size {
                //     let tid = warp_id * self.config.warp_size + t;
                //
                //     // todo: remove options from this
                //     let Some(ref mut state) = self.thread_state[tid] else {
                //         continue;
                //     };
                //     if !state.active {
                //         continue;
                //     }
                //     // if let Some(Some(state)) = self.thread_state.get_mut(tid) {
                //     // if state.active {
                //     state.active = false;
                //
                //     log::debug!(
                //         "thread {} of hw block {} completed ({} active in core, {} active in block {})",
                //         tid,
                //         block_hw_id,
                //         self.active_thread_mask.count_ones() - 1,
                //         self.active_threads_per_hardware_block[block_hw_id],
                //         block_hw_id,
                //     );
                //     crate::timeit!(
                //         "core::fetch::thread_exited",
                //         self.register_thread_in_block_exited(block_hw_id, kernel_id)
                //     );
                //
                //     self.num_active_threads -= 1;
                //     self.active_thread_mask.set(tid, false);
                //     did_exit = true;
                //     // }
                //     // }
                // }
                self.num_active_warps -= 1;
            }
            // }

            // drop(warp);
            let warp = &mut self.warps[warp_id];
            // if did_exit {
            //     warp.done_exit = true;
            // }

            // !warp.trace_instructions.is_empty() &&
            let should_fetch_instruction =
                !warp.functional_done() && !warp.has_imiss_pending && warp.instr_buffer.is_empty();

            // this code fetches instructions
            // from the i-cache or generates memory
            if should_fetch_instruction {
                debug_assert!(warp.current_instr().is_some());
                let Some(instr) = warp.current_instr() else {
                    // if warp.current_instr().is_none() {
                    // warp.hardware_done() && pending_writes.is_empty() && !warp.done_exit()
                    // dbg!(&warp);
                    // dbg!(&warp.active_mask.to_bit_string());
                    // dbg!(&warp.num_completed());
                    // panic!("?");
                    // skip and do nothing (can happen during nondeterministic parallel)
                    continue;
                };

                debug_assert_eq!(warp.pc(), Some(instr.pc));
                log::debug!(
                    "\t fetching instr {} for warp_id = {} (pc={})",
                    &instr,
                    warp.warp_id,
                    instr.pc,
                );

                // let icache_config = self
                //     .config
                //     .inst_cache_l1
                //     .as_ref()
                //     .expect("have instruction cache");
                // assert_eq!(
                //     self.instr_l1_cache.line_size(),
                //     icache_config.line_size as usize
                // );

                let status = if self.config.perfect_inst_const_cache {
                    cache::RequestStatus::HIT
                } else {
                    crate::timeit!(
                        "core::fetch::fetch_from_l1i",
                        Self::fetch_instruction_from_instruction_cache(
                            instr,
                            &mut self.instr_l1_cache,
                            &self.mem_controller,
                            self.global_core_id,
                            self.cluster_id,
                            cycle,
                        )
                    )
                };

                // log::warn!("L1I access({}) -> {:?}", fetch, status);
                // let warp = &mut self.warps[warp_id];
                self.last_warp_fetched = warp_id;

                if status == cache::RequestStatus::MISS {
                    warp.has_imiss_pending = true;
                    // warp.set_last_fetch(m_gpu->gpu_sim_cycle);
                } else if status == cache::RequestStatus::HIT {
                    self.instr_fetch_buffer_state.set_valid(warp_id);
                    // m_warp[warp_id]->set_last_fetch(m_gpu->gpu_sim_cycle);
                } else {
                    debug_assert_eq!(status, cache::RequestStatus::RESERVATION_FAIL);
                }
                break;
            }
        }
        log::trace!("fetch: num checked={}", num_checked);
        // dbg!(&num_checked);
    }

    #[tracing::instrument]
    fn fetch(&mut self, cycle: u64) {
        // if log::log_enabled!(log::Level::Debug) {
        //     log::debug!(
        //         "{}",
        //         style(format!(
        //             "cycle {:03} core {:?}: fetch (fetch buffer valid={}, l1i ready={:?})",
        //             cycle,
        //             self.id(),
        //             self.instr_fetch_buffer.valid,
        //             self.instr_l1_cache
        //                 .ready_accesses()
        //                 .cloned()
        //                 .unwrap_or_default()
        //                 .iter()
        //                 .map(std::string::ToString::to_string)
        //                 .collect::<Vec<_>>(),
        //         ))
        //         .green()
        //     );
        // }

        // if the instruction fetch buffer is valid, do not fetch
        // more instructions
        if !self.instr_fetch_buffer_state.is_valid() {
            // try to receive an instruction from the l1 inst cache
            if let Some(fetch) = self.instr_l1_cache.pop_next_ready_access() {
                let warp = &mut self.warps[fetch.warp_id];
                warp.has_imiss_pending = false;

                // mark the instruction fetch buffer for this warp
                // as valid, such that no more instructions are fetched.
                self.instr_fetch_buffer_state.set_valid(fetch.warp_id);

                // verify that we got the instruction we were expecting.
                debug_assert_eq!(
                    warp.pc(),
                    Some(fetch.addr() as usize - super::PROGRAM_MEM_START)
                );
            } else {
                // otherwise, find an active warp to fetch instructions for
                self.find_active_warp_and_fetch_instructions(cycle);
            }
        }
        if !self.config.perfect_inst_const_cache {
            // let mem_port = self.mem_port.try_lock();
            crate::timeit!(
                "core::fetch::l1i::cycle",
                self.instr_l1_cache.cycle(&mut self.mem_port, cycle)
            );
        }
    }

    #[tracing::instrument]
    fn execute(&mut self, cycle: u64) {
        let core_id = self.id();
        log::debug!(
            "{}",
            style(format!("cycle {cycle:03} core {core_id:?} execute: ")).red()
        );

        for (_, res_bus) in self.result_busses.iter_mut().enumerate() {
            // note: in rust, shift left is semantically equal to "towards the zero index"
            res_bus.shift_left(1);
            // log::debug!(
            //     "res bus {:03}[:128]: {}",
            //     i,
            //     &res_bus.to_bit_string()[0..128]
            // );
        }

        let functional_units_iter = self
            .functional_units
            .iter_mut()
            .map(|fu| fu.as_mut() as &mut dyn SimdFunctionUnit)
            .chain(std::iter::once(
                &mut self.load_store_unit as &mut dyn SimdFunctionUnit,
            ));

        for fu in functional_units_iter {
            let fu_id = fu.id().to_string();
            let issue_port = fu.issue_port();
            let result_port = fu.result_port();

            log::debug!(
                "fu[{:03}] {:<10} before \t{:?}={}",
                &fu_id,
                fu.to_string(),
                issue_port,
                &self.pipeline_reg[issue_port as usize]
            );

            // let todo = self.mem_port.try_lock();
            fu.cycle(
                &mut *self.register_file,
                &mut self.scoreboard,
                &mut self.warps,
                &mut self.stats,
                &mut self.mem_port,
                result_port.map(|port| &mut self.pipeline_reg[port as usize]),
                cycle,
            );
            fu.active_lanes_in_pipeline();

            log::debug!(
                "fu[{:03}] {:<10} after \t{:?}={}",
                &fu_id,
                fu.to_string(),
                issue_port,
                &self.pipeline_reg[issue_port as usize]
            );

            let partition_issue = self.config.sub_core_model && fu.is_issue_partitioned();

            let issue_inst = &mut self.pipeline_reg[issue_port as usize];
            let ready_reg: Option<&mut Option<WarpInstruction>> = if partition_issue {
                issue_inst
                    .get_ready_sub_core_mut(fu.issue_reg_id())
                    .map(|(_, r)| r)
            } else {
                issue_inst.get_ready_mut().map(|(_, r)| r)
            };

            let Some(ready_reg) = ready_reg else {
                continue;
            };

            log::trace!("occupied: {}", fu.occupied().to_bit_string());
            log::trace!(
                "{} checking {}: fu[{:03}] can issue={:?} latency={:?}",
                style(format!("cycle {cycle:03} core {core_id:?}: execute:",)).red(),
                crate::Optional(ready_reg.as_ref()),
                fu_id,
                ready_reg.as_ref().map(|instr| fu.can_issue(instr)),
                ready_reg.as_ref().map(|instr| instr.latency),
            );

            if let Some(ref instr) = ready_reg {
                if fu.can_issue(instr) {
                    let schedule_wb_now = !fu.stallable();
                    let result_bus = self
                        .result_busses
                        .iter_mut()
                        .find(|bus| !bus[instr.latency]);

                    log::debug!(
                        "{} {} (partition issue={}, schedule wb now={}, resbus={}, latency={:?}) ready for issue to fu[{:03}]={}",
                        style(format!(
                            "cycle {cycle:03} core {core_id:?}: execute:",
                        ))
                        .red(),
                        instr,
                        partition_issue,
                        schedule_wb_now,
                        result_bus.is_some(),
                        ready_reg.as_ref().map(|reg| reg.latency),
                        fu_id,
                        fu,
                    );

                    let mut issued = true;
                    match result_bus {
                        Some(result_bus) if schedule_wb_now => {
                            debug_assert!(instr.latency < fu::MAX_ALU_LATENCY);
                            result_bus.set(instr.latency, true);
                            // println!("execute {} [latency={}]", instr, instr.latency);
                            fu.issue(ready_reg.take().unwrap(), &mut self.stats);
                        }
                        _ if !schedule_wb_now => {
                            // println!("execute {} [latency={}]", instr, instr.latency);
                            fu.issue(ready_reg.take().unwrap(), &mut self.stats);
                        }
                        _ => {
                            // stall issue (cannot reserve result bus)
                            issued = false;
                        }
                    }
                    log::debug!("execute: issue={}", issued);
                }
            }
        }
    }
}

impl<I, MC> Core<I, MC> {
    pub fn max_warps_per_core(&self) -> usize {
        debug_assert_eq!(self.config.max_warps_per_core(), self.warps.len());
        self.warps.len()
    }

    pub fn stats(&self) -> stats::PerKernel {
        let mut stats = self.stats.clone();

        // add l1i stats
        let l1i = self.instr_l1_cache.per_kernel_stats().clone();
        for (kernel_launch_id, cache_stats) in l1i.into_iter().enumerate() {
            let kernel_stats = stats.get_mut(Some(kernel_launch_id));
            kernel_stats.l1i_stats[self.global_core_id] += cache_stats.clone();
        }

        // add l1d stats
        if let Some(ref l1d) = self.load_store_unit.data_l1 {
            let l1d = l1d.per_kernel_stats().clone();
            for (kernel_launch_id, cache_stats) in l1d.into_iter().enumerate() {
                let kernel_stats = stats.get_mut(Some(kernel_launch_id));
                kernel_stats.l1d_stats[self.global_core_id] += cache_stats.clone();
            }
        }

        // add scheduler stats
        for scheduler in self.schedulers.iter() {
            stats += scheduler.inner.stats.clone();
        }

        stats += self.mem_port.stats.clone();
        stats
    }

    // fn check_for_completed_threads(&mut self, warp_id: usize) {
    //     let warp = &self.warps[warp_id];
    //
    //     #[cfg(debug_assertions)]
    //     if warp.warp_id != u32::MAX as usize {
    //         debug_assert_eq!(warp.warp_id, warp_id);
    //     }
    //
    //     let kernel_id = warp.kernel_id;
    //     let block_hw_id = warp.block_id as usize;
    //     debug_assert!(block_hw_id < self.active_threads_per_hardware_block.len());
    //
    //     // todo: how expensive are arc clones?
    //     // let kernel = warp.kernel.as_ref().map(Arc::clone);
    //
    //     let has_pending_writes = !self.scoreboard.pending_writes(warp_id).is_empty();
    //
    //     let warp_completed = warp.hardware_done() && !has_pending_writes && !warp.done_exit();
    //
    //     // check if this warp has finished executing and can be reclaimed.
    //
    //     // let mut did_exit = false;
    //     if warp_completed {
    //         log::debug!("\tchecking if warp_id = {} did complete", warp_id);
    //
    //         // let warp_thread_states = &self.thread_state
    //         //     [warp_id * self.config.warp_size..(warp_id + 1) * self.config.warp_size];
    //
    //         let start_tid = warp_id * self.config.warp_size;
    //         // for thread_state in warp_thread_states.iter_mut() {
    //         for tid in start_tid..start_tid + self.config.warp_size {
    //             todo!();
    //
    //             let Some(state) = &mut self.thread_state[tid] else {
    //                     continue;
    //                 };
    //             if state.active {
    //                 state.active = false;
    //                 //     log::debug!(
    //                 //     "thread {} of hw block {} completed ({} active in core, {} active in block {})",
    //                 //     tid,
    //                 //     block_hw_id,
    //                 //     self.active_thread_mask.count_ones() - 1,
    //                 //     self.active_threads_per_hardware_block[block_hw_id],
    //                 //     block_hw_id,
    //                 // );
    //                 self.num_active_threads -= 1;
    //                 self.active_thread_mask.set(tid, false);
    //
    //                 let warp = &mut self.warps[warp_id];
    //                 warp.done_exit = true;
    //
    //                 crate::timeit!(
    //                     "core::fetch::thread_exited",
    //                     self.register_thread_in_block_exited(block_hw_id, kernel_id)
    //                 );
    //
    //                 // did_exit = true;
    //             }
    //         }
    //
    //         // for t in 0..self.config.warp_size {
    //         //     let tid = warp_id * self.config.warp_size + t;
    //         //
    //         //     // todo: remove options from this
    //         //     let Some(ref mut state) = self.thread_state[tid] else {
    //         //         continue;
    //         //     };
    //         //     if !state.active {
    //         //         continue;
    //         //     }
    //         //     // if let Some(Some(state)) = self.thread_state.get_mut(tid) {
    //         //     // if state.active {
    //         //     state.active = false;
    //         //
    //         //     log::debug!(
    //         //         "thread {} of hw block {} completed ({} active in core, {} active in block {})",
    //         //         tid,
    //         //         block_hw_id,
    //         //         self.active_thread_mask.count_ones() - 1,
    //         //         self.active_threads_per_hardware_block[block_hw_id],
    //         //         block_hw_id,
    //         //     );
    //         //     crate::timeit!(
    //         //         "core::fetch::thread_exited",
    //         //         self.register_thread_in_block_exited(block_hw_id, kernel_id)
    //         //     );
    //         //
    //         //     self.num_active_threads -= 1;
    //         //     self.active_thread_mask.set(tid, false);
    //         //     did_exit = true;
    //         //     // }
    //         //     // }
    //         // }
    //         self.num_active_warps -= 1;
    //     }
    // }
}

// Returns numbers of addresses in translated_addrs.
//
// Each addr points to a 4B (32-bit) word
#[must_use]
pub fn translate_local_memaddr(
    // &self,
    local_addr: address,
    global_core_id: usize,
    thread_id: usize,
    num_cores: usize,
    data_size: u32,
    thread_block_size: usize,
    max_blocks_per_core: usize,
    config: &config::GPU,
) -> Vec<address> {
    // During functional execution, each thread sees its own memory space for
    // local memory, but these need to be mapped to a shared address space for
    // timing simulation.  We do that mapping here.

    let (thread_base, max_concurrent_threads) = if config.local_mem_map {
        // Dnew = D*N + T%nTpC + nTpC*C
        // N = nTpC*nCpS*nS (max concurent threads)
        // C = nS*K + S (hw cta number per gpu)
        // K = T/nTpC   (hw cta number per core)
        // D = data index
        // T = thread
        // nTpC = number of threads per CTA
        // nCpS = number of CTA per shader
        //
        // for a given local memory address threads in a CTA map to
        // contiguous addresses, then distribute across memory space by CTAs
        // from successive shader cores first, then by successive CTA in same
        // shader core
        let kernel_padded_threads_per_cta = thread_block_size;
        // let kernel_max_cta_per_sm = self.block_status.len();

        let temp = global_core_id + num_cores * (thread_id / kernel_padded_threads_per_cta);
        let rest = thread_id % kernel_padded_threads_per_cta;
        let thread_base = 4 * (kernel_padded_threads_per_cta * temp + rest);
        let max_concurrent_threads =
            kernel_padded_threads_per_cta * max_blocks_per_core * num_cores;
        (thread_base, max_concurrent_threads)
    } else {
        // legacy mapping that maps the same address in the local memory
        // space of all threads to a single contiguous address region
        let thread_base = 4 * (config.max_threads_per_core * global_core_id + thread_id);
        let max_concurrent_threads = num_cores * config.max_threads_per_core;
        (thread_base, max_concurrent_threads)
    };
    debug_assert!(thread_base < 4 /*word size*/ * max_concurrent_threads);

    // If requested datasize > 4B, split into multiple 4B accesses
    // otherwise do one sub-4 byte memory access
    let mut translated_addresses = vec![];

    if data_size >= 4 {
        // >4B access, split into 4B chunks
        debug_assert_eq!(data_size % 4, 0); // Must be a multiple of 4B
        let num_accesses = data_size / 4;
        // max 32B
        debug_assert!(
            num_accesses <= super::instruction::MAX_ACCESSES_PER_THREAD_INSTRUCTION as u32
        );
        // Address must be 4B aligned - required if
        // accessing 4B per request, otherwise access
        // will overflow into next thread's space
        debug_assert_eq!(local_addr % 4, 0);
        for i in 0..num_accesses {
            let local_word = local_addr / 4 + u64::from(i);
            let linear_address: address = local_word * max_concurrent_threads as u64 * 4
                + thread_base as u64
                + crate::LOCAL_GENERIC_START;
            translated_addresses.push(linear_address);
        }
    } else {
        // Sub-4B access, do only one access
        debug_assert!(data_size > 0);
        let local_word = local_addr / 4;
        let local_word_offset = local_addr % 4;
        // Make sure access doesn't overflow into next 4B chunk
        dbg!(local_addr);
        dbg!(local_word);
        dbg!((local_addr + u64::from(data_size) - 1) / 4);
        debug_assert_eq!((local_addr + u64::from(data_size) - 1) / 4, local_word);
        let linear_address: address = local_word * max_concurrent_threads as u64 * 4
            + local_word_offset
            + thread_base as u64
            + crate::LOCAL_GENERIC_START;
        translated_addresses.push(linear_address);
    }
    translated_addresses
}

impl<I, MC> Core<I, MC>
where
    MC: crate::mcu::MemoryController,
{
    pub fn cache_flush(&mut self) {
        self.load_store_unit.flush();
    }

    // pub fn is_cache_flushed(&mut self) {
    //     self.load_store_unit.is_flushed();
    // }

    pub fn cache_invalidate(&mut self) {
        self.load_store_unit.invalidate();
    }
}

impl<I, MC> Core<I, MC> {
    #[must_use]
    pub fn num_active_threads(&self) -> usize {
        self.num_active_threads
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.num_active_blocks > 0
    }

    #[must_use]
    pub fn num_active_blocks(&self) -> usize {
        self.num_active_blocks
    }

    pub fn can_issue_block(&self, kernel: &dyn Kernel) -> bool {
        let max_blocks = kernel.max_blocks_per_core();
        if self.config.concurrent_kernel_sm {
            if max_blocks == 0 {
                return false;
            }
            unimplemented!("concurrent kernel sm model");
        } else {
            self.num_active_blocks < max_blocks
        }
    }

    #[must_use]
    pub fn id(&self) -> (usize, usize) {
        (self.cluster_id, self.global_core_id)
    }

    #[tracing::instrument(name = "core_reinit")]
    pub fn reinit(&mut self, start_thread: usize, end_thread: usize, reset_not_completed: bool) {
        if reset_not_completed {
            self.num_active_warps = 0;
            self.num_active_threads = 0;
            self.active_thread_mask.fill(false);
            // self.occupied_block_to_hw_thread_id.clear();
            self.occupied_hw_thread_ids.fill(false);
        }
        self.active_thread_mask[start_thread..end_thread].fill(false);
        // for t in start_thread..end_thread {
        //     self.thread_state[t] = None;
        // }
        let warp_size = self.config.warp_size;

        let start_warp = start_thread / warp_size;
        let end_warp = end_thread / warp_size;
        log::debug!(
            "reset warps {}..{} (threads {}..{})",
            start_warp,
            end_warp,
            start_thread,
            end_thread
        );

        for w in start_warp..end_warp {
            self.warps[w].reset();
        }
    }

    #[tracing::instrument(name = "core_issue_block")]
    // pub fn issue_block(&mut self, kernel: &dyn Kernel, cycle: u64) {
    // pub fn maybe_issue_block(
    pub fn maybe_issue_block<K>(
        &mut self,
        // kernel_manager: &dyn crate::kernel_manager::SelectKernel,
        kernel_manager: &K,
        cycle: u64,
    ) -> bool
    where
        K: crate::kernel_manager::SelectKernel,
    {
        if let Some(ref current_kernel) = self.current_kernel {
            // self.num_active_threads() == 0
            if !self.can_issue_block(&**current_kernel) {
                // if self.num_active_blocks >= current.max_blocks_per_core() {
                // fast path
                return false;
            }
        }

        log::debug!("core {:?}: issue block", self.id());

        // let max_blocks = self.config.max_blocks(kernel).unwrap();
        // if self.config.concurrent_kernel_sm {
        //     if max_blocks < 1 {
        //         return false;
        //     }
        //     // self.occupy_resource_for_block(kernel, false);
        //     unimplemented!("concurrent kernel sm model");
        // } else {
        //     self.num_active_blocks < max_blocks
        // }

        // select a kernel
        let should_select_new_kernel = if let Some(ref current) = self.current_kernel {
            // if no more blocks left, get new kernel once current block completes
            current.no_more_blocks_to_run() && self.num_active_threads() == 0
        } else {
            // core was not assigned a kernel yet
            true
        };

        if should_select_new_kernel {
            self.current_kernel = kernel_manager.select_kernel();
        }

        match self.current_kernel.as_ref() {
            None => {
                log::debug!("core {:?}: selected kernel NULL", self.id());
                return false;
            }
            Some(kernel) => {
                log::debug!(
                    "core {:?}: selected kernel {} more blocks={} can issue={}",
                    self.id(),
                    kernel,
                    !kernel.no_more_blocks_to_run(),
                    // self.can_issue_block(kernel.config()),
                    self.can_issue_block(&**kernel),
                );
                let can_issue = !kernel.no_more_blocks_to_run() && self.can_issue_block(&**kernel);
                if !can_issue {
                    return false;
                }
            }
        }
        crate::timeit!("issue_block::actually", self.issue_block_actually())
    }

    pub fn issue_block_actually(
        &mut self,
        // kernel_manager: &dyn crate::kernel_manager::SelectKernel,
        // cycle: u64,
    ) -> bool {
        // *crate::core::debug::NUM_ISSUE_BLOCK.lock() += 1;

        #[cfg(feature = "timings")]
        let start = std::time::Instant::now();

        // let Some(kernel) = self.current_kernel.as_ref() else {
        //     log::debug!(
        //         "core {:?}: selected kernel NULL",
        //         self.id(),
        //     );
        //     return false;
        // };
        //
        // log::debug!(
        //     "core {:?}: selected kernel {} more blocks={} can issue={}",
        //     self.id(),
        //     kernel,
        //     !kernel.no_more_blocks_to_run(),
        //     self.can_issue_block(&**kernel),
        // );
        //
        // let can_issue = !kernel.no_more_blocks_to_run() && self.can_issue_block(&**kernel);
        // // drop(core);
        // if !can_issue {
        //     // let mut core = self.cores[core_id].write();
        //     // let core = &mut self.cores[core_id];
        //     // self.issue_block(&*kernel, cycle);
        //     // num_blocks_issued += 1;
        //     // self.block_issue_next_core = core_id;
        //     // break;
        //     return false;
        // }

        let (free_block_hw_id, thread_block_size, padded_thread_block_size) = {
            let kernel = self.current_kernel.as_deref().unwrap();
            // let kernel_opt = &self.current_kernel;
            // let kernel = kernel_opt.as_deref().unwrap();

            if self.config.concurrent_kernel_sm {
                // let occupied = self.occupy_resource_for_block(&*kernel, true);
                // assert!(occupied);
                unimplemented!("concurrent kernel sm");
            } else {
                // calculate the max cta count and cta size for local memory address mapping
                // self.max_blocks_per_sm = self.config.max_blocks(kernel).unwrap();
                // self.current_kernel_max_blocks = self.config.max_blocks(kernel.config()).unwrap();
                // self.current_kernel_max_blocks = kernel.max_blocks_per_core();
                // self.thread_block_size = self.config.threads_per_block_padded(&*kernel);
            }

            // find a free block context
            let max_blocks_per_core = if self.config.concurrent_kernel_sm {
                unimplemented!("concurrent kernel sm");
                // self.config.max_concurrent_blocks_per_core
            } else {
                self.active_threads_per_hardware_block.len()
            };
            log::debug!(
                "core {:?}: free block number of threads: {:?}",
                self.id(),
                self.active_threads_per_hardware_block
            );

            debug_assert_eq!(
                self.num_active_blocks,
                self.active_threads_per_hardware_block
                    .iter()
                    .filter(|&num_threads_in_block| *num_threads_in_block > 0)
                    .count()
            );
            let Some(free_block_hw_id) = self.active_threads_per_hardware_block
                [0..max_blocks_per_core]
                .iter()
                .position(|num_threads| *num_threads == 0)
            else {
                // ROMAN: this should also be false? but accelsim compat...
                return true;
            };

            // determine hardware threads and warps that will be used for this block
            let thread_block_size = kernel.threads_per_block();
            let padded_thread_block_size = kernel.threads_per_block_padded();
            (
                free_block_hw_id,
                thread_block_size,
                padded_thread_block_size,
            )
        };

        // hw warp id = hw thread id mod warp size, so we need to find a range
        // of hardware thread ids corresponding to an integral number of hardware
        // thread ids
        let (start_thread, end_thread) = if self.config.concurrent_kernel_sm {
            let start_thread = self
                .find_available_hw_thread_id(padded_thread_block_size, true)
                .unwrap();
            let end_thread = start_thread + thread_block_size;

            // assert!(!self
            //     .occupied_block_to_hw_thread_id
            //     .contains_key(&free_block_hw_id));
            // self.occupied_block_to_hw_thread_id
            //     .insert(free_block_hw_id, start_thread);
            (start_thread, end_thread)
        } else {
            let start_thread = free_block_hw_id * padded_thread_block_size;
            let end_thread = start_thread + thread_block_size;
            (start_thread, end_thread)
        };

        // reset state of the selected hardware thread and warp contexts
        self.reinit(start_thread, end_thread, false);

        // initalize scalar threads and determine which hardware warps they are
        // allocated to bind functional simulation state of threads to hardware
        // resources (simulation)

        // dirty fix
        let kernel: Arc<_> = self.current_kernel.as_ref().unwrap().clone();
        // let no_more_blocks_to_run = kernel

        #[cfg(feature = "timings")]
        crate::TIMINGS
            .lock()
            .entry("issue_block::actually::pre_lock")
            .or_default()
            .add(start.elapsed());

        // let (block_id, mut block_reader) = {
        // why do threads spend literally 25s waiting for this lock?
        let mut kernel_reader = kernel.reader().lock();

        #[cfg(feature = "timings")]
        let start = std::time::Instant::now();

        // let block_reader_lock = block.reader_
        let Some(block) = kernel_reader.current_block() else {
            return false;
        };
        // .expect("kernel has current block");
        // let block = kernel.next_block().expect("kernel has current block");

        log::debug!(
            "core {:?}: issue block {} from kernel {}",
            self.id(),
            block,
            kernel,
        );
        let block_id = block.id();

        // self.active_thread_mask[start_thread..end_thread].fill(true);

        let start_warp = start_thread / WARP_SIZE;
        let end_warp = end_thread / WARP_SIZE;

        // let mut other_warps = warps.clone();
        let mut warps = WarpMask::ZERO;
        warps[start_warp..end_warp].fill(true);

        let num_threads_in_block = end_thread - start_thread;

        // let mut num_threads_in_block = 0;
        // for i in start_thread..end_thread {
        //     // self.active_thread_mask[start_thread..end_thread].fill(true);
        //     // self.thread_state[i] = Some(ThreadState {
        //     //     // block_id: free_block_hw_id,
        //     //     active: true,
        //     //     // pc: 0, // todo
        //     // });
        //     let warp_id = i / self.config.warp_size;
        //
        //     // ROMAN: removed this but is that fine?
        //     // if !kernel.no_more_blocks_to_run() {
        //     //     if !kernel.more_threads_in_block() {
        //     //         kernel.next_thread_iterlock().next();
        //     //     }
        //     //
        //     //     // we just incremented the thread id so this is not the same
        //     //     if !kernel.more_threads_in_block() {
        //     //         kernel.next_block_iterlock().next();
        //     //         *kernel.next_thread_iterlock() =
        //     //             kernel.config.block.into_iter().peekable();
        //     //     }
        //     num_threads_in_block += 1;
        //     // }
        //
        //     warps.set(warp_id, true);
        // }

        // assert_eq!(warps, other_warps);
        // assert_eq!(num_threads_in_block, end_thread - start_thread);

        kernel.increment_running_blocks();

        log::debug!(
            "num threads in block {}={} (hw {}) = {}",
            block,
            block_id,
            free_block_hw_id,
            num_threads_in_block
        );

        // TODO: factor all this into an execution state struct so we
        // do not lose count of where to update what and when..
        self.block_ids_per_hardware_block[free_block_hw_id] = Some(block);
        self.active_threads_per_hardware_block[free_block_hw_id] = num_threads_in_block;
        self.barriers.allocate(free_block_hw_id as u64, warps);
        self.init_warps(
            // &*self.current_kernel.unwrap().as_ref(),
            &mut *kernel_reader,
            free_block_hw_id,
            block_id,
            start_thread,
            end_thread,
        );
        self.num_active_blocks += 1;

        #[cfg(feature = "timings")]
        crate::TIMINGS
            .lock()
            .entry("issue_block::actually::post_lock")
            .or_default()
            .add(start.elapsed());

        true
    }
}

impl<I, MC> Core<I, MC> {
    fn init_operand_collector(register_file: &mut RegisterFileUnit, config: &config::GPU) {
        register_file.add_cu_set(
            operand_collector::Kind::GEN_CUS,
            config.operand_collector_num_units_gen,
            config.operand_collector_num_out_ports_gen,
        );

        for _i in 0..config.operand_collector_num_in_ports_gen {
            let mut in_ports = operand_collector::PortVec::new();
            let mut out_ports = operand_collector::PortVec::new();
            let mut cu_sets: Vec<operand_collector::Kind> = Vec::new();

            in_ports.push(PipelineStage::ID_OC_SP);
            in_ports.push(PipelineStage::ID_OC_SFU);
            in_ports.push(PipelineStage::ID_OC_MEM);
            out_ports.push(PipelineStage::OC_EX_SP);
            out_ports.push(PipelineStage::OC_EX_SFU);
            out_ports.push(PipelineStage::OC_EX_MEM);

            if config.num_dp_units > 0 {
                in_ports.push(PipelineStage::ID_OC_DP);
                out_ports.push(PipelineStage::OC_EX_DP);
            }
            if config.num_int_units > 0 {
                in_ports.push(PipelineStage::ID_OC_INT);
                out_ports.push(PipelineStage::OC_EX_INT);
            }

            cu_sets.push(operand_collector::Kind::GEN_CUS);
            register_file.add_port(in_ports, out_ports, cu_sets);
        }

        if config.enable_specialized_operand_collector {
            unimplemented!();
            // for (kind, num_collector_units, num_dispatch_units) in [
            //     (
            //         operand_collector::Kind::SP_CUS,
            //         config.operand_collector_num_units_sp,
            //         config.operand_collector_num_out_ports_sp,
            //     ),
            //     (
            //         operand_collector::Kind::DP_CUS,
            //         config.operand_collector_num_units_dp,
            //         config.operand_collector_num_out_ports_dp,
            //     ),
            //     (
            //         operand_collector::Kind::SFU_CUS,
            //         config.operand_collector_num_units_sfu,
            //         config.operand_collector_num_out_ports_sfu,
            //     ),
            //     (
            //         operand_collector::Kind::MEM_CUS,
            //         config.operand_collector_num_units_mem,
            //         config.operand_collector_num_out_ports_mem,
            //     ),
            //     (
            //         operand_collector::Kind::INT_CUS,
            //         config.operand_collector_num_units_int,
            //         config.operand_collector_num_out_ports_int,
            //     ),
            // ] {
            //     register_file.add_cu_set(kind, num_collector_units, num_dispatch_units);
            // }
            //
            // for _ in 0..config.operand_collector_num_in_ports_sp {
            //     let in_ports = vec![PipelineStage::ID_OC_SP];
            //     let out_ports = vec![PipelineStage::OC_EX_SP];
            //
            //     let cu_sets = vec![
            //         operand_collector::Kind::SP_CUS,
            //         operand_collector::Kind::GEN_CUS,
            //     ];
            //
            //     register_file.add_port(in_ports, out_ports, cu_sets);
            // }
            //
            // for _ in 0..config.operand_collector_num_in_ports_dp {
            //     let in_ports = vec![PipelineStage::ID_OC_DP];
            //     let out_ports = vec![PipelineStage::OC_EX_DP];
            //
            //     let cu_sets = vec![
            //         operand_collector::Kind::DP_CUS,
            //         operand_collector::Kind::GEN_CUS,
            //     ];
            //
            //     register_file.add_port(in_ports, out_ports, cu_sets);
            // }
            //
            // for _ in 0..config.operand_collector_num_in_ports_sfu {
            //     let in_ports = vec![PipelineStage::ID_OC_SFU];
            //     let out_ports = vec![PipelineStage::OC_EX_SFU];
            //
            //     let cu_sets = vec![
            //         operand_collector::Kind::SFU_CUS,
            //         operand_collector::Kind::GEN_CUS,
            //     ];
            //
            //     register_file.add_port(in_ports, out_ports, cu_sets);
            // }
            //
            // for _ in 0..config.operand_collector_num_in_ports_mem {
            //     let in_ports = vec![PipelineStage::ID_OC_MEM];
            //     let out_ports = vec![PipelineStage::OC_EX_MEM];
            //
            //     let cu_sets = vec![
            //         operand_collector::Kind::MEM_CUS,
            //         operand_collector::Kind::GEN_CUS,
            //     ];
            //
            //     register_file.add_port(in_ports, out_ports, cu_sets);
            // }
            //
            // for _ in 0..config.operand_collector_num_in_ports_int {
            //     let in_ports = vec![PipelineStage::ID_OC_INT];
            //     let out_ports = vec![PipelineStage::OC_EX_INT];
            //
            //     let cu_sets = vec![
            //         operand_collector::Kind::INT_CUS,
            //         operand_collector::Kind::GEN_CUS,
            //     ];
            //
            //     register_file.add_port(in_ports, out_ports, cu_sets);
            // }
        }

        // this must be called after we add the collector unit sets!
        register_file.init(config.num_reg_banks);
    }

    fn register_threads_in_block_exited(
        &mut self,
        block_hw_id: usize,
        kernel_id: Option<u64>,
        num_threads_exited: usize,
    ) {
        let global_core_id = self.global_core_id;
        let current_kernel: &mut Option<_> = &mut self.current_kernel;
        let current_kernel_id = current_kernel.as_ref().map(|k| k.id());

        debug_assert!(block_hw_id < self.active_threads_per_hardware_block.len());
        debug_assert!(self.active_threads_per_hardware_block[block_hw_id] > 0);

        self.active_threads_per_hardware_block[block_hw_id] -= num_threads_exited;

        // this is the last thread that exited
        if self.active_threads_per_hardware_block[block_hw_id] == 0 {
            let block = &self.block_ids_per_hardware_block[block_hw_id];

            if let Some(block) = block {
                let block_id = block.id();
                let block_size = block.size();
                let active_threads = self.active_threads_per_hardware_block[block_hw_id];
                let total_threads = current_kernel
                    .as_ref()
                    .map(|kernel| kernel.threads_per_block())
                    .unwrap_or(0);

                // let running_blocks = current_kernel
                //     .as_ref()
                //     .map(|kernel| kernel.num_running_blocks())
                //     .unwrap_or(0);

                // let kernel_block_size = current_kernel
                //     .as_ref()
                //     .map(|kernel| kernel.num_blocks())
                //     .unwrap_or(0);
                //
                // assert_eq!(kernel_block_size as u64, block_size);

                eprintln!(
                    "  => core {:>4} \tblock {:>4} {:<22} ({:>4}/{:<4} hw {:<2}) finished thread\t({:>3}/{:<3} threads remaining)",
                    global_core_id,
                    block_id,
                    block.to_string(),
                    // block.map(ToString::to_string).as_deref().unwrap_or("?"),
                    self.num_active_blocks,
                    // running_blocks,
                    block_size,
                    block_hw_id,
                    active_threads,
                    total_threads,
                );
            }
            // }
            //
            // if self.active_threads_per_hardware_block[block_hw_id] == 0 {
            assert_eq!(current_kernel_id, kernel_id);

            assert!(current_kernel.is_some());

            // debug::COMPLETED_BLOCKS.lock().push(debug::CompletedBlock {
            //     global_core_id,
            //     block: self.block_ids_per_hardware_block[block_hw_id]
            //         .as_ref()
            //         .unwrap()
            //         .clone(),
            //     kernel_id: current_kernel_id.unwrap(),
            // });

            // deallocate barriers for this block
            self.barriers.deallocate(block_hw_id as u64);

            // remove the block id
            self.block_ids_per_hardware_block[block_hw_id] = None;

            // decrement running blocks for the current kernel
            // TODO: if we want to support multiple, we would get the kernel by id

            // self.release_shader_resource_1block(cta_num, kernel);

            if let Some(kernel) = current_kernel {
                kernel.decrement_running_blocks();
                if kernel.no_more_blocks_to_run() && !kernel.running() {
                    log::info!("kernel {} ({}) completed", kernel.name(), kernel.id());
                    eprintln!("kernel {} ({}) completed", kernel.name(), kernel.id());
                    *current_kernel = None;
                    // set done here
                }
            }

            // if let Some(kernel) = kernel {
            //     kernel.decrement_running_blocks();
            //
            //     if kernel.no_more_blocks_to_run()
            //         && !kernel.running()
            //         && current_kernel.as_ref().map(|k| k.id()) == Some(kernel.id())
            //     {
            //         log::info!("kernel {} ({}) completed", kernel.name(), kernel.id());
            //         *current_kernel = None;
            //         // set done here
            //     }
            // }

            // increment the number of completed blocks
            self.num_active_blocks -= 1;

            if self.num_active_blocks == 0 {
                // Shader can only be empty when no more cta are dispatched
                // if kernel.as_ref().map(|k| k.id()) != current_kernel.as_ref().map(|k| k.id()) {
                // debug_assert!(current_kernel.is_none() || kernel.no_more_blocks_to_run());
                // }
                *current_kernel = None;
            }
        }
    }

    /// Shader core decode
    #[tracing::instrument]
    fn decode(&mut self, cycle: u64) {
        log::debug!(
            "{}",
            style(format!(
                "cycle {:03} core {:?}: decode (fetch buffer valid={})",
                cycle,
                self.id(),
                self.instr_fetch_buffer_state.is_valid(),
            ))
            .blue()
        );

        // instruction fetch buffer state is basically a pipeline register
        // interpret this as if we have fetched up to two instructions and
        // now we are encoding them and placing them into the instruction
        // buffer.

        // if the instruction fetch buffer for this warp is valid,
        // there are still valid, decoded instructions in the buffer
        // waiting to be issued, so we do not continue decoding
        let InstructionFetchBufferState::Valid { warp_id } = self.instr_fetch_buffer_state else {
            return;
        };

        // decode 1 or 2 instructions and buffer them
        let warp = self.warps.get_mut(warp_id).unwrap();
        debug_assert_eq!(warp.warp_id, warp_id);

        // TODO: maybe reduce memory by actually taking the instructions
        // from the warp trace
        let instr1 = warp.next_trace_inst().cloned();
        let instr2 = if instr1.is_some() {
            warp.next_trace_inst().cloned()
        } else {
            None
        };

        // debug: print all instructions in this warp
        assert_eq!(warp.warp_id, warp_id);
        if false {
            let already_issued_trace_pc = warp.trace_pc;
            print_trace_instructions(&warp, already_issued_trace_pc);
        }

        if let Some(instr1) = instr1 {
            self.decode_instruction(warp_id, instr1, 0);
        }

        if let Some(instr2) = instr2 {
            self.decode_instruction(warp_id, instr2, 1);
        }

        // this should not be necessary?
        self.instr_fetch_buffer_state.set_invalid();
    }

    fn decode_instruction(&mut self, warp_id: usize, instr: WarpInstruction, slot: usize) {
        let warp = self.warps.get_mut(warp_id).unwrap();

        log::debug!(
            "====> warp[warp_id={:03}] ibuffer fill at slot {:01} with instruction {}",
            warp.warp_id,
            slot,
            instr,
        );

        warp.instr_buffer.fill(slot, instr);
        warp.num_instr_in_pipeline += 1;
    }

    #[tracing::instrument]
    fn issue(&mut self, cycle: u64) {
        // fair round robin issue between schedulers
        let num_schedulers = self.schedulers.len();
        assert_eq!(num_schedulers, self.config.num_schedulers_per_core);

        for scheduler_idx in 0..num_schedulers {
            let scheduler_idx = (self.scheduler_issue_priority + scheduler_idx) % num_schedulers;

            // compute subset of warps supervised by this scheduler
            let mut scheduler_supervised_warps = self
                .warps
                .iter_mut()
                .enumerate()
                .filter(|(i, _)| scheduler_idx == i % num_schedulers)
                .map(|(_, warp)| warp)
                .enumerate()
                .collect::<SmallVec<[(usize, &mut warp::Warp); 64]>>();

            assert!(!scheduler_supervised_warps.spilled());

            // dbg!(
            //     &scheduler_idx,
            //     &scheduler_supervised_warps
            //         .iter()
            //         .map(|(id, warp)| (id, warp.warp_id, warp.dynamic_warp_id))
            //         .collect::<Vec<_>>()
            // );

            let max_blocks_per_core = self
                .current_kernel
                .as_ref()
                .map(|kernel| kernel.max_blocks_per_core())
                .unwrap_or(0);
            let thread_block_size = self
                .current_kernel
                .as_ref()
                .map(|kernel| kernel.threads_per_block_padded())
                .unwrap_or(0);

            let mut issuer = CoreIssuer {
                config: &self.config,
                pipeline_reg: &mut self.pipeline_reg,
                warp_instruction_unique_uid: &self.warp_instruction_unique_uid,
                allocations: &self.allocations,
                stats: &mut self.stats,
                global_core_id: self.global_core_id,
                // thread_block_size: self.thread_block_size,
                // max_blocks_per_core: self.current_kernel_max_blocks,
                thread_block_size,
                max_blocks_per_core,
                scoreboard: &mut self.scoreboard,
                barriers: &mut self.barriers,
            };

            self.schedulers[scheduler_idx].issue_to(
                &mut issuer,
                &mut scheduler_supervised_warps,
                cycle,
            );
        }
        self.scheduler_issue_priority = (self.scheduler_issue_priority + 1) % num_schedulers;
    }

    #[tracing::instrument]
    fn writeback(&mut self, cycle: u64) {
        // from the functional units
        let id = self.id();
        let exec_writeback_pipeline = &mut self.pipeline_reg[PipelineStage::EX_WB as usize];

        log::debug!(
            "{}",
            style(format!(
                "cycle {:03} core {:?}: writeback: ex wb pipeline={}",
                cycle, id, exec_writeback_pipeline
            ))
            .cyan()
        );
        let _max_committed_thread_instructions =
            self.config.warp_size * exec_writeback_pipeline.size();

        // m_stats->m_pipeline_duty_cycle[m_sid] =
        //     ((float)(m_stats->m_num_sim_insn[m_sid] -
        //              m_stats->m_last_num_sim_insn[m_sid])) /
        //     max_committed_thread_instructions;
        //
        // m_stats->m_last_num_sim_insn[m_sid] = m_stats->m_num_sim_insn[m_sid];
        // m_stats->m_last_num_sim_winsn[m_sid] = m_stats->m_num_sim_winsn[m_sid];
        while let Some(mut ready) = exec_writeback_pipeline
            .get_ready_mut()
            .and_then(|(_, r)| r.take())
        {
            log::debug!("ready for writeback: {}", ready);

            // Right now, the writeback stage drains all waiting instructions
            // assuming there are enough ports in the register file or the
            // conflicts are resolved at issue.
            //
            // The operand collector writeback can generally generate a stall
            // However, here, the pipelines should be un-stallable. This is
            // guaranteed because this is the first time the writeback function
            // is called after the operand collector's step function, which
            // resets the allocations. There is one case which could result in
            // the writeback function returning false (stall), which is when
            // an instruction tries to modify two registers (GPR and predicate)
            // To handle this case, we ignore the return value (thus allowing
            // no stalling).
            //
            self.register_file.writeback(&mut ready);
            self.scoreboard.release_all(&ready);
            let warp = self.warps.get_mut(ready.warp_id).unwrap();
            warp.num_instr_in_pipeline -= 1;
            warp_inst_complete(&mut ready, &mut self.stats);

            //   m_gpu->gpu_sim_insn_last_update_sid = m_sid;
            //   m_gpu->gpu_sim_insn_last_update = m_gpu->gpu_sim_cycle;
            //   m_last_inst_gpu_sim_cycle = m_gpu->gpu_sim_cycle;
            //   m_last_inst_gpu_tot_sim_cycle = m_gpu->gpu_tot_sim_cycle;
            // preg = m_pipeline_reg[EX_WB].get_ready();
            //   pipe_reg = (preg == NULL) ? NULL : *preg;
        }
    }

    #[must_use]
    fn find_available_hw_thread_id(
        &mut self,
        thread_block_size: usize,
        occupy: bool,
    ) -> Option<usize> {
        let mut step = 0;
        while step < self.config.max_threads_per_core {
            if self.occupied_hw_thread_ids[step..(step + thread_block_size)].not_any() {
                // found consecutive non-active
                break;
            }
            // for hw_thread_id in step..(step + thread_block_size) {
            //     if self.occupied_hw_thread_ids[hw_thread_id] {
            //         break;
            //     }
            // }
            // consecutive non-active
            // if hw_thread_id == step + thread_block_size {
            //     break;
            // }
            step += thread_block_size;
        }
        if step >= self.config.max_threads_per_core {
            // didn't find
            None
        } else {
            if occupy {
                self.occupied_hw_thread_ids[step..(step + thread_block_size)].fill(true);
                // for hw_thread_id in step..(step + thread_block_size) {
                //     self.occupied_hw_thread_ids.set(hw_thread_id, true);
                // }
            }
            Some(step)
        }
    }

    #[tracing::instrument(name = "core_init_warps_from_traces")]
    fn init_warps_from_traces(
        &mut self,
        reader: &mut dyn crate::trace::ReadWarpsForBlock,
        start_warp: usize,
        end_warp: usize,
    ) {
        let kernel = self.current_kernel.as_deref().unwrap();

        debug_assert!(!self.warps.is_empty());
        let selected_warps = &mut self.warps[start_warp..end_warp];
        for warp in &mut *selected_warps {
            warp.trace_instructions.clear();
            warp.kernel_id = Some(kernel.id());
            warp.trace_pc = 0;
        }

        let (block, _) = crate::timeit!(
            "core::read_trace",
            reader.read_warps_for_block(selected_warps, &*kernel, &self.config)
        );
        if block.is_some() {
            let kernel_stats = self.stats.get_mut(Some(kernel.id() as usize));
            kernel_stats.sim.num_blocks += 1;
        }
        log::debug!(
            "initialized traces {}..{} of {} warps",
            start_warp,
            end_warp,
            &self.warps.len()
        );
    }

    #[tracing::instrument(name = "core_init_warps")]
    fn init_warps(
        &mut self,
        reader: &mut dyn crate::trace::ReadWarpsForBlock,
        block_hw_id: usize,
        block_id: u64,
        start_thread: usize,
        end_thread: usize,
    ) {
        let kernel = self.current_kernel.as_deref().unwrap();

        let start_warp = start_thread / self.config.warp_size;
        let end_warp = end_thread / self.config.warp_size
            + usize::from(end_thread % self.config.warp_size != 0);

        self.active_thread_mask[start_thread..end_thread].fill(true);

        // TODO: all this can we written more concisely, but not performance
        // critical so left as is
        for warp_id in start_warp..end_warp {
            let mut num_active_threads_in_warp = 0;

            let mut local_active_thread_mask = warp::ActiveMask::ZERO;
            for warp_thread_id in 0..self.config.warp_size {
                let hwtid = warp_id * self.config.warp_size + warp_thread_id;
                if hwtid < end_thread {
                    num_active_threads_in_warp += 1;
                    local_active_thread_mask.set(warp_thread_id, true);
                }
            }

            debug_assert_eq!(
                num_active_threads_in_warp,
                local_active_thread_mask.count_ones()
            );

            let warp = self.warps.get_mut(warp_id).unwrap();
            warp.init(
                block_hw_id as u64,
                warp_id,
                self.dynamic_warp_id,
                local_active_thread_mask,
                kernel.id(),
            );

            self.dynamic_warp_id += 1;
            self.num_active_warps += 1;
            self.num_active_threads += num_active_threads_in_warp;
        }

        log::debug!(
            "initialized warps {}..{} (threads {}..{}) for block {} (hw {})",
            start_warp,
            end_warp,
            start_thread,
            end_thread,
            block_id,
            block_hw_id,
        );
        self.init_warps_from_traces(reader, start_warp, end_warp);
    }
}

impl<I, MC> Core<I, MC>
where
    I: ic::Interconnect<ic::Packet<mem_fetch::MemFetch>>,
    MC: crate::mcu::MemoryController,
{
    #[tracing::instrument(name = "core_cycle")]
    pub fn cycle(&mut self, cycle: u64) {
        if !self.config.accelsim_compat && self.current_kernel.is_none() {
            // fast path
            return;
        }

        if log::log_enabled!(log::Level::Debug) {
            log::debug!(
                "{} \tactive={}, not completed={} ldst unit response buffer={}",
                style(format!(
                    "cycle {:03} core {:?}: core cycle",
                    cycle,
                    self.id()
                ))
                .blue(),
                self.is_active(),
                self.num_active_threads(),
                -1,
            );
        }

        {
            while let Some(ic::Packet { fetch, time }) = self.instr_fetch_response_queue.receive() {
                // if let Some(fetch_return_cb) = &self.fetch_return_callback {
                //     fetch_return_cb(time, &fetch);
                // }
                self.instr_l1_cache.fill(fetch, time);
            }
        }

        crate::timeit!("core::writeback", self.writeback(cycle));
        crate::timeit!("core::execute", self.execute(cycle));
        for _ in 0..self.config.reg_file_port_throughput {
            crate::timeit!(
                "core::operand collector",
                self.register_file.cycle(&mut self.pipeline_reg)
            );
        }

        crate::timeit!("core::issue", self.issue(cycle));
        for _i in 0..self.config.inst_fetch_throughput {
            crate::timeit!("core::decode", self.decode(cycle));
            crate::timeit!("core::fetch", self.fetch(cycle));
        }
    }
}

pub fn warp_inst_complete(instr: &mut WarpInstruction, stats: &mut stats::PerKernel) {
    let kernel_stats = stats.get_mut(Some(instr.kernel_launch_id as usize));
    kernel_stats.sim.instructions += instr.active_thread_count() as u64;
    // log::error!(
    //     "kernel {}: warp inst {} completed. instructions={}",
    //     instr.kernel_launch_id,
    //     &instr,
    //     kernel_stats.sim.instructions
    // );
    // crate::WIP_STATS.lock().warp_instructions += 1;
}

#[allow(dead_code)]
fn print_trace_instructions(warp: &warp::Warp, already_issued_trace_pc: usize) {
    log::debug!(
        "====> instruction at trace pc < {:<4} already issued ...",
        already_issued_trace_pc
    );

    for (trace_pc, trace_instr) in warp
        .trace_instructions
        .iter()
        .enumerate()
        .skip(already_issued_trace_pc)
    {
        log::debug!(
            "====> warp[{:03}][trace_pc={:03}]:\t {}\t\t active={} \tpc={} idx={}",
            warp.warp_id,
            trace_pc,
            trace_instr,
            trace_instr.active_mask.to_bit_string(),
            trace_instr.pc,
            trace_instr.trace_idx
        );
    }
}
