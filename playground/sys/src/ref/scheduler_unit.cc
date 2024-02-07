#include "scheduler_unit.hpp"

#include <memory>
#include <iostream>

#include "barrier_set.hpp"
#include "exec_unit_type.hpp"
#include "ifetch_buffer.hpp"
#include "mem_fetch_interface.hpp"
#include "opndcoll_rfu.hpp"
#include "shader_core_mem_fetch_allocator.hpp"
#include "shader_core_stats.hpp"
#include "shader_trace.hpp"
#include "trace_shader_core_ctx.hpp"
#include "trace_shd_warp.hpp"
#include "trace_warp_inst.hpp"

trace_shd_warp_t &scheduler_unit::warp(int i) { return *((*m_warp)[i]); }

void scheduler_unit::cycle() {
  logger->debug("{}[{}][core {}]::cycle()", name(), m_id, m_shader->m_sid);
  bool valid_inst =
      false;  // there was one warp with a valid instruction to
              // issue (didn't require flush due to control hazard)
  bool ready_inst = false;   // of the valid instructions, there was one not
                             // waiting for pending register writes
  bool issued_inst = false;  // of these we issued one

  bool should_log = logger->should_log(spdlog::level::debug);

  // if (logger->should_log(spdlog::level::debug)) {
  //   std::vector<unsigned> tmp_warp_ids;
  //   std::vector<trace_shd_warp_t *>::const_iterator iter;
  //
  //   for (iter = m_next_cycle_prioritized_warps.begin();
  //        iter != m_next_cycle_prioritized_warps.end(); iter++) {
  //     tmp_warp_ids.push_back((*iter)->get_warp_id());
  //   }
  //   logger->debug("{}::scheduler_unit[{}] BEFORE: prioritized warp ids:
  //   [{}]",
  //                 name(), m_id, fmt::join(tmp_warp_ids, ", "));
  //
  //   tmp_warp_ids.clear();
  //   for (iter = m_next_cycle_prioritized_warps.begin();
  //        iter != m_next_cycle_prioritized_warps.end(); iter++) {
  //     tmp_warp_ids.push_back((*iter)->get_dynamic_warp_id());
  //   }
  //   logger->debug(
  //       "{}::scheduler_unit[{}] BEFORE: prioritized dynamic warp ids: [{}]",
  //       name(), m_id, fmt::join(tmp_warp_ids, ", "));
  // }

  if (should_log) {
    // logger->debug(
    // fmt::println(
    //     "{}[{}, core {}]: BEFORE: prioritized warp ids: [{}]", name(), m_id,
    //     m_shader->m_sid,
    //     fmt::join(m_next_cycle_prioritized_warps_lockstep_compat_warp_ids,
    //               ", "));
    logger->debug(
        // fmt::println(
        "{}[{}, core {}]: BEFORE: prioritized dynamic warp ids: [{}]", name(),
        m_id, m_shader->m_sid,

        fmt::join(
            m_next_cycle_prioritized_warps_lockstep_compat_dynamic_warp_ids,
            ", "));
  }

  // TODO REMOVE
  // fmt::println(
  //     "{}[{}, core {}]: PLAY BEFORE: prioritized warp ids: [{}]", name(),
  //     m_id, m_shader->m_sid,
  //     fmt::join(m_next_cycle_prioritized_warps_lockstep_compat_warp_ids, ",
  //     "));

  if (should_log) {
    // fmt::println(
    logger->debug("{}[{}, core {}]: last issued from {}", name(), m_id,
                  m_shader->m_sid,
                  (*m_last_supervised_issued)->get_dynamic_warp_id());
  }
  order_warps();

  // if (logger->should_log(spdlog::level::debug)) {
  // std::vector<unsigned> tmp_warp_ids;
  std::vector<trace_shd_warp_t *>::const_iterator iter;

  m_next_cycle_prioritized_warps_lockstep_compat_warp_ids.clear();
  for (iter = m_next_cycle_prioritized_warps.begin();
       iter != m_next_cycle_prioritized_warps.end(); iter++) {
    m_next_cycle_prioritized_warps_lockstep_compat_warp_ids.push_back(
        (*iter)->get_warp_id());
  }
  if (should_log) {
    // logger->debug(
    // fmt::println(
    //     "{}[{}, core {}]: AFTER: prioritized warp ids: [{}]", name(), m_id,
    //     m_shader->m_sid,
    //     fmt::join(m_next_cycle_prioritized_warps_lockstep_compat_warp_ids,
    //               ", "));
  }

  m_next_cycle_prioritized_warps_lockstep_compat_dynamic_warp_ids.clear();
  for (iter = m_next_cycle_prioritized_warps.begin();
       iter != m_next_cycle_prioritized_warps.end(); iter++) {
    m_next_cycle_prioritized_warps_lockstep_compat_dynamic_warp_ids.push_back(
        (*iter)->get_dynamic_warp_id());
  }
  if (should_log) {
    logger->debug(
        // fmt::println(
        "{}[{}, core {}]: AFTER: prioritized dynamic warp ids: [{}]", name(),
        m_id, m_shader->m_sid,
        fmt::join(
            m_next_cycle_prioritized_warps_lockstep_compat_dynamic_warp_ids,
            ", "));
  }

  // TODO REMOVE
  // fmt::println(
  //     "{}[{}, core {}]: PLAY AFTER: prioritized warp ids: [{}]", name(),
  //     m_id, m_shader->m_sid,
  //     fmt::join(m_next_cycle_prioritized_warps_lockstep_compat_warp_ids, ",
  //     "));

  // this is for compatibility with lockstep testing
  // m_next_cycle_prioritized_warps_lockstep_compat.clear();
  // copy(m_next_cycle_prioritized_warps.begin(),
  //      m_next_cycle_prioritized_warps.end(),
  //      back_inserter(m_next_cycle_prioritized_warps_lockstep_compat));

  for (std::vector<trace_shd_warp_t *>::iterator iter =
           m_next_cycle_prioritized_warps.begin();
       iter != m_next_cycle_prioritized_warps.end(); iter++) {
    // Don't consider warps that are not yet valid
    trace_shd_warp_t *next_warp = *iter;
    if (next_warp == NULL || next_warp->done_exit()) {
      continue;
    }
    assert(next_warp->instruction_count() > 0);

    if (should_log && !next_warp->trace_done() &&
        next_warp->instruction_count() > 1) {
      logger->debug(
          "core[{}][{}] scheduler[{}]: \n\t => testing (warp_id={}, "
          "dynamic_warp_id={}, trace_pc = {}, pc={}, ibuffer=[{}, {}], {} "
          "instructions)",
          m_shader->get_tpc(), m_shader->get_sid(), m_id,
          next_warp->get_warp_id(), next_warp->get_dynamic_warp_id(),
          next_warp->trace_pc, next_warp->get_pc(),
          next_warp->m_ibuffer[0].m_valid ? next_warp->m_ibuffer[0].m_inst->pc
                                          : 0,
          next_warp->m_ibuffer[1].m_valid ? next_warp->m_ibuffer[1].m_inst->pc
                                          : 0,
          next_warp->instruction_count());
    }
    SCHED_DPRINTF("Testing (warp_id %u, dynamic_warp_id %u)\n",
                  next_warp->get_warp_id(), next_warp->get_dynamic_warp_id());
    unsigned warp_id = next_warp->get_warp_id();
    unsigned checked = 0;
    unsigned issued = 0;
    exec_unit_type_t previous_issued_inst_exec_type = exec_unit_type_t::NONE;
    unsigned max_issue = m_shader->m_config->gpgpu_max_insn_issue_per_warp;
    bool diff_exec_units =
        m_shader->m_config
            ->gpgpu_dual_issue_diff_exec_units;  // In this mode, we only allow
                                                 // dual issue to diff execution
                                                 // units (as in Maxwell and
                                                 // Pascal)

    assert(next_warp->instruction_count() > 0);

    if (next_warp->instruction_count() > 1) {
      if (warp(warp_id).ibuffer_empty()) {
        logger->debug(
            "\t => Warp (warp_id {}, dynamic_warp_id {}) fails as "
            "ibuffer_empty",
            next_warp->get_warp_id(), next_warp->get_dynamic_warp_id());
      }

      if (warp(warp_id).waiting()) {
        logger->debug(
            "\t => Warp (warp_id {}, dynamic_warp_id {}) fails as waiting for "
            "completion",
            next_warp->get_warp_id(), next_warp->get_dynamic_warp_id());
      }
    }

    if (warp(warp_id).ibuffer_empty())
      SCHED_DPRINTF(
          "Warp (warp_id %u, dynamic_warp_id %u) fails as ibuffer_empty\n",
          next_warp->get_warp_id(), next_warp->get_dynamic_warp_id());

    if (warp(warp_id).waiting())
      SCHED_DPRINTF(
          "Warp (warp_id %u, dynamic_warp_id %u) fails as waiting for "
          "barrier\n",
          next_warp->get_warp_id(), next_warp->get_dynamic_warp_id());

    while (!warp(warp_id).waiting() && !warp(warp_id).ibuffer_empty() &&
           (checked < max_issue) && (checked <= issued) &&
           (issued < max_issue)) {
      const warp_inst_t *pI = warp(warp_id).ibuffer_next_inst();

      // Jin: handle cdp latency;
      if (pI && pI->m_is_cdp && warp(warp_id).m_cdp_latency > 0) {
        assert(warp(warp_id).m_cdp_dummy);
        warp(warp_id).m_cdp_latency--;
        break;
      }

      bool valid = warp(warp_id).ibuffer_next_valid();
      bool warp_inst_issued = false;
      unsigned pc = (unsigned)-1;
      unsigned rpc = (unsigned)-1;
      if (pI) m_shader->get_pdom_stack_top_info(warp_id, pI, &pc, &rpc);

      if (pI) {
        if (should_log) {
          logger->debug(
              "Warp (warp_id {}, dynamic_warp_id {}) instruction buffer[{}] "
              "has "
              "valid instruction ({}, op={})",
              next_warp->get_warp_id(), next_warp->get_dynamic_warp_id(),
              warp(warp_id).m_next, pI->display(), uarch_op_t_str[pI->op]);
        }
        assert(valid);
        assert(pI->pc == pc &&
               pc == rpc);  // trace driven mode has no control hazards
        if (pc != pI->pc) {
          SCHED_DPRINTF(
              "Warp (warp_id %u, dynamic_warp_id %u) control hazard "
              "instruction flush\n",
              next_warp->get_warp_id(), next_warp->get_dynamic_warp_id());
          // control hazard
          warp(warp_id).set_next_pc(pc);
          warp(warp_id).ibuffer_flush();
        } else {
          valid_inst = true;
          if (!m_scoreboard->checkCollision(warp_id, pI)) {
            logger->debug(
                "Warp (warp_id {}, dynamic_warp_id {}) passes scoreboard",
                next_warp->get_warp_id(), next_warp->get_dynamic_warp_id());
            ready_inst = true;

            const active_mask_t &active_mask =
                m_shader->get_active_mask(warp_id, pI);

            assert(warp(warp_id).inst_in_pipeline());

            if ((pI->op == LOAD_OP) || (pI->op == STORE_OP) ||
                (pI->op == MEMORY_BARRIER_OP) ||
                (pI->op == TENSOR_CORE_LOAD_OP) ||
                (pI->op == TENSOR_CORE_STORE_OP)) {
              if (m_mem_out->has_free(m_shader->m_config->sub_core_model,
                                      m_id) &&
                  (!diff_exec_units ||
                   previous_issued_inst_exec_type != exec_unit_type_t::MEM)) {
                m_shader->issue_warp(*m_mem_out, pI, active_mask, warp_id,
                                     m_id);
                issued++;
                issued_inst = true;
                warp_inst_issued = true;
                previous_issued_inst_exec_type = exec_unit_type_t::MEM;
              } else {
                logger->debug("issue failed: no free mem port register");
              }
            } else {
              // This code need to be refactored
              if (pI->op != TENSOR_CORE_OP && pI->op != SFU_OP &&
                  pI->op != DP_OP && !(pI->op >= SPEC_UNIT_START_ID)) {
                bool execute_on_SP = false;
                bool execute_on_INT = false;

                bool sp_pipe_avail =
                    (m_shader->m_config->gpgpu_num_sp_units > 0) &&
                    m_sp_out->has_free(m_shader->m_config->sub_core_model,
                                       m_id);
                bool int_pipe_avail =
                    (m_shader->m_config->gpgpu_num_int_units > 0) &&
                    m_int_out->has_free(m_shader->m_config->sub_core_model,
                                        m_id);
                logger->debug(
                    "sp pipe avail ={}({} units) int pipe avail ={}({} units)",
                    sp_pipe_avail, m_shader->m_config->gpgpu_num_sp_units,
                    int_pipe_avail, m_shader->m_config->gpgpu_num_int_units);

                // if INT unit pipline exist, then execute ALU and INT
                // operations on INT unit and SP-FPU on SP unit (like in Volta)
                // if INT unit pipline does not exist, then execute all ALU, INT
                // and SP operations on SP unit (as in Fermi, Pascal GPUs)
                if (m_shader->m_config->gpgpu_num_int_units > 0 &&
                    int_pipe_avail && pI->op != SP_OP &&
                    !(diff_exec_units &&
                      previous_issued_inst_exec_type == exec_unit_type_t::INT))
                  execute_on_INT = true;
                else if (sp_pipe_avail &&
                         (m_shader->m_config->gpgpu_num_int_units == 0 ||
                          (m_shader->m_config->gpgpu_num_int_units > 0 &&
                           pI->op == SP_OP)) &&
                         !(diff_exec_units && previous_issued_inst_exec_type ==
                                                  exec_unit_type_t::SP))
                  execute_on_SP = true;

                logger->debug("execute on INT={} execute on SP={}",
                              execute_on_INT, execute_on_SP);
                if (execute_on_INT || execute_on_SP) {
                  // Jin: special for CDP api
                  if (pI->m_is_cdp && !warp(warp_id).m_cdp_dummy) {
                    assert(warp(warp_id).m_cdp_latency == 0);

                    if (pI->m_is_cdp == 1) {
                      warp(warp_id).m_cdp_latency =
                          m_shader->m_config->gpgpu_ctx->func_sim
                              ->cdp_latency[pI->m_is_cdp - 1];
                    } else {
                      // cudaLaunchDeviceV2 and cudaGetParameterBufferV2
                      warp(warp_id).m_cdp_latency =
                          m_shader->m_config->gpgpu_ctx->func_sim
                              ->cdp_latency[pI->m_is_cdp - 1] +
                          m_shader->m_config->gpgpu_ctx->func_sim
                                  ->cdp_latency[pI->m_is_cdp] *
                              active_mask.count();
                    }
                    warp(warp_id).m_cdp_dummy = true;
                    break;
                  } else if (pI->m_is_cdp && warp(warp_id).m_cdp_dummy) {
                    assert(warp(warp_id).m_cdp_latency == 0);
                    warp(warp_id).m_cdp_dummy = false;
                  }
                }

                if (execute_on_SP) {
                  m_shader->issue_warp(*m_sp_out, pI, active_mask, warp_id,
                                       m_id);
                  issued++;
                  issued_inst = true;
                  warp_inst_issued = true;
                  previous_issued_inst_exec_type = exec_unit_type_t::SP;
                } else if (execute_on_INT) {
                  m_shader->issue_warp(*m_int_out, pI, active_mask, warp_id,
                                       m_id);
                  issued++;
                  issued_inst = true;
                  warp_inst_issued = true;
                  previous_issued_inst_exec_type = exec_unit_type_t::INT;
                }
              } else if ((m_shader->m_config->gpgpu_num_dp_units > 0) &&
                         (pI->op == DP_OP) &&
                         !(diff_exec_units && previous_issued_inst_exec_type ==
                                                  exec_unit_type_t::DP)) {
                throw std::runtime_error("case 2");
                bool dp_pipe_avail =
                    (m_shader->m_config->gpgpu_num_dp_units > 0) &&
                    m_dp_out->has_free(m_shader->m_config->sub_core_model,
                                       m_id);

                if (dp_pipe_avail) {
                  m_shader->issue_warp(*m_dp_out, pI, active_mask, warp_id,
                                       m_id);
                  issued++;
                  issued_inst = true;
                  warp_inst_issued = true;
                  previous_issued_inst_exec_type = exec_unit_type_t::DP;
                }
              }  // If the DP units = 0 (like in Fermi archi), then execute DP
                 // inst on SFU unit
              else if (((m_shader->m_config->gpgpu_num_dp_units == 0 &&
                         pI->op == DP_OP) ||
                        (pI->op == SFU_OP) || (pI->op == ALU_SFU_OP)) &&
                       !(diff_exec_units && previous_issued_inst_exec_type ==
                                                exec_unit_type_t::SFU)) {
                // throw std::runtime_error("case 3");
                bool sfu_pipe_avail =
                    (m_shader->m_config->gpgpu_num_sfu_units > 0) &&
                    m_sfu_out->has_free(m_shader->m_config->sub_core_model,
                                        m_id);

                if (sfu_pipe_avail) {
                  m_shader->issue_warp(*m_sfu_out, pI, active_mask, warp_id,
                                       m_id);
                  issued++;
                  issued_inst = true;
                  warp_inst_issued = true;
                  previous_issued_inst_exec_type = exec_unit_type_t::SFU;
                }
              } else if ((pI->op == TENSOR_CORE_OP) &&
                         !(diff_exec_units && previous_issued_inst_exec_type ==
                                                  exec_unit_type_t::TENSOR)) {
                throw std::runtime_error("case 4");
                bool tensor_core_pipe_avail =
                    (m_shader->m_config->gpgpu_num_tensor_core_units > 0) &&
                    m_tensor_core_out->has_free(
                        m_shader->m_config->sub_core_model, m_id);

                if (tensor_core_pipe_avail) {
                  m_shader->issue_warp(*m_tensor_core_out, pI, active_mask,
                                       warp_id, m_id);
                  issued++;
                  issued_inst = true;
                  warp_inst_issued = true;
                  previous_issued_inst_exec_type = exec_unit_type_t::TENSOR;
                }
              } else if ((pI->op >= SPEC_UNIT_START_ID) &&
                         !(diff_exec_units &&
                           previous_issued_inst_exec_type ==
                               exec_unit_type_t::SPECIALIZED)) {
                throw std::runtime_error("case 5");
                unsigned spec_id = pI->op - SPEC_UNIT_START_ID;
                assert(spec_id < m_shader->m_config->m_specialized_unit.size());
                register_set *spec_reg_set = m_spec_cores_out[spec_id];
                bool spec_pipe_avail =
                    (m_shader->m_config->m_specialized_unit[spec_id].num_units >
                     0) &&
                    spec_reg_set->has_free(m_shader->m_config->sub_core_model,
                                           m_id);

                if (spec_pipe_avail) {
                  m_shader->issue_warp(*spec_reg_set, pI, active_mask, warp_id,
                                       m_id);
                  issued++;
                  issued_inst = true;
                  warp_inst_issued = true;
                  previous_issued_inst_exec_type =
                      exec_unit_type_t::SPECIALIZED;
                }
              }

            }  // end of else
          } else {
            SCHED_DPRINTF(
                "Warp (warp_id %u, dynamic_warp_id %u) fails scoreboard\n",
                next_warp->get_warp_id(), next_warp->get_dynamic_warp_id());
          }
        }
      } else if (valid) {
        // this case can happen after a return instruction in diverged warp
        SCHED_DPRINTF(
            "Warp (warp_id %u, dynamic_warp_id %u) return from diverged warp "
            "flush\n",
            next_warp->get_warp_id(), next_warp->get_dynamic_warp_id());
        assert(pc != (unsigned)-1);
        warp(warp_id).set_next_pc(pc);
        warp(warp_id).ibuffer_flush();
      }
      if (warp_inst_issued) {
        logger->debug(
            "Warp (warp_id {}, dynamic_warp_id {}) issued {} instructions",
            next_warp->get_warp_id(), next_warp->get_dynamic_warp_id(), issued);
        do_on_warp_issued(warp_id, issued, iter);
      }
      checked++;
    }
    if (issued) {
      // throw std::runtime_error("issued instruction");
      // This might be a bit inefficient, but we need to maintain
      // two ordered list for proper scheduler execution.
      // We could remove the need for this loop by associating a
      // supervised_is index with each entry in the
      // m_next_cycle_prioritized_warps vector. For now, just run through until
      // you find the right warp_id
      for (std::vector<trace_shd_warp_t *>::const_iterator supervised_iter =
               m_supervised_warps.begin();
           supervised_iter != m_supervised_warps.end(); ++supervised_iter) {
        if (next_warp == *supervised_iter) {
          m_last_supervised_issued = supervised_iter;
        }
      }
      m_num_issued_last_cycle = issued;
      if (issued == 1)
        m_stats->single_issue_nums[m_id]++;
      else if (issued > 1)
        m_stats->dual_issue_nums[m_id]++;
      else
        abort();  // issued should be > 0

      break;
    }
  }

  // issue stall statistics:
  if (!valid_inst)
    m_stats->shader_cycle_distro[0]++;  // idle or control hazard
  else if (!ready_inst)
    m_stats->shader_cycle_distro[1]++;  // waiting for RAW hazards (possibly due
                                        // to memory)
  else if (!issued_inst)
    m_stats->shader_cycle_distro[2]++;  // pipeline stalled
}

void scheduler_unit::do_on_warp_issued(
    unsigned warp_id, unsigned num_issued,
    const std::vector<trace_shd_warp_t *>::const_iterator &prioritized_iter) {
  m_stats->event_warp_issued(m_shader->get_sid(), warp_id, num_issued,
                             warp(warp_id).get_dynamic_warp_id());
  warp(warp_id).ibuffer_step();
}

bool scheduler_unit::sort_warps_by_oldest_dynamic_id(trace_shd_warp_t *lhs,
                                                     trace_shd_warp_t *rhs) {
  if (rhs && lhs) {
    if (lhs->done_exit() || lhs->waiting()) {
      return false;
    } else if (rhs->done_exit() || rhs->waiting()) {
      return true;
    } else {
      return lhs->get_dynamic_warp_id() < rhs->get_dynamic_warp_id();
    }
  } else {
    return lhs < rhs;
  }
}

std::unique_ptr<scheduler_unit> new_scheduler_unit() {
  throw std::runtime_error("todo");
  // return std::make_unique<scheduler_unit>();
}
