#include "gpgpu_context.hpp"

#include "stream_manager.hpp"
#include "trace_gpgpu_sim.hpp"

void *gpgpu_sim_thread_sequential(void *ctx_ptr, FILE *fp) {
  gpgpu_context *ctx = (gpgpu_context *)ctx_ptr;
  // at most one kernel running at a time
  bool done;
  do {
    sem_wait(&(ctx->the_gpgpusim->g_sim_signal_start));
    done = true;
    if (ctx->the_gpgpusim->g_the_gpu->get_more_cta_left()) {
      done = false;
      ctx->the_gpgpusim->g_the_gpu->init();
      while (ctx->the_gpgpusim->g_the_gpu->active()) {
        ctx->the_gpgpusim->g_the_gpu->cycle();
        ctx->the_gpgpusim->g_the_gpu->deadlock_check();
      }
      ctx->the_gpgpusim->g_the_gpu->print_stats(fp);
      ctx->the_gpgpusim->g_the_gpu->update_stats();
      ctx->print_simulation_time(fp);
    }
    sem_post(&(ctx->the_gpgpusim->g_sim_signal_finish));
  } while (!done);
  sem_post(&(ctx->the_gpgpusim->g_sim_signal_exit));
  return NULL;
}

static void termination_callback(FILE *fp) {
  fprintf(fp, "GPGPU-Sim: *** exit detected ***\n");
  fflush(fp);
}

// void *gpgpu_sim_thread_concurrent(void *ctx_ptr) {
//   gpgpu_context *ctx = (gpgpu_context *)ctx_ptr;
//   atexit(termination_callback);
//   // concurrent kernel execution simulation thread
//   do {
//     if (g_debug_execution >= 3) {
//       printf(
//           "GPGPU-Sim: *** simulation thread starting and spinning waiting for
//           " "work ***\n");
//       fflush(stdout);
//     }
//     while (ctx->the_gpgpusim->g_stream_manager->empty_protected() &&
//            !ctx->the_gpgpusim->g_sim_done)
//       ;
//     if (g_debug_execution >= 3) {
//       printf("GPGPU-Sim: ** START simulation thread (detected work) **\n");
//       ctx->the_gpgpusim->g_stream_manager->print(stdout);
//       fflush(stdout);
//     }
//     pthread_mutex_lock(&(ctx->the_gpgpusim->g_sim_lock));
//     ctx->the_gpgpusim->g_sim_active = true;
//     pthread_mutex_unlock(&(ctx->the_gpgpusim->g_sim_lock));
//     bool active = false;
//     bool sim_cycles = false;
//     ctx->the_gpgpusim->g_the_gpu->init();
//     do {
//       // check if a kernel has completed
//       // launch operation on device if one is pending and can be run
//
//       // Need to break this loop when a kernel completes. This was a
//       // source of non-deterministic behaviour in GPGPU-Sim (bug 147).
//       // If another stream operation is available, g_the_gpu remains active,
//       // causing this loop to not break. If the next operation happens to be
//       // another kernel, the gpu is not re-initialized and the inter-kernel
//       // behaviour may be incorrect. Check that a kernel has finished and
//       // no other kernel is currently running.
//       if (ctx->the_gpgpusim->g_stream_manager->operation(&sim_cycles) &&
//           !ctx->the_gpgpusim->g_the_gpu->active())
//         break;
//
//       // functional simulation
//       if (ctx->the_gpgpusim->g_the_gpu->is_functional_sim()) {
//         trace_kernel_info_t *kernel =
//             ctx->the_gpgpusim->g_the_gpu->get_functional_kernel();
//         assert(kernel);
//         ctx->the_gpgpusim->gpgpu_ctx->func_sim->gpgpu_cuda_ptx_sim_main_func(
//             *kernel);
//         ctx->the_gpgpusim->g_the_gpu->finish_functional_sim(kernel);
//       }
//
//       // performance simulation
//       if (ctx->the_gpgpusim->g_the_gpu->active()) {
//         ctx->the_gpgpusim->g_the_gpu->cycle();
//         sim_cycles = true;
//         ctx->the_gpgpusim->g_the_gpu->deadlock_check();
//       } else {
//         if (ctx->the_gpgpusim->g_the_gpu->cycle_insn_cta_max_hit()) {
//           ctx->the_gpgpusim->g_stream_manager->stop_all_running_kernels();
//           ctx->the_gpgpusim->g_sim_done = true;
//           ctx->the_gpgpusim->break_limit = true;
//         }
//       }
//
//       active = ctx->the_gpgpusim->g_the_gpu->active() ||
//                !(ctx->the_gpgpusim->g_stream_manager->empty_protected());
//
//     } while (active && !ctx->the_gpgpusim->g_sim_done);
//     if (g_debug_execution >= 3) {
//       printf("GPGPU-Sim: ** STOP simulation thread (no work) **\n");
//       fflush(stdout);
//     }
//     if (sim_cycles) {
//       ctx->the_gpgpusim->g_the_gpu->print_stats();
//       ctx->the_gpgpusim->g_the_gpu->update_stats();
//       ctx->print_simulation_time();
//     }
//     pthread_mutex_lock(&(ctx->the_gpgpusim->g_sim_lock));
//     ctx->the_gpgpusim->g_sim_active = false;
//     pthread_mutex_unlock(&(ctx->the_gpgpusim->g_sim_lock));
//   } while (!ctx->the_gpgpusim->g_sim_done);
//
//   printf("GPGPU-Sim: *** simulation thread exiting ***\n");
//   fflush(stdout);
//
//   if (ctx->the_gpgpusim->break_limit) {
//     printf(
//         "GPGPU-Sim: ** break due to reaching the maximum cycles (or "
//         "instructions) **\n");
//     exit(1);
//   }
//
//   sem_post(&(ctx->the_gpgpusim->g_sim_signal_exit));
//   return NULL;
// }

void gpgpu_context::synchronize() {
  printf("GPGPU-Sim: synchronize waiting for inactive GPU simulation\n");
  the_gpgpusim->g_stream_manager->print(stdout);
  fflush(stdout);
  //    sem_wait(&g_sim_signal_finish);
  bool done = false;
  do {
    pthread_mutex_lock(&(the_gpgpusim->g_sim_lock));
    done = (the_gpgpusim->g_stream_manager->empty() &&
            !the_gpgpusim->g_sim_active) ||
           the_gpgpusim->g_sim_done;
    pthread_mutex_unlock(&(the_gpgpusim->g_sim_lock));
  } while (!done);
  printf("GPGPU-Sim: detected inactive GPU simulation thread\n");
  fflush(stdout);
  //    sem_post(&g_sim_signal_start);
}

void gpgpu_context::exit_simulation() {
  the_gpgpusim->g_sim_done = true;
  printf("GPGPU-Sim: exit_simulation called\n");
  fflush(stdout);
  sem_wait(&(the_gpgpusim->g_sim_signal_exit));
  printf("GPGPU-Sim: simulation thread signaled exit\n");
  fflush(stdout);
}

// REMOVE: ptx
// gpgpu_sim *gpgpu_context::gpgpu_ptx_sim_init_perf() {
//   srand(1);
//   print_splash();
//   func_sim->read_sim_environment_variables();
//   ptx_parser->read_parser_environment_variables();
//   option_parser_t opp = option_parser_create();
//
//   ptx_reg_options(opp);
//   func_sim->ptx_opcocde_latency_options(opp);
//
//   icnt_reg_options(opp);
//   the_gpgpusim->g_the_gpu_config = new gpgpu_sim_config(this);
//   the_gpgpusim->g_the_gpu_config->reg_options(
//       opp); // register GPU microrachitecture options
//
//   option_parser_cmdline(opp, sg_argc, sg_argv); // parse configuration
//   options fprintf(stdout, "GPGPU-Sim: Configuration options:\n\n");
//   option_parser_print(opp, stdout);
//   // Set the Numeric locale to a standard locale where a decimal point is a
//   // "dot" not a "comma" so it does the parsing correctly independent of the
//   // system environment variables
//   assert(setlocale(LC_NUMERIC, "C"));
//   the_gpgpusim->g_the_gpu_config->init();
//
//   the_gpgpusim->g_the_gpu =
//       new exec_gpgpu_sim(*(the_gpgpusim->g_the_gpu_config), this);
//   the_gpgpusim->g_stream_manager = new stream_manager(
//       (the_gpgpusim->g_the_gpu), func_sim->g_cuda_launch_blocking);
//
//   the_gpgpusim->g_simulation_starttime = time((time_t *)NULL);
//
//   sem_init(&(the_gpgpusim->g_sim_signal_start), 0, 0);
//   sem_init(&(the_gpgpusim->g_sim_signal_finish), 0, 0);
//   sem_init(&(the_gpgpusim->g_sim_signal_exit), 0, 0);
//
//   return the_gpgpusim->g_the_gpu;
// }
static bool g_save_embedded_ptx = false;
static bool g_keep_intermediate_files = false;
static bool g_ptx_save_converted_ptxplus = false;
static int g_occupancy_sm_number = 0;

void gpgpu_context::ptx_reg_options(option_parser_t opp) {
  option_parser_register(opp, "-save_embedded_ptx", OPT_BOOL,
                         &g_save_embedded_ptx,
                         "saves ptx files embedded in binary as <n>.ptx", "0");
  option_parser_register(opp, "-keep", OPT_BOOL, &g_keep_intermediate_files,
                         "keep intermediate files created by GPGPU-Sim when "
                         "interfacing with external programs",
                         "0");
  option_parser_register(opp, "-gpgpu_ptx_save_converted_ptxplus", OPT_BOOL,
                         &g_ptx_save_converted_ptxplus,
                         "Saved converted ptxplus to a file", "0");
  option_parser_register(opp, "-gpgpu_occupancy_sm_number", OPT_INT32,
                         &g_occupancy_sm_number,
                         "The SM number to pass to ptxas when getting register "
                         "usage for computing GPU occupancy. "
                         "This parameter is required in the config.",
                         "0");
}

// void gpgpu_context::start_sim_thread(int api) {
//   if (the_gpgpusim->g_sim_done) {
//     the_gpgpusim->g_sim_done = false;
//     if (api == 1) {
//       pthread_create(&(the_gpgpusim->g_simulation_thread), NULL,
//                      gpgpu_sim_thread_concurrent, (void *)this);
//     } else {
//       pthread_create(&(the_gpgpusim->g_simulation_thread), NULL,
//                      gpgpu_sim_thread_sequential, (void *)this);
//     }
//   }
// }

#define MAX(a, b) (((a) > (b)) ? (a) : (b))

void gpgpu_context::print_simulation_time(FILE *fp) {
  time_t current_time, difference, d, h, m, s;
  current_time = time((time_t *)NULL);
  difference = MAX(current_time - the_gpgpusim->g_simulation_starttime, 1);

  d = difference / (3600 * 24);
  h = difference / 3600 - 24 * d;
  m = difference / 60 - 60 * (h + 24 * d);
  s = difference - 60 * (m + 60 * (h + 24 * d));

  fflush(stderr);
  fflush(fp);
  fprintf(
      fp,
      "\n\ngpgpu_simulation_time = %u days, %u hrs, %u min, %u sec (%u sec)\n",
      (unsigned)d, (unsigned)h, (unsigned)m, (unsigned)s, (unsigned)difference);
  fprintf(fp, "gpgpu_simulation_rate = %u (inst/sec)\n",
          (unsigned)(the_gpgpusim->g_the_gpu->gpu_tot_sim_insn / difference));
  const unsigned cycles_per_sec =
      (unsigned)(the_gpgpusim->g_the_gpu->gpu_tot_sim_cycle / difference);
  fprintf(fp, "gpgpu_simulation_rate = %u (cycle/sec)\n", cycles_per_sec);
  fprintf(fp, "gpgpu_silicon_slowdown = %ux\n",
          the_gpgpusim->g_the_gpu->shader_clock() * 1000 / cycles_per_sec);
  fflush(stdout);
  fflush(fp);
}

// int gpgpu_context::gpgpu_opencl_ptx_sim_main_perf(trace_kernel_info_t *grid)
// {
//   the_gpgpusim->g_the_gpu->launch(grid);
//   sem_post(&(the_gpgpusim->g_sim_signal_start));
//   sem_wait(&(the_gpgpusim->g_sim_signal_finish));
//   return 0;
// }
