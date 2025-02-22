#pragma once

#include <vector>

#include "trace_warp_inst.hpp"

// register that can hold multiple instructions.
class register_set {
 public:
  register_set(unsigned num, const char *name, unsigned id,
               std::shared_ptr<spdlog::logger> logger)
      : logger(logger) {
    for (unsigned i = 0; i < num; i++) {
      regs.push_back(new warp_inst_t());
    }
    m_name = name;
    m_id = id;
  }

  const char *get_name() const { return m_name; }
  unsigned id() const { return m_id; }

  bool has_free() const {
    for (unsigned i = 0; i < regs.size(); i++) {
      if (regs[i]->empty()) {
        return true;
      }
    }
    return false;
  }

  bool has_free(bool sub_core_model, unsigned reg_id) const {
    // in subcore model, each sched has a one specific reg to use (based on
    // sched id)
    if (!sub_core_model) return has_free();

    assert(reg_id < regs.size());
    return regs[reg_id]->empty();
  }

  bool has_ready() const {
    for (unsigned i = 0; i < regs.size(); i++) {
      if (not regs[i]->empty()) {
        return true;
      }
    }
    return false;
  }

  bool has_ready(bool sub_core_model, unsigned reg_id) const {
    if (!sub_core_model) return has_ready();
    assert(reg_id < regs.size());
    return (not regs[reg_id]->empty());
  }

  unsigned get_ready_reg_id() {
    // for sub core model we need to figure which reg_id has the ready warp
    // this function should only be called if has_ready() was true
    assert(has_ready());
    warp_inst_t **ready;
    ready = NULL;
    unsigned reg_id = (unsigned)-1;
    for (unsigned i = 0; i < regs.size(); i++) {
      if (not regs[i]->empty()) {
        if (ready and (*ready)->get_uid() < regs[i]->get_uid()) {
          // ready is oldest
        } else {
          ready = &regs[i];
          reg_id = i;
        }
      }
    }
    return reg_id;
  }

  unsigned get_schd_id(unsigned reg_id) const {
    assert(not regs[reg_id]->empty());
    return regs[reg_id]->get_schd_id();
  }

  void move_in(warp_inst_t *&src) {  // , std::string msg) {
    warp_inst_t **free = get_free();
    move_warp(*free, src);  // , msg, logger);
  }

  void move_in(bool sub_core_model, unsigned reg_id,
               warp_inst_t *&src) {  //, std::string msg) {
    warp_inst_t **free;
    if (!sub_core_model) {
      free = get_free();
    } else {
      assert(reg_id < regs.size());
      free = get_free(sub_core_model, reg_id);
    }
    move_warp(*free, src);  // , msg, logger);
  }

  void move_out_to(warp_inst_t *&dest) {  // , std::string msg) {
    warp_inst_t **ready = get_ready();
    assert(ready != NULL);
    move_warp(dest, *ready);  // , msg, logger);
  }

  void move_out_to(bool sub_core_model, unsigned reg_id, warp_inst_t *&dest) {
    if (!sub_core_model) {
      return move_out_to(dest);
    }
    warp_inst_t **ready = get_ready(sub_core_model, reg_id);
    assert(ready != NULL);
    move_warp(dest, *ready);
  }

  warp_inst_t **get_ready() {
    warp_inst_t **ready;
    ready = NULL;
    for (unsigned i = 0; i < regs.size(); i++) {
      if (not regs[i]->empty()) {
        if (ready) {
          logger->trace("oldest={} uid = {}  <  ready={} uid = {}",
                        warp_instr_ptr(*ready), (*ready)->get_uid(),
                        warp_instr_ptr(regs[i]), regs[i]->get_uid());
        }
        if (ready and (*ready)->get_uid() < regs[i]->get_uid()) {
          // ready is oldest
        } else {
          ready = &regs[i];
        }
      }
    }
    return ready;
  }
  warp_inst_t **get_ready(bool sub_core_model, unsigned reg_id) {
    if (!sub_core_model) return get_ready();

    // assert(0 && "sub core model"); // have sub core model for titan config
    warp_inst_t **ready;
    ready = NULL;
    assert(reg_id < regs.size());
    if (not regs[reg_id]->empty()) ready = &regs[reg_id];
    return ready;
  }

  warp_inst_t **get_free() {
    for (unsigned i = 0; i < regs.size(); i++) {
      if (regs[i]->empty()) {
        return &regs[i];
      }
    }
    assert(0 && "No free registers found");
    return NULL;
  }

  warp_inst_t **get_free(bool sub_core_model, unsigned reg_id) {
    // in subcore model, each sched has a one specific reg to use (based on
    // sched id)
    if (!sub_core_model) return get_free();

    // assert(0 && "sub core model"); // have sub core model for titan config
    assert(reg_id < regs.size());
    if (regs[reg_id]->empty()) {
      logger->trace("found free register at index {}", reg_id);
      return &regs[reg_id];
    }
    assert(0 && "No free register found");
    return NULL;
  }

  unsigned get_size() { return regs.size(); }

  friend class register_set_bridge;
  friend class core_bridge;

  friend std::ostream &operator<<(std::ostream &os, const register_set &reg);
  friend struct fmt::formatter<register_set>;

  std::shared_ptr<spdlog::logger> logger;

 private:
  std::vector<warp_inst_t *> regs;
  const char *m_name;
  unsigned m_id;
};

#include "fmt/core.h"
#include "fmt/format.h"

template <>
struct fmt::formatter<register_set> {
  constexpr auto parse(format_parse_context &ctx)
      -> format_parse_context::iterator {
    return ctx.end();
  }

  auto format(const register_set &reg, format_context &ctx) const
      -> format_context::iterator {
    return fmt::format_to(ctx.out(), "{}[{}]=[{}]", reg.m_name, reg.m_id,
                          fmt::join(reg.regs, ","));
  }
};
