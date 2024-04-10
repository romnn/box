#pragma once

#include <cstdio>
#include <map>

#include "address.hpp"
#include "mem_storage.hpp"

class ptx_thread_info;
class ptx_instruction;

enum _memory_space_t {
  undefined_space = 0,
  reg_space,
  local_space,
  shared_space,
  sstarr_space,
  param_space_unclassified,
  param_space_kernel, /* global to all threads in a kernel : read-only */
  param_space_local,  /* local to a thread : read-writable */
  const_space,
  tex_space,
  surf_space,
  global_space,
  generic_space,
  instruction_space
};

class memory_space_t {
public:
  memory_space_t() {
    m_type = undefined_space;
    m_bank = 0;
  }
  memory_space_t(const enum _memory_space_t &from) {
    m_type = from;
    m_bank = 0;
  }
  bool operator==(const memory_space_t &x) const {
    return (m_bank == x.m_bank) && (m_type == x.m_type);
  }
  bool operator!=(const memory_space_t &x) const { return !(*this == x); }
  bool operator<(const memory_space_t &x) const {
    if (m_type < x.m_type)
      return true;
    else if (m_type > x.m_type)
      return false;
    else if (m_bank < x.m_bank)
      return true;
    return false;
  }
  enum _memory_space_t get_type() const { return m_type; }
  void set_type(enum _memory_space_t t) { m_type = t; }
  unsigned get_bank() const { return m_bank; }
  void set_bank(unsigned b) { m_bank = b; }
  bool is_const() const {
    return (m_type == const_space) || (m_type == param_space_kernel);
  }
  bool is_local() const {
    return (m_type == local_space) || (m_type == param_space_local);
  }
  bool is_global() const { return (m_type == global_space); }

private:
  enum _memory_space_t m_type;
  unsigned m_bank;
};

class memory_space {
public:
  virtual ~memory_space() {}
  virtual void write(mem_addr_t addr, size_t length, const void *data,
                     ptx_thread_info *thd, const ptx_instruction *pI) = 0;
  virtual void write_only(mem_addr_t index, mem_addr_t offset, size_t length,
                          const void *data) = 0;
  virtual void read(mem_addr_t addr, size_t length, void *data) const = 0;
  virtual void print(const char *format, FILE *fout) const = 0;
  virtual void set_watch(addr_t addr, unsigned watchpoint) = 0;
};

template <unsigned BSIZE> class memory_space_impl : public memory_space {
public:
  memory_space_impl(std::string name, unsigned hash_size);

  virtual void write(mem_addr_t addr, size_t length, const void *data,
                     ptx_thread_info *thd, const ptx_instruction *pI);
  virtual void write_only(mem_addr_t index, mem_addr_t offset, size_t length,
                          const void *data);
  virtual void read(mem_addr_t addr, size_t length, void *data) const;
  virtual void print(const char *format, FILE *fout) const;

  virtual void set_watch(addr_t addr, unsigned watchpoint);

private:
  void read_single_block(mem_addr_t blk_idx, mem_addr_t addr, size_t length,
                         void *data) const;
  std::string m_name;
  unsigned m_log2_block_size;
  typedef std::map<mem_addr_t, mem_storage<(BSIZE)>> map_t;
  map_t m_data;
  std::map<unsigned, mem_addr_t> m_watchpoints;
};
