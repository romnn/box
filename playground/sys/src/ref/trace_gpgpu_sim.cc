#include "trace_gpgpu_sim.hpp"

#include <signal.h>
#include <iomanip>

#include "bridge/accelsim_config.hpp"
#include "fmt/format.h"
#include "hal.hpp"
#include "io.hpp"
#include "timeit.hpp"
#include "cache_sub_stats.hpp"
#include "icnt_wrapper.hpp"
#include "memory_partition_unit.hpp"
#include "memory_stats.hpp"
#include "memory_sub_partition.hpp"
#include "shader_core_stats.hpp"
#include "stats/tool.hpp"
#include "stats_wrapper.hpp"
#include "trace_simt_core_cluster.hpp"

#define MAX(a, b) (((a) > (b)) ? (a) : (b))

bool Allocation::contains(new_addr_type addr) const {
  return start_addr <= addr && addr < end_addr;
}

void trace_gpgpu_sim::createSIMTCluster() {
  m_cluster = new trace_simt_core_cluster *[m_shader_config->n_simt_clusters];
  for (unsigned i = 0; i < m_shader_config->n_simt_clusters; i++)
    m_cluster[i] =
        new trace_simt_core_cluster(this, i, m_shader_config, m_memory_config,
                                    m_shader_stats, m_memory_stats, stats_out);
}

void trace_gpgpu_sim::reinit_clock_domains(void) {
  core_time = 0;
  dram_time = 0;
  icnt_time = 0;
  l2_time = 0;
}

void trace_gpgpu_sim::init() {
  // run a CUDA grid on the GPU microarchitecture simulator
  gpu_sim_cycle = 0;
  gpu_sim_insn = 0;
  last_gpu_sim_insn = 0;
  m_total_cta_launched = 0;
  gpu_completed_cta = 0;
  partiton_reqs_in_parallel = 0;
  partiton_replys_in_parallel = 0;
  partiton_reqs_in_parallel_util = 0;
  gpu_sim_cycle_parition_util = 0;

  // REMOVE: power
  // McPAT initialization function. Called on first launch of GPU
  // #ifdef GPGPUSIM_POWER_MODEL
  //   if (m_config.g_power_simulation_enabled) {
  //     init_mcpat(m_config, m_gpgpusim_wrapper, m_config.gpu_stat_sample_freq,
  //                gpu_tot_sim_insn, gpu_sim_insn);
  //   }
  // #endif

  reinit_clock_domains();
  gpgpu_ctx->func_sim->set_param_gpgpu_num_shaders(m_config.num_shader());
  for (unsigned i = 0; i < m_shader_config->n_simt_clusters; i++)
    m_cluster[i]->reinit();
  m_shader_stats->new_grid();
  // initialize the control-flow, memory access, memory latency logger
  if (m_config.g_visualizer_enabled) {
    create_thread_CFlogger(gpgpu_ctx, m_config.num_shader(),
                           m_shader_config->n_thread_per_shader, 0,
                           m_config.gpgpu_cflog_interval);
  }
  shader_CTA_count_create(m_config.num_shader(), m_config.gpgpu_cflog_interval);
  if (m_config.gpgpu_cflog_interval != 0) {
    insn_warp_occ_create(m_config.num_shader(), m_shader_config->warp_size);
    shader_warp_occ_create(m_config.num_shader(), m_shader_config->warp_size,
                           m_config.gpgpu_cflog_interval);
    shader_mem_acc_create(m_config.num_shader(), m_memory_config->m_n_mem, 4,
                          m_config.gpgpu_cflog_interval);
    shader_mem_lat_create(m_config.num_shader(), m_config.gpgpu_cflog_interval);
    shader_cache_access_create(m_config.num_shader(), 3,
                               m_config.gpgpu_cflog_interval);
    set_spill_interval(m_config.gpgpu_cflog_interval * 40);
  }

  if (g_network_mode) icnt_init();
}

void trace_gpgpu_sim::perf_memcpy_to_gpu(size_t dst_start_addr, size_t count) {
  logger->info("memcopy: <unnamed> {:>15} ({:>5} f32) to address {:>20}", count,
               count / 4, dst_start_addr);

  unsigned id = m_allocations.size() + 1;  // zero is reserved for instructions
  m_allocations.insert(Allocation(id, dst_start_addr, dst_start_addr + count));

  if (m_memory_config->m_perf_sim_memcpy) {
    // if(!m_config.trace_driven_mode)    //in trace-driven mode, CUDA runtime
    // can start nre data structure at any position 	assert (dst_start_addr %
    // 32
    //== 0);

    for (size_t counter = 0; counter < count; counter += 32) {
      // FIX: ROMAN write address must be size_t as well, otherwise we overflow
      // the address
      const size_t wr_addr = dst_start_addr + counter;
      addrdec_t raw_addr;

      m_memory_config->m_address_mapping.addrdec_tlx(wr_addr, &raw_addr);
      const unsigned partition_id =
          raw_addr.sub_partition /
          m_memory_config->m_n_sub_partition_per_memory_channel;
      const unsigned sub_partition_id =
          raw_addr.sub_partition %
          m_memory_config->m_n_sub_partition_per_memory_channel;

      mem_access_sector_mask_t mask;
      mask.set(wr_addr % 128 / 32);

      logger->trace(
          "memcopy to gpu: copy 32 byte chunk starting at {} to sub partition "
          "unit {} of partition unit {} ({} )(mask {})",
          wr_addr, sub_partition_id, partition_id, raw_addr.sub_partition,
          mask_to_string(mask));

      m_memory_partition_unit[partition_id]->handle_memcpy_to_gpu(
          wr_addr, raw_addr.sub_partition, mask);
    }
  }
}

bool trace_gpgpu_sim::can_start_kernel() {
  for (unsigned n = 0; n < m_running_kernels.size(); n++) {
    if ((NULL == m_running_kernels[n]) || m_running_kernels[n]->done())
      return true;
  }
  return false;
}

void trace_gpgpu_sim::launch(trace_kernel_info_t *kinfo) {
  unsigned cta_size = kinfo->threads_per_cta();
  if (cta_size > m_shader_config->n_thread_per_shader) {
    printf(
        "Execution error: Shader kernel CTA (block) size is too large for "
        "microarch config.\n");
    printf("                 CTA size (x*y*z) = %u, max supported = %u\n",
           cta_size, m_shader_config->n_thread_per_shader);
    printf(
        "                 => either change -gpgpu_shader argument in "
        "gpgpusim.config file or\n");
    printf(
        "                 modify the CUDA source to decrease the kernel block "
        "size.\n");
    abort();
  }
  unsigned n = 0;
  for (n = 0; n < m_running_kernels.size(); n++) {
    if ((NULL == m_running_kernels[n]) || m_running_kernels[n]->done()) {
      m_running_kernels[n] = kinfo;
      break;
    }
  }
  assert(n < m_running_kernels.size());
}

bool trace_gpgpu_sim::active() {
  if (m_config.gpu_max_cycle_opt &&
      (gpu_tot_sim_cycle + gpu_sim_cycle) >= m_config.gpu_max_cycle_opt)
    return false;
  if (m_config.gpu_max_insn_opt &&
      (gpu_tot_sim_insn + gpu_sim_insn) >= m_config.gpu_max_insn_opt)
    return false;
  if (m_config.gpu_max_cta_opt &&
      (gpu_tot_issued_cta >= m_config.gpu_max_cta_opt))
    return false;
  if (m_config.gpu_max_completed_cta_opt &&
      (gpu_completed_cta >= m_config.gpu_max_completed_cta_opt))
    return false;
  if (m_config.gpu_deadlock_detect && gpu_deadlock) return false;
  for (unsigned i = 0; i < m_shader_config->n_simt_clusters; i++)
    if (m_cluster[i]->get_not_completed() > 0) return true;
  ;
  for (unsigned i = 0; i < m_memory_config->m_n_mem; i++)
    if (m_memory_partition_unit[i]->busy() > 0) return true;
  ;
  if (icnt_busy()) return true;
  if (get_more_cta_left()) return true;
  return false;
}

// set this in gdb to single step the pipeline
unsigned long long g_single_step = 0;

// simpler version of the main loop, which does not use different clock domains.
void trace_gpgpu_sim::simple_cycle() {
  logger->info("=============== cycle {} ===============", gpu_sim_cycle);
  logger->info("");

  Instant start;
  Instant start_total = now();

  int clock_mask = next_clock_domain();
  bool simulate_clock_domains =
      m_shader_config->gpgpu_ctx->accelsim_compat_mode;
  if (simulate_clock_domains) {
    logger->trace("clock mask: {}", mask_to_string(std::bitset<8>(clock_mask)));
  }

  if (!simulate_clock_domains || clock_mask & CORE) {
    Instant start = now();
    for (unsigned i = 0; i < m_shader_config->n_simt_clusters; i++) {
      m_cluster[i]->icnt_cycle();
    }
    increment_timing("cycle::interconn", duration(now() - start));
  }

  unsigned partiton_replys_in_parallel_per_cycle = 0;
  // pop from memory controller to interconnect

  if (!simulate_clock_domains || clock_mask & ICNT) {
    logger->debug("POP from {} memory sub partitions",
                  m_memory_config->m_n_mem_sub_partition);

    Instant start = now();

    for (unsigned i = 0; i < m_memory_config->m_n_mem_sub_partition; i++) {
      logger->debug("checking sub partition[{}]:", i);
      logger->debug("\t icnt to l2 queue = {}",
                    *(m_memory_sub_partition[i]->m_icnt_L2_queue));
      logger->debug("\t l2 to icnt queue = {}",
                    *(m_memory_sub_partition[i]->m_L2_icnt_queue));
      logger->debug("\t l2 to dram queue = {}",
                    *(m_memory_sub_partition[i]->m_L2_dram_queue));
      logger->debug("\t dram to l2 queue = {}",
                    *(m_memory_sub_partition[i]->m_dram_L2_queue));

      unsigned partition_id = i / m_memory_config->m_n_mem_sub_partition;
      assert(partition_id < m_memory_config->m_n_mem);
      logger->debug(
          "\t dram latency queue ({:<3}) = [{}]",
          m_memory_partition_unit[partition_id]->m_dram_latency_queue.size(),
          fmt::join(m_memory_partition_unit[partition_id]->m_dram_latency_queue,
                    ","));

      logger->debug("");

      mem_fetch *mf = m_memory_sub_partition[i]->top();
      if (mf) {
        unsigned response_size =
            mf->get_is_write() ? mf->get_ctrl_size() : mf->size();
        if (::icnt_has_buffer(m_shader_config->mem2device(i), response_size)) {
          // if (!mf->get_is_write())
          mf->set_return_timestamp(gpu_sim_cycle + gpu_tot_sim_cycle);
          mf->set_status(IN_ICNT_TO_SHADER, gpu_sim_cycle + gpu_tot_sim_cycle);
          ::icnt_push(m_shader_config->mem2device(i), mf->get_tpc(), mf,
                      response_size);
          m_memory_sub_partition[i]->pop();
          partiton_replys_in_parallel_per_cycle++;
        } else {
          gpu_stall_icnt2sh++;
        }
      } else {
        m_memory_sub_partition[i]->pop();
      }
    }

    increment_timing("cycle::subpartitions", duration(now() - start));
  }
  partiton_replys_in_parallel += partiton_replys_in_parallel_per_cycle;

  if (!simulate_clock_domains || clock_mask & DRAM) {
    logger->debug("cycle for {} drams", m_memory_config->m_n_mem);
    Instant start = now();
    for (unsigned i = 0; i < m_memory_config->m_n_mem; i++) {
      if (m_memory_config->simple_dram_model) {
        m_memory_partition_unit[i]->simple_dram_model_cycle();
      } else {
        // Issue the dram command (scheduler + delay model)
        m_memory_partition_unit[i]->dram_cycle();
      }
    }
    increment_timing("cycle::dram", duration(now() - start));
  }

  unsigned partiton_reqs_in_parallel_per_cycle = 0;
  if (!simulate_clock_domains || clock_mask & L2) {
    logger->debug("moving mem requests from interconn to {} mem partitions",
                  m_memory_config->m_n_mem_sub_partition);
    Instant start = now();

    for (unsigned i = 0; i < m_memory_config->m_n_mem_sub_partition; i++) {
      // move memory request from interconnect into memory partition (if not
      // backed up) Note:This needs to be called in DRAM clock domain if there
      // is no L2 cache in the system In the worst case, we may need to push
      // SECTOR_CHUNCK_SIZE requests, so ensure you have enough buffer for them
      unsigned device = m_shader_config->mem2device(i);
      if (m_memory_sub_partition[i]->full(SECTOR_CHUNCK_SIZE)) {
        logger->debug("SKIP sub partition {} ({}): DRAM full stall", i, device);
        gpu_stall_dramfull++;
      } else {
        mem_fetch *mf = (mem_fetch *)icnt_pop(device);
        if (mf) {
          logger->debug("got new fetch {} for mem sub partition {} ({})",
                        mem_fetch_ptr(mf), i, device);
          m_memory_sub_partition[i]->push(mf,
                                          gpu_sim_cycle + gpu_tot_sim_cycle);
        }
        if (mf) partiton_reqs_in_parallel_per_cycle++;
      }
      m_memory_sub_partition[i]->cache_cycle(gpu_sim_cycle + gpu_tot_sim_cycle);
    }
    increment_timing("cycle::l2", duration(now() - start));
  }

  partiton_reqs_in_parallel += partiton_reqs_in_parallel_per_cycle;
  if (partiton_reqs_in_parallel_per_cycle > 0) {
    partiton_reqs_in_parallel_util += partiton_reqs_in_parallel_per_cycle;
    gpu_sim_cycle_parition_util++;
  }

  if (!simulate_clock_domains || clock_mask & ICNT) {
    // logger->debug("icnt transfer");
    icnt_transfer();
  }

  // L1 cache + shader core pipeline stages
  if (!simulate_clock_domains || clock_mask & CORE) {
    logger->debug("core cycle for {} clusters",
                  m_shader_config->n_simt_clusters);
    start = now();

    for (unsigned i = 0; i < m_shader_config->n_simt_clusters; i++) {
      if (m_cluster[i]->get_not_completed() || get_more_cta_left()) {
        m_cluster[i]->core_cycle();
        *active_sms += m_cluster[i]->get_n_active_sms();
      }
      m_cluster[i]->get_current_occupancy(
          gpu_occupancy.aggregate_warp_slot_filled,
          gpu_occupancy.aggregate_theoretical_warp_slots);
    }

    increment_timing("cycle::core", duration(now() - start));

    float temp = 0;
    for (unsigned i = 0; i < m_shader_config->num_shader(); i++) {
      temp += m_shader_stats->m_pipeline_duty_cycle[i];
    }
    temp = temp / m_shader_config->num_shader();
    *average_pipeline_duty_cycle = ((*average_pipeline_duty_cycle) + temp);

    gpu_sim_cycle++;

    start = now();
    issue_block2core();
    increment_timing("cycle::issue_block_to_core", duration(now() - start));

    decrement_kernel_latency();

    // Depending on configuration, invalidate the caches once all threads
    // completed.
    unsigned not_completed = 1;
    bool all_threads_complete = true;
    if (m_config.gpgpu_flush_l1_cache) {
      logger->debug("flushing l1 caches");
      for (unsigned i = 0; i < m_shader_config->n_simt_clusters; i++) {
        unsigned cluster_not_completed = m_cluster[i]->get_not_completed();
        logger->trace("cluster {}: {} not completed", i, cluster_not_completed);
        if (cluster_not_completed == 0) {
          m_cluster[i]->cache_invalidate();
        } else {
          not_completed += cluster_not_completed;
          all_threads_complete = false;
        }
      }
      logger->trace("all threads completed: {} ({} not completed)",
                    all_threads_complete, not_completed);
    }

    if (m_config.gpgpu_flush_l2_cache) {
      if (!m_config.gpgpu_flush_l1_cache) {
        for (unsigned i = 0; i < m_shader_config->n_simt_clusters; i++) {
          if (m_cluster[i]->get_not_completed() != 0) {
            all_threads_complete = 0;
            break;
          }
        }
      }

      if (all_threads_complete && !m_memory_config->m_L2_config.disabled()) {
        logger->debug("flushing l2 caches");
        if (m_memory_config->m_L2_config.get_num_lines()) {
          int dlc = 0;
          for (unsigned i = 0; i < m_memory_config->m_n_mem; i++) {
            dlc = m_memory_sub_partition[i]->flushL2();
            assert(dlc == 0);  // TODO: need to model actual writes to DRAM here
            logger->debug("dirty lines flushed from L2 {} is {}", i, dlc);
          }
        }
      }
    }

    increment_timing("cycle::total", duration(now() - start_total));
  }
}

void trace_gpgpu_sim::cycle() {
  // clock mask is which clock domains are active in this cycle (core, icnt)
  // due to the different frequencies
  int clock_mask = next_clock_domain();

  if (clock_mask & CORE) {
    // shader core loading (pop from ICNT into core) follows CORE clock
    for (unsigned i = 0; i < m_shader_config->n_simt_clusters; i++)
      m_cluster[i]->icnt_cycle();
  }
  unsigned partiton_replys_in_parallel_per_cycle = 0;
  if (clock_mask & ICNT) {
    // pop from memory controller to interconnect
    for (unsigned i = 0; i < m_memory_config->m_n_mem_sub_partition; i++) {
      mem_fetch *mf = m_memory_sub_partition[i]->top();
      if (mf) {
        unsigned response_size =
            mf->get_is_write() ? mf->get_ctrl_size() : mf->size();
        if (::icnt_has_buffer(m_shader_config->mem2device(i), response_size)) {
          // if (!mf->get_is_write())
          mf->set_return_timestamp(gpu_sim_cycle + gpu_tot_sim_cycle);
          mf->set_status(IN_ICNT_TO_SHADER, gpu_sim_cycle + gpu_tot_sim_cycle);
          logger->debug("trace_gpgpu_sim: icnt_push({})", mf->get_addr());
          ::icnt_push(m_shader_config->mem2device(i), mf->get_tpc(), mf,
                      response_size);
          m_memory_sub_partition[i]->pop();
          partiton_replys_in_parallel_per_cycle++;
        } else {
          gpu_stall_icnt2sh++;
        }
      } else {
        m_memory_sub_partition[i]->pop();
      }
    }
  }
  partiton_replys_in_parallel += partiton_replys_in_parallel_per_cycle;

  if (clock_mask & DRAM) {
    for (unsigned i = 0; i < m_memory_config->m_n_mem; i++) {
      if (m_memory_config->simple_dram_model) {
        m_memory_partition_unit[i]->simple_dram_model_cycle();
      } else {
        // Issue the dram command (scheduler + delay model)
        m_memory_partition_unit[i]->dram_cycle();
      }

      // REMOVE: power
      // Update performance counters for DRAM
      // m_memory_partition_unit[i]->set_dram_power_stats(
      //     m_power_stats->pwr_mem_stat->n_cmd[CURRENT_STAT_IDX][i],
      //     m_power_stats->pwr_mem_stat->n_activity[CURRENT_STAT_IDX][i],
      //     m_power_stats->pwr_mem_stat->n_nop[CURRENT_STAT_IDX][i],
      //     m_power_stats->pwr_mem_stat->n_act[CURRENT_STAT_IDX][i],
      //     m_power_stats->pwr_mem_stat->n_pre[CURRENT_STAT_IDX][i],
      //     m_power_stats->pwr_mem_stat->n_rd[CURRENT_STAT_IDX][i],
      //     m_power_stats->pwr_mem_stat->n_wr[CURRENT_STAT_IDX][i],
      //     m_power_stats->pwr_mem_stat->n_wr_WB[CURRENT_STAT_IDX][i],
      //     m_power_stats->pwr_mem_stat->n_req[CURRENT_STAT_IDX][i]);
    }
  }

  // L2 operations follow L2 clock domain
  unsigned partiton_reqs_in_parallel_per_cycle = 0;
  if (clock_mask & L2) {
    // REMOVE: power
    // m_power_stats->pwr_mem_stat->l2_cache_stats[CURRENT_STAT_IDX].clear();
    for (unsigned i = 0; i < m_memory_config->m_n_mem_sub_partition; i++) {
      // move memory request from interconnect into memory partition (if not
      // backed up) Note:This needs to be called in DRAM clock domain if there
      // is no L2 cache in the system In the worst case, we may need to push
      // SECTOR_CHUNCK_SIZE requests, so ensure you have enough buffer for them
      if (m_memory_sub_partition[i]->full(SECTOR_CHUNCK_SIZE)) {
        gpu_stall_dramfull++;
      } else {
        mem_fetch *mf = (mem_fetch *)icnt_pop(m_shader_config->mem2device(i));
        if (mf) {
          logger->debug("got new fetch {} for mem sub partition {} ({})",
                        mem_fetch_ptr(mf), i, m_shader_config->mem2device(i));

          m_memory_sub_partition[i]->push(mf,
                                          gpu_sim_cycle + gpu_tot_sim_cycle);
          partiton_reqs_in_parallel_per_cycle++;
        }
      }
      m_memory_sub_partition[i]->cache_cycle(gpu_sim_cycle + gpu_tot_sim_cycle);
      // REMOVE: power
      // m_memory_sub_partition[i]->accumulate_L2cache_stats(
      //     m_power_stats->pwr_mem_stat->l2_cache_stats[CURRENT_STAT_IDX]);
    }
  }
  partiton_reqs_in_parallel += partiton_reqs_in_parallel_per_cycle;
  if (partiton_reqs_in_parallel_per_cycle > 0) {
    partiton_reqs_in_parallel_util += partiton_reqs_in_parallel_per_cycle;
    gpu_sim_cycle_parition_util++;
  }

  if (clock_mask & ICNT) {
    icnt_transfer();
  }

  if (clock_mask & CORE) {
    // L1 cache + shader core pipeline stages

    // REMOVE: power
    // m_power_stats->pwr_mem_stat->core_cache_stats[CURRENT_STAT_IDX].clear();
    for (unsigned i = 0; i < m_shader_config->n_simt_clusters; i++) {
      if (m_cluster[i]->get_not_completed() || get_more_cta_left()) {
        m_cluster[i]->core_cycle();
        *active_sms += m_cluster[i]->get_n_active_sms();
      }
      // REMOVE: power
      // Update core icnt/cache stats for AccelWattch
      // m_cluster[i]->get_icnt_stats(
      //     m_power_stats->pwr_mem_stat->n_simt_to_mem[CURRENT_STAT_IDX][i],
      //     m_power_stats->pwr_mem_stat->n_mem_to_simt[CURRENT_STAT_IDX][i]);
      // m_cluster[i]->get_cache_stats(
      //     m_power_stats->pwr_mem_stat->core_cache_stats[CURRENT_STAT_IDX]);
      m_cluster[i]->get_current_occupancy(
          gpu_occupancy.aggregate_warp_slot_filled,
          gpu_occupancy.aggregate_theoretical_warp_slots);
    }
    float temp = 0;
    for (unsigned i = 0; i < m_shader_config->num_shader(); i++) {
      temp += m_shader_stats->m_pipeline_duty_cycle[i];
    }
    temp = temp / m_shader_config->num_shader();
    *average_pipeline_duty_cycle = ((*average_pipeline_duty_cycle) + temp);
    // cout<<"Average pipeline duty cycle:
    // "<<*average_pipeline_duty_cycle<<endl;

    if (g_single_step &&
        ((gpu_sim_cycle + gpu_tot_sim_cycle) >= g_single_step)) {
      raise(SIGTRAP);  // Debug breakpoint
    }
    gpu_sim_cycle++;

    // if (g_interactive_debugger_enabled)
    //   gpgpu_debug();

    // REMOVE: power
    // McPAT main cycle (interface with McPAT)
    // #ifdef GPGPUSIM_POWER_MODEL
    //     if (m_config.g_power_simulation_enabled) {
    //       if (m_config.g_power_simulation_mode == 0) {
    //         mcpat_cycle(m_config, getShaderCoreConfig(), m_gpgpusim_wrapper,
    //                     m_power_stats, m_config.gpu_stat_sample_freq,
    //                     gpu_tot_sim_cycle, gpu_sim_cycle, gpu_tot_sim_insn,
    //                     gpu_sim_insn, m_config.g_dvfs_enabled);
    //       }
    //     }
    // #endif

    issue_block2core();
    decrement_kernel_latency();

    // Depending on configuration, invalidate the caches once all of threads are
    // completed.
    int all_threads_complete = 1;
    if (m_config.gpgpu_flush_l1_cache) {
      for (unsigned i = 0; i < m_shader_config->n_simt_clusters; i++) {
        if (m_cluster[i]->get_not_completed() == 0)
          m_cluster[i]->cache_invalidate();
        else
          all_threads_complete = 0;
      }
    }

    if (m_config.gpgpu_flush_l2_cache) {
      if (!m_config.gpgpu_flush_l1_cache) {
        for (unsigned i = 0; i < m_shader_config->n_simt_clusters; i++) {
          if (m_cluster[i]->get_not_completed() != 0) {
            all_threads_complete = 0;
            break;
          }
        }
      }

      if (all_threads_complete && !m_memory_config->m_L2_config.disabled()) {
        logger->debug("Flushed L2 caches...");
        if (m_memory_config->m_L2_config.get_num_lines()) {
          int dlc = 0;
          for (unsigned i = 0; i < m_memory_config->m_n_mem; i++) {
            dlc = m_memory_sub_partition[i]->flushL2();
            assert(dlc == 0);  // TODO: need to model actual writes to DRAM here
            logger->debug("Dirty lines flushed from L2 {} is {}", i, dlc);
          }
        }
      }
    }

    if (!(gpu_sim_cycle % m_config.gpu_stat_sample_freq)) {
      time_t days, hrs, minutes, sec;
      time_t curr_time;
      time(&curr_time);
      unsigned long long elapsed_time =
          MAX(curr_time - gpgpu_ctx->the_gpgpusim->g_simulation_starttime, 1);
      if ((elapsed_time - last_liveness_message_time) >=
              m_config.liveness_message_freq &&
          DTRACE(LIVENESS)) {
        days = elapsed_time / (3600 * 24);
        hrs = elapsed_time / 3600 - 24 * days;
        minutes = elapsed_time / 60 - 60 * (hrs + 24 * days);
        sec = elapsed_time - 60 * (minutes + 60 * (hrs + 24 * days));

        unsigned long long active = 0, total = 0;
        for (unsigned i = 0; i < m_shader_config->n_simt_clusters; i++) {
          m_cluster[i]->get_current_occupancy(active, total);
        }
        DPRINTFG(LIVENESS,
                 "uArch: inst.: %lld (ipc=%4.1f, occ=%0.4f\% [%llu / %llu]) "
                 "sim_rate=%u (inst/sec) elapsed = %u:%u:%02u:%02u / %s",
                 gpu_tot_sim_insn + gpu_sim_insn,
                 (double)gpu_sim_insn / (double)gpu_sim_cycle,
                 float(active) / float(total) * 100, active, total,
                 (unsigned)((gpu_tot_sim_insn + gpu_sim_insn) / elapsed_time),
                 (unsigned)days, (unsigned)hrs, (unsigned)minutes,
                 (unsigned)sec, ctime(&curr_time));
        fflush(stats_out);
        fflush(stdout);
        last_liveness_message_time = elapsed_time;
      }
      // visualizer_printstat();
      m_memory_stats->memlatstat_lat_pw();
      if (m_config.gpgpu_runtime_stat &&
          (m_config.gpu_runtime_stat_flag != 0)) {
        if (m_config.gpu_runtime_stat_flag & GPU_RSTAT_BW_STAT) {
          for (unsigned i = 0; i < m_memory_config->m_n_mem; i++)
            m_memory_partition_unit[i]->print_stat(stats_out);
          fprintf(stats_out, "maxmrqlatency = %d \n",
                  m_memory_stats->max_mrq_latency);
          fprintf(stats_out, "maxmflatency = %d \n",
                  m_memory_stats->max_mf_latency);
        }
        if (m_config.gpu_runtime_stat_flag & GPU_RSTAT_SHD_INFO)
          shader_print_runtime_stat(stats_out);
        if (m_config.gpu_runtime_stat_flag & GPU_RSTAT_L1MISS)
          shader_print_l1_miss_stat(stats_out);
        if (m_config.gpu_runtime_stat_flag & GPU_RSTAT_SCHED)
          shader_print_scheduler_stat(stats_out, false);
      }
    }

    if (!(gpu_sim_cycle % 50000)) {
      // deadlock detection
      if (m_config.gpu_deadlock_detect && gpu_sim_insn == last_gpu_sim_insn) {
        gpu_deadlock = true;
      } else {
        last_gpu_sim_insn = gpu_sim_insn;
      }
    }
    try_snap_shot(gpu_sim_cycle);
    spill_log_to_file(stats_out, 0, gpu_sim_cycle);

#if (CUDART_VERSION >= 5000)
    // launch device kernel
    gpgpu_ctx->device_runtime->launch_one_device_kernel();
#endif
  }
}

void trace_gpgpu_sim::deadlock_check() {
  if (m_config.gpu_deadlock_detect && gpu_deadlock) {
    fflush(stats_out);
    fflush(stdout);
    printf(
        "\n\nGPGPU-Sim uArch: ERROR ** deadlock detected: last writeback core "
        "%u @ gpu_sim_cycle %u (+ gpu_tot_sim_cycle %u) (%u cycles ago)\n",
        gpu_sim_insn_last_update_sid, (unsigned)gpu_sim_insn_last_update,
        (unsigned)(gpu_tot_sim_cycle - gpu_sim_cycle),
        (unsigned)(gpu_sim_cycle - gpu_sim_insn_last_update));
    unsigned num_cores = 0;
    for (unsigned i = 0; i < m_shader_config->n_simt_clusters; i++) {
      unsigned not_completed = m_cluster[i]->get_not_completed();
      if (not_completed) {
        if (!num_cores) {
          printf(
              "GPGPU-Sim uArch: DEADLOCK  shader cores no longer committing "
              "instructions [core(# threads)]:\n");
          printf("GPGPU-Sim uArch: DEADLOCK  ");
          m_cluster[i]->print_not_completed(stdout);
        } else if (num_cores < 8) {
          m_cluster[i]->print_not_completed(stdout);
        } else if (num_cores >= 8) {
          printf(" + others ... ");
        }
        num_cores += m_shader_config->n_simt_cores_per_cluster;
      }
    }
    printf("\n");
    for (unsigned i = 0; i < m_memory_config->m_n_mem; i++) {
      bool busy = m_memory_partition_unit[i]->busy();
      if (busy)
        printf("GPGPU-Sim uArch DEADLOCK:  memory partition %u busy\n", i);
    }
    if (icnt_busy()) {
      printf("GPGPU-Sim uArch DEADLOCK:  iterconnect contains traffic\n");
      icnt_display_state(stdout);
    }
    printf(
        "\nRe-run the simulator in gdb and use debug routines in .gdbinit to "
        "debug this\n");
    fflush(stdout);
    abort();
  }
}

void trace_gpgpu_sim::gpu_print_stat(FILE *fp) {
  std::string kernel_info_str = executed_kernel_info_string();
  fprintf(fp, "%s", kernel_info_str.c_str());

  fprintf(fp, "gpu_sim_cycle = %lld\n", gpu_sim_cycle);
  fprintf(fp, "gpu_sim_insn = %lld\n", gpu_sim_insn);
  fprintf(fp, "gpu_ipc = %12.4f\n", (float)gpu_sim_insn / gpu_sim_cycle);
  fprintf(fp, "gpu_tot_sim_cycle = %lld\n", gpu_tot_sim_cycle + gpu_sim_cycle);
  fprintf(fp, "gpu_tot_sim_insn = %lld\n", gpu_tot_sim_insn + gpu_sim_insn);
  fprintf(fp, "gpu_tot_ipc = %12.4f\n",
          (float)(gpu_tot_sim_insn + gpu_sim_insn) /
              (gpu_tot_sim_cycle + gpu_sim_cycle));
  fprintf(fp, "gpu_tot_issued_cta = %lld\n",
          gpu_tot_issued_cta + m_total_cta_launched);
  fprintf(fp, "gpu_occupancy = %.4f%% \n",
          gpu_occupancy.get_occ_fraction() * 100);
  fprintf(fp, "gpu_tot_occupancy = %.4f%% \n",
          (gpu_occupancy + gpu_tot_occupancy).get_occ_fraction() * 100);

  fprintf(fp, "max_total_param_size = %llu\n",
          gpgpu_ctx->device_runtime->g_max_total_param_size);

  // performance counter for stalls due to congestion.
  fprintf(fp, "gpu_stall_dramfull = %d\n", gpu_stall_dramfull);
  fprintf(fp, "gpu_stall_icnt2sh    = %d\n", gpu_stall_icnt2sh);

  // printf("partiton_reqs_in_parallel = %lld\n", partiton_reqs_in_parallel);
  // printf("partiton_reqs_in_parallel_total    = %lld\n",
  // partiton_reqs_in_parallel_total );
  fprintf(fp, "partiton_level_parallism = %12.4f\n",
          (float)partiton_reqs_in_parallel / gpu_sim_cycle);
  fprintf(fp, "partiton_level_parallism_total  = %12.4f\n",
          (float)(partiton_reqs_in_parallel + partiton_reqs_in_parallel_total) /
              (gpu_tot_sim_cycle + gpu_sim_cycle));
  // fprintf("partiton_reqs_in_parallel_util = %lld\n",
  // partiton_reqs_in_parallel_util);
  // fprintf("partiton_reqs_in_parallel_util_total    = %lld\n",
  // partiton_reqs_in_parallel_util_total );
  // fprintf("gpu_sim_cycle_parition_util = %lld\n",
  // gpu_sim_cycle_parition_util); fprintf("gpu_tot_sim_cycle_parition_util    =
  // %lld\n", gpu_tot_sim_cycle_parition_util );
  fprintf(fp, "partiton_level_parallism_util = %12.4f\n",
          (float)partiton_reqs_in_parallel_util / gpu_sim_cycle_parition_util);
  fprintf(fp, "partiton_level_parallism_util_total  = %12.4f\n",
          (float)(partiton_reqs_in_parallel_util +
                  partiton_reqs_in_parallel_util_total) /
              (gpu_sim_cycle_parition_util + gpu_tot_sim_cycle_parition_util));
  // fprintf("partiton_replys_in_parallel = %lld\n",
  // partiton_replys_in_parallel); fprintf("partiton_replys_in_parallel_total =
  // %lld\n", partiton_replys_in_parallel_total );
  fprintf(fp, "L2_BW  = %12.4f GB/Sec\n",
          ((float)(partiton_replys_in_parallel * 32) /
           (gpu_sim_cycle * m_config.icnt_period)) /
              1000000000);
  fprintf(fp, "L2_BW_total  = %12.4f GB/Sec\n",
          ((float)((partiton_replys_in_parallel +
                    partiton_replys_in_parallel_total) *
                   32) /
           ((gpu_tot_sim_cycle + gpu_sim_cycle) * m_config.icnt_period)) /
              1000000000);

  time_t curr_time;
  time(&curr_time);
  unsigned long long elapsed_time =
      MAX(curr_time - gpgpu_ctx->the_gpgpusim->g_simulation_starttime, 1);
  fprintf(fp, "gpu_total_sim_rate=%u\n",
          (unsigned)((gpu_tot_sim_insn + gpu_sim_insn) / elapsed_time));

  // shader_print_l1_miss_stat(fp);
  shader_print_cache_stats(fp);

  cache_stats core_cache_stats;
  core_cache_stats.clear();
  for (unsigned i = 0; i < m_config.num_cluster(); i++) {
    m_cluster[i]->get_cache_stats(core_cache_stats);
  }
  fprintf(fp, "\nTotal_core_cache_stats:\n");
  core_cache_stats.print_stats(fp, "Total_core_cache_stats_breakdown");
  fprintf(fp, "\nTotal_core_cache_fail_stats:\n");
  core_cache_stats.print_fail_stats(fp,
                                    "Total_core_cache_fail_stats_breakdown");
  shader_print_scheduler_stat(fp, false);

  m_shader_stats->print(fp);
  // REMOVE: power
  // #ifdef GPGPUSIM_POWER_MODEL
  //   if (m_config.g_power_simulation_enabled) {
  //     if (m_config.g_power_simulation_mode > 0) {
  //       // if(!m_config.g_aggregate_power_stats)
  //       mcpat_reset_perf_count(m_gpgpusim_wrapper);
  //       calculate_hw_mcpat(m_config, getShaderCoreConfig(),
  //       m_gpgpusim_wrapper,
  //                          m_power_stats, m_config.gpu_stat_sample_freq,
  //                          gpu_tot_sim_cycle, gpu_sim_cycle,
  //                          gpu_tot_sim_insn, gpu_sim_insn,
  //                          m_config.g_power_simulation_mode,
  //                          m_config.g_dvfs_enabled,
  //                          m_config.g_hw_perf_file_name,
  //                          m_config.g_hw_perf_bench_name,
  //                          executed_kernel_name(),
  //                          m_config.accelwattch_hybrid_configuration,
  //                          m_config.g_aggregate_power_stats);
  //     }
  //     m_gpgpusim_wrapper->print_power_kernel_stats(
  //         gpu_sim_cycle, gpu_tot_sim_cycle, gpu_tot_sim_insn + gpu_sim_insn,
  //         kernel_info_str, true);
  //     // if(!m_config.g_aggregate_power_stats)
  //     mcpat_reset_perf_count(m_gpgpusim_wrapper);
  //   }
  // #endif

  // performance counter that are not local to one shader
  m_memory_stats->memlatstat_print(fp, m_memory_config->m_n_mem,
                                   m_memory_config->nbk);
  // for (unsigned i = 0; i < m_memory_config->m_n_mem; i++)
  //   m_memory_partition_unit[i]->print(stdout);

  // L2 cache stats
  if (!m_memory_config->m_L2_config.disabled()) {
    cache_stats l2_stats;
    struct cache_sub_stats l2_css;
    struct cache_sub_stats total_l2_css;
    l2_stats.clear();
    l2_css.clear();
    total_l2_css.clear();

    fprintf(fp, "\n========= L2 cache stats =========\n");
    for (unsigned i = 0; i < m_memory_config->m_n_mem_sub_partition; i++) {
      m_memory_sub_partition[i]->accumulate_L2cache_stats(l2_stats);
      m_memory_sub_partition[i]->get_L2cache_sub_stats(l2_css);

      fprintf(fp,
              "L2_cache_bank[%d]: Access = %llu, Miss = %llu, Miss_rate = "
              "%.3lf, Pending_hits = %llu, Reservation_fails = %llu\n",
              i, l2_css.accesses, l2_css.misses,
              (double)l2_css.misses / (double)l2_css.accesses,
              l2_css.pending_hits, l2_css.res_fails);

      total_l2_css += l2_css;
    }
    if (!m_memory_config->m_L2_config.disabled() &&
        m_memory_config->m_L2_config.get_num_lines()) {
      // L2c_print_cache_stat(fp);
      fprintf(fp, "L2_total_cache_accesses = %llu\n", total_l2_css.accesses);
      fprintf(fp, "L2_total_cache_misses = %llu\n", total_l2_css.misses);
      if (total_l2_css.accesses > 0)
        fprintf(fp, "L2_total_cache_miss_rate = %.4lf\n",
                (double)total_l2_css.misses / (double)total_l2_css.accesses);
      fprintf(fp, "L2_total_cache_pending_hits = %llu\n",
              total_l2_css.pending_hits);
      fprintf(fp, "L2_total_cache_reservation_fails = %llu\n",
              total_l2_css.res_fails);
      fprintf(fp, "L2_total_cache_breakdown:\n");
      l2_stats.print_stats(fp, "L2_cache_stats_breakdown");
      fprintf(fp, "L2_total_cache_reservation_fail_breakdown:\n");
      l2_stats.print_fail_stats(fp, "L2_cache_stats_fail_breakdown");
      total_l2_css.print_port_stats(fp, "L2_cache");
    }
  }

  if (m_config.gpgpu_cflog_interval != 0) {
    spill_log_to_file(fp, 1, gpu_sim_cycle);
    insn_warp_occ_print(fp);
  }
  if (gpgpu_ctx->func_sim->gpgpu_ptx_instruction_classification) {
    StatDisp(fp, gpgpu_ctx->func_sim->g_inst_classification_stat
                     [gpgpu_ctx->func_sim->g_ptx_kernel_count]);
    StatDisp(fp, gpgpu_ctx->func_sim->g_inst_op_classification_stat
                     [gpgpu_ctx->func_sim->g_ptx_kernel_count]);
  }

  // REMOVE: power
  // #ifdef GPGPUSIM_POWER_MODEL
  //   if (m_config.g_power_simulation_enabled) {
  //     m_gpgpusim_wrapper->detect_print_steady_state(1, gpu_tot_sim_insn +
  //                                                          gpu_sim_insn);
  //   }
  // #endif

  // Interconnect power stat print
  long total_simt_to_mem = 0;
  long total_mem_to_simt = 0;
  long temp_stm = 0;
  long temp_mts = 0;
  for (unsigned i = 0; i < m_config.num_cluster(); i++) {
    m_cluster[i]->get_icnt_stats(temp_stm, temp_mts);
    total_simt_to_mem += temp_stm;
    total_mem_to_simt += temp_mts;
  }
  fprintf(fp, "\nicnt_total_pkts_mem_to_simt=%ld\n", total_mem_to_simt);
  fprintf(fp, "icnt_total_pkts_simt_to_mem=%ld\n", total_simt_to_mem);

  time_vector_print(fp);
  fflush(fp);

  clear_executed_kernel_info();
}

void trace_gpgpu_sim::print_stats(FILE *fp) {
  // REMOVE: ptx
  // gpgpu_ctx->stats->ptx_file_line_stats_write_file();
  gpu_print_stat(fp);

  if (g_network_mode) {
    fprintf(
        fp,
        "----------------------------Interconnect-DETAILS----------------------"
        "----------\n");
    icnt_display_stats(fp);
    icnt_display_overall_stats(fp);
    fprintf(
        fp,
        "----------------------------END-of-Interconnect-DETAILS---------------"
        "----------\n");
  }
}

void trace_gpgpu_sim::update_stats() {
  m_memory_stats->memlatstat_lat_pw();
  gpu_tot_sim_cycle += gpu_sim_cycle;
  gpu_tot_sim_insn += gpu_sim_insn;
  gpu_tot_issued_cta += m_total_cta_launched;
  partiton_reqs_in_parallel_total += partiton_reqs_in_parallel;
  partiton_replys_in_parallel_total += partiton_replys_in_parallel;
  partiton_reqs_in_parallel_util_total += partiton_reqs_in_parallel_util;
  gpu_tot_sim_cycle_parition_util += gpu_sim_cycle_parition_util;
  gpu_tot_occupancy += gpu_occupancy;

  gpu_sim_cycle = 0;
  partiton_reqs_in_parallel = 0;
  partiton_replys_in_parallel = 0;
  partiton_reqs_in_parallel_util = 0;
  gpu_sim_cycle_parition_util = 0;
  gpu_sim_insn = 0;
  m_total_cta_launched = 0;
  gpu_completed_cta = 0;
  gpu_occupancy = occupancy_stats();
}

bool trace_gpgpu_sim::get_more_cta_left() const {
  if (hit_max_cta_count()) return false;

  for (unsigned n = 0; n < m_running_kernels.size(); n++) {
    if (m_running_kernels[n] && !m_running_kernels[n]->no_more_ctas_to_run())
      return true;
  }
  return false;
}

void trace_gpgpu_sim::issue_block2core() {
  logger->debug("===> issue block to core");
  unsigned last_issued = m_last_cluster_issue;
  for (unsigned i = 0; i < m_shader_config->n_simt_clusters; i++) {
    unsigned idx = (i + last_issued + 1) % m_shader_config->n_simt_clusters;
    unsigned num = m_cluster[idx]->issue_block2core();
    logger->trace("cluster[{}] issued {} blocks", idx, num);
    if (num) {
      m_last_cluster_issue = idx;
      m_total_cta_launched += num;
    }
  }
}

void trace_gpgpu_sim::shader_print_runtime_stat(FILE *fout) {}

void trace_gpgpu_sim::shader_print_scheduler_stat(
    FILE *fout, bool print_dynamic_info) const {
  fprintf(fout, "ctas_completed %d, ", m_shader_stats->ctas_completed);
  // Print out the stats from the sampling shader core
  const unsigned scheduler_sampling_core =
      m_shader_config->gpgpu_warp_issue_shader;
#define STR_SIZE 55
  char name_buff[STR_SIZE];
  name_buff[STR_SIZE - 1] = '\0';
  const std::vector<unsigned> &distro =
      print_dynamic_info
          ? m_shader_stats->get_dynamic_warp_issue()[scheduler_sampling_core]
          : m_shader_stats->get_warp_slot_issue()[scheduler_sampling_core];
  if (print_dynamic_info) {
    snprintf(name_buff, STR_SIZE - 1, "dynamic_warp_id");
  } else {
    snprintf(name_buff, STR_SIZE - 1, "warp_id");
  }
  fprintf(fout, "Shader %d %s issue ditsribution:\n", scheduler_sampling_core,
          name_buff);
  const unsigned num_warp_ids = distro.size();
  // First print out the warp ids
  fprintf(fout, "%s:\n", name_buff);
  for (unsigned warp_id = 0; warp_id < num_warp_ids; ++warp_id) {
    fprintf(fout, "%d, ", warp_id);
  }

  fprintf(fout, "\ndistro:\n");
  // Then print out the distribution of instuctions issued
  for (std::vector<unsigned>::const_iterator iter = distro.begin();
       iter != distro.end(); iter++) {
    fprintf(fout, "%d, ", *iter);
  }
  fprintf(fout, "\n");
}

void trace_gpgpu_sim::shader_print_cache_stats(FILE *fout) const {
  // L1I
  struct cache_sub_stats total_css;
  struct cache_sub_stats css;

  if (!m_shader_config->m_L1I_config.disabled()) {
    total_css.clear();
    css.clear();
    fprintf(fout, "\n========= Core cache stats =========\n");
    fprintf(fout, "L1I_cache:\n");
    for (unsigned i = 0; i < m_shader_config->n_simt_clusters; ++i) {
      m_cluster[i]->get_L1I_sub_stats(css);
      total_css += css;
    }
    fprintf(fout, "\tL1I_total_cache_accesses = %llu\n", total_css.accesses);
    fprintf(fout, "\tL1I_total_cache_misses = %llu\n", total_css.misses);
    if (total_css.accesses > 0) {
      fprintf(fout, "\tL1I_total_cache_miss_rate = %.4lf\n",
              (double)total_css.misses / (double)total_css.accesses);
    }
    fprintf(fout, "\tL1I_total_cache_pending_hits = %llu\n",
            total_css.pending_hits);
    fprintf(fout, "\tL1I_total_cache_reservation_fails = %llu\n",
            total_css.res_fails);
  }

  // L1D
  if (!m_shader_config->m_L1D_config.disabled()) {
    total_css.clear();
    css.clear();
    fprintf(fout, "L1D_cache:\n");
    for (unsigned i = 0; i < m_shader_config->n_simt_clusters; i++) {
      m_cluster[i]->get_L1D_sub_stats(css);

      fprintf(fout,
              "\tL1D_cache_core[%d]: Access = %llu, Miss = %llu, Miss_rate = "
              "%.3lf, Pending_hits = %llu, Reservation_fails = %llu\n",
              i, css.accesses, css.misses,
              (double)css.misses / (double)css.accesses, css.pending_hits,
              css.res_fails);

      total_css += css;
    }
    fprintf(fout, "\tL1D_total_cache_accesses = %llu\n", total_css.accesses);
    fprintf(fout, "\tL1D_total_cache_misses = %llu\n", total_css.misses);
    if (total_css.accesses > 0) {
      fprintf(fout, "\tL1D_total_cache_miss_rate = %.4lf\n",
              (double)total_css.misses / (double)total_css.accesses);
    }
    fprintf(fout, "\tL1D_total_cache_pending_hits = %llu\n",
            total_css.pending_hits);
    fprintf(fout, "\tL1D_total_cache_reservation_fails = %llu\n",
            total_css.res_fails);
    total_css.print_port_stats(fout, "\tL1D_cache");
  }

  // L1C
  if (!m_shader_config->m_L1C_config.disabled()) {
    total_css.clear();
    css.clear();
    fprintf(fout, "L1C_cache:\n");
    for (unsigned i = 0; i < m_shader_config->n_simt_clusters; ++i) {
      m_cluster[i]->get_L1C_sub_stats(css);
      total_css += css;
    }
    fprintf(fout, "\tL1C_total_cache_accesses = %llu\n", total_css.accesses);
    fprintf(fout, "\tL1C_total_cache_misses = %llu\n", total_css.misses);
    if (total_css.accesses > 0) {
      fprintf(fout, "\tL1C_total_cache_miss_rate = %.4lf\n",
              (double)total_css.misses / (double)total_css.accesses);
    }
    fprintf(fout, "\tL1C_total_cache_pending_hits = %llu\n",
            total_css.pending_hits);
    fprintf(fout, "\tL1C_total_cache_reservation_fails = %llu\n",
            total_css.res_fails);
  }

  // L1T
  if (!m_shader_config->m_L1T_config.disabled()) {
    total_css.clear();
    css.clear();
    fprintf(fout, "L1T_cache:\n");
    for (unsigned i = 0; i < m_shader_config->n_simt_clusters; ++i) {
      m_cluster[i]->get_L1T_sub_stats(css);
      total_css += css;
    }
    fprintf(fout, "\tL1T_total_cache_accesses = %llu\n", total_css.accesses);
    fprintf(fout, "\tL1T_total_cache_misses = %llu\n", total_css.misses);
    if (total_css.accesses > 0) {
      fprintf(fout, "\tL1T_total_cache_miss_rate = %.4lf\n",
              (double)total_css.misses / (double)total_css.accesses);
    }
    fprintf(fout, "\tL1T_total_cache_pending_hits = %llu\n",
            total_css.pending_hits);
    fprintf(fout, "\tL1T_total_cache_reservation_fails = %llu\n",
            total_css.res_fails);
  }
}

void trace_gpgpu_sim::shader_print_l1_miss_stat(FILE *fout) const {
  unsigned total_d1_misses = 0, total_d1_accesses = 0;
  for (unsigned i = 0; i < m_shader_config->n_simt_clusters; ++i) {
    unsigned custer_d1_misses = 0, cluster_d1_accesses = 0;
    m_cluster[i]->print_cache_stats(fout, cluster_d1_accesses,
                                    custer_d1_misses);
    total_d1_misses += custer_d1_misses;
    total_d1_accesses += cluster_d1_accesses;
  }
  fprintf(fout, "total_dl1_misses=%d\n", total_d1_misses);
  fprintf(fout, "total_dl1_accesses=%d\n", total_d1_accesses);
  fprintf(fout, "total_dl1_miss_rate= %f\n",
          (float)total_d1_misses / (float)total_d1_accesses);
}

int trace_gpgpu_sim::shader_clock() const { return m_config.core_freq / 1000; }

bool trace_gpgpu_sim::hit_max_cta_count() const {
  if (m_config.gpu_max_cta_opt != 0) {
    if ((gpu_tot_issued_cta + m_total_cta_launched) >= m_config.gpu_max_cta_opt)
      return true;
  }
  return false;
}

void trace_gpgpu_sim::clear_executed_kernel_info() {
  m_executed_kernel_names.clear();
  m_executed_kernel_uids.clear();
}

void trace_gpgpu_sim::stop_all_running_kernels() {
  std::vector<trace_kernel_info_t *>::iterator k;
  for (k = m_running_kernels.begin(); k != m_running_kernels.end(); ++k) {
    if (*k != NULL) {       // If a kernel is active
      set_kernel_done(*k);  // Stop the kernel
      assert(*k == NULL);
    }
  }
}

// Find next clock domain and increment its time
int trace_gpgpu_sim::next_clock_domain(void) {
  double smallest = min3(core_time, icnt_time, dram_time);
  int mask = 0x00;
  if (l2_time <= smallest) {
    smallest = l2_time;
    mask |= L2;
    l2_time += m_config.l2_period;
  }
  if (icnt_time <= smallest) {
    mask |= ICNT;
    icnt_time += m_config.icnt_period;
  }
  if (dram_time <= smallest) {
    mask |= DRAM;
    dram_time += m_config.dram_period;
  }
  if (core_time <= smallest) {
    mask |= CORE;
    core_time += m_config.core_period;
  }
  return mask;
}

void trace_gpgpu_sim::decrement_kernel_latency() {
  for (unsigned n = 0; n < m_running_kernels.size(); n++) {
    if (m_running_kernels[n] && m_running_kernels[n]->m_kernel_TB_latency)
      m_running_kernels[n]->m_kernel_TB_latency--;
  }
}

trace_kernel_info_t *trace_gpgpu_sim::select_kernel() {
  unsigned num_running = 0;
  for (unsigned n = 0; n < m_running_kernels.size(); n++) {
    if (m_running_kernels[n]) {
      num_running++;
    }
  }
  logger->trace("select kernel: {} running kernels, last issued kernel={}",
                num_running, m_last_issued_kernel);
  if (m_running_kernels[m_last_issued_kernel]) {
    trace_kernel_info_t *k = m_running_kernels[m_last_issued_kernel];
    unsigned launch_uid = k->get_uid();
    logger->trace(
        "select kernel: => running_kernels[{}] no more blocks to run={} {}/{} "
        "kernel "
        "block latency={} launch uid={}",
        m_last_issued_kernel, k->no_more_ctas_to_run(), k->get_cta_dim(),
        // k->get_next_cta_id(),
        k->get_grid_dim(),
        // return (m_next_cta.x >= m_grid_dim.x || m_next_cta.y >= m_grid_dim.y
        // ||
        //             m_next_cta.z >= m_grid_dim.z);
        k->m_kernel_TB_latency, launch_uid);
  }
  if (m_running_kernels[m_last_issued_kernel] &&
      !m_running_kernels[m_last_issued_kernel]->no_more_ctas_to_run() &&
      !m_running_kernels[m_last_issued_kernel]->m_kernel_TB_latency) {
    trace_kernel_info_t *k = m_running_kernels[m_last_issued_kernel];
    unsigned launch_uid = k->get_uid();
    // logger->trace(
    //     "select kernel: => running_kernels[{}] no more blocks to run={}
    //     kernel " "block latency={} launch uid={}", m_last_issued_kernel,
    //     k->no_more_ctas_to_run(), k->m_kernel_TB_latency, launch_uid);

    if (std::find(m_executed_kernel_uids.begin(), m_executed_kernel_uids.end(),
                  launch_uid) == m_executed_kernel_uids.end()) {
      m_running_kernels[m_last_issued_kernel]->start_cycle =
          gpu_sim_cycle + gpu_tot_sim_cycle;
      m_executed_kernel_uids.push_back(launch_uid);
      m_executed_kernel_names.push_back(
          m_running_kernels[m_last_issued_kernel]->name());
    }
    return m_running_kernels[m_last_issued_kernel];
  }

  for (unsigned n = 0; n < m_running_kernels.size(); n++) {
    unsigned idx =
        (n + m_last_issued_kernel + 1) % m_config.max_concurrent_kernel;
    if (m_running_kernels[idx]) {
      logger->trace(
          "select kernel: runing_kernels[{}] more blocks left={}, kernel block "
          "latency={}",
          idx, kernel_more_cta_left(m_running_kernels[idx]),
          m_running_kernels[idx]->m_kernel_TB_latency);
    }

    if (kernel_more_cta_left(m_running_kernels[idx]) &&
        !m_running_kernels[idx]->m_kernel_TB_latency) {
      m_last_issued_kernel = idx;
      m_running_kernels[idx]->start_cycle = gpu_sim_cycle + gpu_tot_sim_cycle;
      // record this kernel for stat print if it is the first time this kernel
      // is selected for execution
      unsigned launch_uid = m_running_kernels[idx]->get_uid();
      assert(std::find(m_executed_kernel_uids.begin(),
                       m_executed_kernel_uids.end(),
                       launch_uid) == m_executed_kernel_uids.end());
      m_executed_kernel_uids.push_back(launch_uid);
      m_executed_kernel_names.push_back(m_running_kernels[idx]->name());

      return m_running_kernels[idx];
    }
  }
  return NULL;
}

// void trace_gpgpu_sim::visualizer_printstat() {
//   gzFile visualizer_file = NULL;  // gzFile is basically a pointer to a
//   struct,
//                                   // so it is fine to initialize it as NULL
//   if (!m_config.g_visualizer_enabled) return;
//
//   // clean the content of the visualizer log if it is the first time,
//   otherwise
//   // attach at the end
//   static bool visualizer_first_printstat = true;
//
//   visualizer_file = gzopen(m_config.g_visualizer_filename,
//                            (visualizer_first_printstat) ? "w" : "a");
//   if (visualizer_file == NULL) {
//     printf("error - could not open visualizer trace file.\n");
//     exit(1);
//   }
//   gzsetparams(visualizer_file, m_config.g_visualizer_zlevel,
//               Z_DEFAULT_STRATEGY);
//   visualizer_first_printstat = false;
//
//   cflog_visualizer_gzprint(visualizer_file);
//   shader_CTA_count_visualizer_gzprint(visualizer_file);
//
//   for (unsigned i = 0; i < m_memory_config->m_n_mem; i++)
//     m_memory_partition_unit[i]->visualizer_print(visualizer_file);
//   m_shader_stats->visualizer_print(visualizer_file);
//   m_memory_stats->visualizer_print(visualizer_file);
//   // m_power_stats->visualizer_print(visualizer_file);
//   // proc->visualizer_print(visualizer_file);
//   // other parameters for graphing
//   gzprintf(visualizer_file, "globalcyclecount: %lld\n", gpu_sim_cycle);
//   gzprintf(visualizer_file, "globalinsncount: %lld\n", gpu_sim_insn);
//   gzprintf(visualizer_file, "globaltotinsncount: %lld\n", gpu_tot_sim_insn);
//
//   time_vector_print_interval2gzfile(visualizer_file);
//
//   gzclose(visualizer_file);
// }

/// printing the names and uids of a set of executed kernels (usually there is
/// only one)
std::string trace_gpgpu_sim::executed_kernel_info_string() {
  std::stringstream statout;

  statout << "kernel_name = ";
  for (unsigned int k = 0; k < m_executed_kernel_names.size(); k++) {
    statout << m_executed_kernel_names[k] << " ";
  }
  statout << std::endl;
  statout << "kernel_launch_uid = ";
  for (unsigned int k = 0; k < m_executed_kernel_uids.size(); k++) {
    statout << m_executed_kernel_uids[k] << " ";
  }
  statout << std::endl;

  return statout.str();
}

void trace_gpgpu_sim::set_kernel_done(trace_kernel_info_t *kernel) {
  unsigned uid = kernel->get_uid();
  logger->info("kernel {} ({}) completed", kernel->name(), uid);

  m_finished_kernel.push_back(uid);
  std::vector<trace_kernel_info_t *>::iterator k;
  for (k = m_running_kernels.begin(); k != m_running_kernels.end(); k++) {
    if (*k == kernel) {
      kernel->end_cycle = gpu_sim_cycle + gpu_tot_sim_cycle;
      *k = NULL;
      break;
    }
  }
  assert(k != m_running_kernels.end());
}

bool trace_gpgpu_sim::kernel_more_cta_left(trace_kernel_info_t *kernel) const {
  if (hit_max_cta_count()) return false;

  // ROMAN: initialization
  if (kernel == NULL) return false;
  if (!kernel->no_more_ctas_to_run()) return true;
  // if (kernel && !kernel->no_more_ctas_to_run()) return true;

  return false;
}
