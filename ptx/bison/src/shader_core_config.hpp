#pragma once

#include <cassert>
#include <cstddef>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <sstream>
#include <vector>

#include "cache_config.hpp"
#include "core_config.hpp"
#include "hal.hpp"
#include "pipeline_stage_name.hpp"

class gpgpu_context;
class kernel_info_t;

// struct shader_core_config_params {
//   unsigned n_thread_per_shader;
//   unsigned warp_size;
//   unsigned max_cta_per_core;
//   unsigned n_simt_cores_per_cluster;
//   unsigned n_simt_clusters;
//   unsigned gpgpu_shader_registers;
// };

class shader_core_config : public core_config {
public:
  // shader_core_config(gpgpu_context *ctx, shader_core_config_params config)
  shader_core_config(gpgpu_context *ctx) : core_config(ctx) {
    // pipeline_widths_string = NULL;
    gpgpu_ctx = ctx;
    n_thread_per_shader = 0;
    warp_size = 32;
    max_cta_per_core = 0;
    n_simt_cores_per_cluster = 0;
    n_simt_clusters = 0;
    gpgpu_shader_registers = 0;

    // if (n_thread_per_shader > MAX_THREAD_PER_SM) {
    //   printf("GPGPU-Sim uArch: Error ** increase MAX_THREAD_PER_SM in "
    //          "abstract_hardware_model.h from %u to %u\n",
    //          MAX_THREAD_PER_SM, n_thread_per_shader);
    //   abort();
    // }
    // max_warps_per_shader = n_thread_per_shader / warp_size;
    // assert(!(n_thread_per_shader % warp_size));
  }

  // void init() {
  // int ntok = sscanf(gpgpu_shader_core_pipeline_opt, "%d:%d",
  //                   &n_thread_per_shader, &warp_size);
  // if (ntok != 2) {
  //   printf("GPGPU-Sim uArch: error while parsing configuration string "
  //          "gpgpu_shader_core_pipeline_opt\n");
  //   abort();
  // }
  //
  // char *toks = new char[100];
  // char *tokd = toks;
  // strcpy(toks, pipeline_widths_string);
  //
  // toks = strtok(toks, ",");

  /*	Removing the tensorcore pipeline while reading the config files if the
     tensor core is not available. If we won't remove it, old regression will
     be broken. So to support the legacy config files it's best to handle in
     this way.
   */
  // int num_config_to_read = N_PIPELINE_STAGES - 2 *
  // (!gpgpu_tensor_core_avail);
  //
  // for (int i = 0; i < num_config_to_read; i++) {
  //   assert(toks);
  //   ntok = sscanf(toks, "%d", &pipe_widths[i]);
  //   assert(ntok == 1);
  //   toks = strtok(NULL, ",");
  // }
  //
  // delete[] tokd;
  //
  //
  // set_pipeline_latency();

  // ROMAN
  // m_L1I_config.init(m_L1I_config.m_config_string, FuncCachePreferNone);
  // m_L1T_config.init(m_L1T_config.m_config_string, FuncCachePreferNone);
  // m_L1C_config.init(m_L1C_config.m_config_string, FuncCachePreferNone);
  // m_L1D_config.init(m_L1D_config.m_config_string, FuncCachePreferNone);
  // gpgpu_cache_texl1_linesize = m_L1T_config.get_line_sz();
  // gpgpu_cache_constl1_linesize = m_L1C_config.get_line_sz();
  // m_valid = true;

  // m_specialized_unit_num = 0;
  // parse the specialized units
  // for (unsigned i = 0; i < SPECIALIZED_UNIT_NUM; ++i) {
  //   unsigned enabled;
  //   // specialized_unit_params sparam;
  //   // sscanf(specialized_unit_string[i], "%u,%u,%u,%u,%u,%s", &enabled,
  //   //        &sparam.num_units, &sparam.latency,
  //   //        &sparam.id_oc_spec_reg_width, &sparam.oc_ex_spec_reg_width,
  //   //        sparam.name);
  //
  //   if (enabled) {
  //     // ROMAN
  //     // m_specialized_unit.push_back(sparam);
  //     // strncpy(m_specialized_unit.back().name, sparam.name,
  //     //         sizeof(m_specialized_unit.back().name));
  //     // m_specialized_unit_num += sparam.num_units;
  //   } else {
  //     // we only accept continuous specialized_units, i.e., 1,2,3,4
  //     break;
  //   }
  // }

  // parse gpgpu_shmem_option for adpative cache config
  // if (adaptive_cache_config) {
  //   std::stringstream ss(gpgpu_shmem_option);
  //   while (ss.good()) {
  //     std::string option;
  //     std::getline(ss, option, ',');
  //     shmem_opt_list.push_back((unsigned)std::stoi(option) * 1024);
  //   }
  //   std::sort(shmem_opt_list.begin(), shmem_opt_list.end());
  // }
  // }
  // void reg_options(class OptionParser *opp);
  // unsigned max_cta(const kernel_info_t &k) const;
  unsigned num_shader() const {
    return n_simt_clusters * n_simt_cores_per_cluster;
  }
  // unsigned sid_to_cluster(unsigned sid) const {
  //   return sid / n_simt_cores_per_cluster;
  // }
  // unsigned sid_to_cid(unsigned sid) const {
  //   return sid % n_simt_cores_per_cluster;
  // }
  // unsigned cid_to_sid(unsigned cid, unsigned cluster_id) const {
  //   return cluster_id * n_simt_cores_per_cluster + cid;
  // }
  // void set_pipeline_latency();

  // backward pointer
  class gpgpu_context *gpgpu_ctx;

  // // data
  // char *gpgpu_shader_core_pipeline_opt;
  // bool gpgpu_perfect_mem;
  // bool gpgpu_clock_gated_reg_file;
  // bool gpgpu_clock_gated_lanes;
  // enum divergence_support_t model;
  unsigned n_thread_per_shader;
  // unsigned n_regfile_gating_group;
  // unsigned max_warps_per_shader;
  // // Limit on number of concurrent CTAs in shader core
  unsigned max_cta_per_core;
  // unsigned max_barriers_per_cta;
  // char *gpgpu_scheduler_string;
  // unsigned gpgpu_shmem_per_block;
  // unsigned gpgpu_registers_per_block;
  // char *pipeline_widths_string;
  // int pipe_widths[N_PIPELINE_STAGES];
  //
  mutable cache_config m_L1I_config;
  mutable cache_config m_L1T_config;
  mutable cache_config m_L1C_config;
  mutable l1d_cache_config m_L1D_config;
  //
  // bool gpgpu_dwf_reg_bankconflict;
  //
  // unsigned gpgpu_num_sched_per_core;
  // int gpgpu_max_insn_issue_per_warp;
  // bool gpgpu_dual_issue_diff_exec_units;
  //
  // // op collector
  // bool enable_specialized_operand_collector;
  // int gpgpu_operand_collector_num_units_sp;
  // int gpgpu_operand_collector_num_units_dp;
  // int gpgpu_operand_collector_num_units_sfu;
  // int gpgpu_operand_collector_num_units_tensor_core;
  // int gpgpu_operand_collector_num_units_mem;
  // int gpgpu_operand_collector_num_units_gen;
  // int gpgpu_operand_collector_num_units_int;
  //
  // unsigned int gpgpu_operand_collector_num_in_ports_sp;
  // unsigned int gpgpu_operand_collector_num_in_ports_dp;
  // unsigned int gpgpu_operand_collector_num_in_ports_sfu;
  // unsigned int gpgpu_operand_collector_num_in_ports_tensor_core;
  // unsigned int gpgpu_operand_collector_num_in_ports_mem;
  // unsigned int gpgpu_operand_collector_num_in_ports_gen;
  // unsigned int gpgpu_operand_collector_num_in_ports_int;
  //
  // unsigned int gpgpu_operand_collector_num_out_ports_sp;
  // unsigned int gpgpu_operand_collector_num_out_ports_dp;
  // unsigned int gpgpu_operand_collector_num_out_ports_sfu;
  // unsigned int gpgpu_operand_collector_num_out_ports_tensor_core;
  // unsigned int gpgpu_operand_collector_num_out_ports_mem;
  // unsigned int gpgpu_operand_collector_num_out_ports_gen;
  // unsigned int gpgpu_operand_collector_num_out_ports_int;
  //
  // unsigned int gpgpu_num_sp_units;
  // unsigned int gpgpu_tensor_core_avail;
  // unsigned int gpgpu_num_dp_units;
  // unsigned int gpgpu_num_sfu_units;
  // unsigned int gpgpu_num_tensor_core_units;
  // unsigned int gpgpu_num_mem_units;
  // unsigned int gpgpu_num_int_units;
  //
  // // Shader core resources
  unsigned gpgpu_shader_registers;
  // int gpgpu_warpdistro_shader;
  // int gpgpu_warp_issue_shader;
  // unsigned gpgpu_num_reg_banks;
  // bool gpgpu_reg_bank_use_warp_id;
  // bool gpgpu_local_mem_map;
  // bool gpgpu_ignore_resources_limitation;
  // bool sub_core_model;
  //
  // unsigned max_sp_latency;
  // unsigned max_int_latency;
  // unsigned max_sfu_latency;
  // unsigned max_dp_latency;
  // unsigned max_tensor_core_latency;
  //

  //
  unsigned n_simt_cores_per_cluster;
  unsigned n_simt_clusters;
  // unsigned n_simt_ejection_buffer_size;
  // unsigned ldst_unit_response_queue_size;
  //
  // int simt_core_sim_order;
  //
  // unsigned smem_latency;
  //
  // unsigned mem2device(unsigned memid) const { return memid + n_simt_clusters;
  // }
  //
  // // Jin: concurrent kernel on sm
  // bool gpgpu_concurrent_kernel_sm;
  //
  // bool perfect_inst_const_cache;
  // unsigned inst_fetch_throughput;
  // unsigned reg_file_port_throughput;

  // specialized unit config strings
  // ROMAN
  // char *specialized_unit_string[SPECIALIZED_UNIT_NUM];
  // mutable std::vector<specialized_unit_params> m_specialized_unit;
  // unsigned m_specialized_unit_num;
};
