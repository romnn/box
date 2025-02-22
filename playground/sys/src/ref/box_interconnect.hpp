#pragma once

#include <cstdio>
#include <list>
#include <memory>

#include "spdlog/logger.h"
#include "intersim2/interconnect_interface.hpp"

extern InterconnectInterface *g_icnt_interface;

class BoxInterconnect : public InterconnectInterface {
 public:
  BoxInterconnect(std::shared_ptr<spdlog::logger> logger, FILE *stats_out)
      : InterconnectInterface(false, stats_out), logger(logger) {}

  // we override these functions
  void Init();
  void Push(unsigned input_deviceID, unsigned output_deviceID, void *data,
            unsigned int size);
  void *Pop(unsigned ouput_deviceID);
  void Advance();
  bool Busy() const;
  bool HasBuffer(unsigned deviceID, unsigned int size) const;

  std::shared_ptr<spdlog::logger> logger;

 protected:
  // std::vector<std::vector<std::vector<std::list<void *>>>>
  // simple_input_queue;
  std::vector<std::vector<std::vector<std::list<void *>>>> simple_output_queue;
};

static void BoxInterconnect_create(unsigned int n_shader, unsigned int n_mem) {
  g_icnt_interface->CreateInterconnect(n_shader, n_mem);
}

static void BoxInterconnect_init() { g_icnt_interface->Init(); }

static bool BoxInterconnect_has_buffer(unsigned input, unsigned int size) {
  return g_icnt_interface->HasBuffer(input, size);
}

static void BoxInterconnect_push(unsigned input, unsigned output, void *data,
                                 unsigned int size) {
  g_icnt_interface->Push(input, output, data, size);
}

static void *BoxInterconnect_pop(unsigned output) {
  return g_icnt_interface->Pop(output);
}

static void BoxInterconnect_transfer() { g_icnt_interface->Advance(); }

static bool BoxInterconnect_busy() { return g_icnt_interface->Busy(); }

static void BoxInterconnect_display_stats(FILE *fp) {
  g_icnt_interface->DisplayStats(fp);
}

static void BoxInterconnect_display_overall_stats(FILE *fp) {
  g_icnt_interface->DisplayOverallStats(fp);
}

static void BoxInterconnect_display_state(FILE *fp) {
  g_icnt_interface->DisplayState(fp);
}

static unsigned BoxInterconnect_get_flit_size() {
  return g_icnt_interface->GetFlitSize();
}

std::unique_ptr<BoxInterconnect> new_box_interconnect(
    const char *config_filename);
