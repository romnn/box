#pragma once

#include "../memory_partition_unit.hpp"
#include "../memory_sub_partition.hpp"
#include "../io.hpp"
#include "mem_fetch.hpp"
#include "cache.hpp"

class memory_partition_unit_bridge {
 public:
  memory_partition_unit_bridge(memory_partition_unit *ptr) : ptr(ptr) {}

  std::unique_ptr<std::vector<mem_fetch_ptr_shim>> get_dram_latency_queue()
      const {
    std::vector<mem_fetch_ptr_shim> q;
    std::list<memory_partition_unit::dram_delay_t>::const_iterator iter;
    for (iter = (ptr->m_dram_latency_queue).begin();
         iter != (ptr->m_dram_latency_queue).end(); iter++) {
      q.push_back(mem_fetch_ptr_shim{iter->req});
    }
    return std::make_unique<std::vector<mem_fetch_ptr_shim>>(q);
  }

  int get_last_borrower() const {
    return ptr->m_arbitration_metadata.last_borrower();
  }

  int get_shared_credit() const {
    return ptr->m_arbitration_metadata.m_shared_credit;
  }

  const std::vector<int> &get_private_credit() const {
    return ptr->m_arbitration_metadata.m_private_credit;
  }

 private:
  class memory_partition_unit *ptr;
};

class rop_delay_shim {
 public:
  rop_delay_shim(uint64_t ready, const mem_fetch *fetch)
      : ready_cycle(ready), req(fetch) {}
  uint64_t get_ready() const { return ready_cycle; }
  const mem_fetch *get_fetch() const { return req; }

 private:
  uint64_t ready_cycle;
  const mem_fetch *req;
};

class memory_sub_partition_bridge {
 public:
  memory_sub_partition_bridge(const memory_sub_partition *ptr) : ptr(ptr) {}

  std::unique_ptr<std::vector<mem_fetch_ptr_shim>> get_fifo_queue(
      fifo_pipeline<mem_fetch> *fifo) const {
    std::vector<mem_fetch_ptr_shim> q;
    if (fifo != NULL) {
      fifo_data<mem_fetch> *ddp = fifo->m_head;
      while (ddp) {
        q.push_back(mem_fetch_ptr_shim{ddp->m_data});
        ddp = ddp->m_next;
      }
    }
    return std::make_unique<std::vector<mem_fetch_ptr_shim>>(q);
  }

  std::unique_ptr<std::vector<mem_fetch_ptr_shim>> get_icnt_L2_queue() const {
    return get_fifo_queue(ptr->m_icnt_L2_queue);
  }
  std::unique_ptr<std::vector<mem_fetch_ptr_shim>> get_L2_dram_queue() const {
    return get_fifo_queue(ptr->m_L2_dram_queue);
  }
  std::unique_ptr<std::vector<mem_fetch_ptr_shim>> get_dram_L2_queue() const {
    return get_fifo_queue(ptr->m_dram_L2_queue);
  }
  std::unique_ptr<std::vector<mem_fetch_ptr_shim>> get_L2_icnt_queue() const {
    return get_fifo_queue(ptr->m_L2_icnt_queue);
  }
  std::unique_ptr<std::vector<rop_delay_shim>> get_rop_delay_queue() const {
    std::vector<rop_delay_shim> out;
    std::vector<rop_delay_t> rop = queue_to_vector(ptr->m_rop);
    std::vector<rop_delay_t>::const_iterator iter;
    for (iter = rop.begin(); iter != rop.end(); iter++) {
      out.push_back(rop_delay_shim(iter->ready_cycle, iter->req));
    }

    return std::make_unique<std::vector<rop_delay_shim>>(out);
  }
  std::shared_ptr<cache_bridge> get_l2_data_cache() const {
    return new_cache_bridge(ptr->m_L2cache);
  }

 private:
  const class memory_sub_partition *ptr;
};
