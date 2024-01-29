#pragma once

#include "spdlog/logger.h"
#include <set>
#include <string>
#include <memory>
#include <vector>

class inst_t;
class warp_inst_t;

class Scoreboard {
 public:
  Scoreboard(unsigned sid, unsigned n_warps, class trace_gpgpu_sim *gpu);

  void reserveRegisters(const warp_inst_t *inst);
  void releaseRegisters(const warp_inst_t *inst);
  void releaseRegister(unsigned wid, unsigned regnum);

  bool checkCollision(unsigned wid, const inst_t *inst) const;
  bool has_pending_writes(unsigned wid) const;
  unsigned num_pending_writes(unsigned wid) const;
  const std::set<unsigned int> &get_pending_writes(unsigned wid) const;
  void printContents() const;
  bool islongop(unsigned warp_id, unsigned regnum) const;

  std::shared_ptr<spdlog::logger> logger;

 private:
  void reserveRegister(unsigned wid, unsigned regnum);
  int get_sid() const { return m_sid; }

  unsigned m_sid;

  // keeps track of pending writes to registers
  // indexed by warp id, reg_id => pending write count
  std::vector<std::set<unsigned>> reg_table;
  // Register that depend on a long operation (global, local or tex memory)
  std::vector<std::set<unsigned>> longopregs;

  class trace_gpgpu_sim *m_gpu;
};
