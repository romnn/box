#include "stats_wrapper.hpp"
#include "intersim2/stats.hpp"

Stats *StatCreate(const char *name, double bin_size, int num_bins) {
  Stats *newstat = new Stats(NULL, name, bin_size, num_bins);
  newstat->Clear();
  return newstat;
}

void StatClear(void *st) { ((Stats *)st)->Clear(); }

void StatAddSample(void *st, int val) { ((Stats *)st)->AddSample(val); }

double StatAverage(void *st) { return ((Stats *)st)->Average(); }

double StatMax(void *st) { return ((Stats *)st)->Max(); }

double StatMin(void *st) { return ((Stats *)st)->Min(); }

void StatDisp(FILE *fp, void *st) {
  fprintf(fp, "Stats for ");
  ((Stats *)st)->DisplayHierarchy();
  //   if (((Stats *)st)->NeverUsed()) {
  //      printf (" was never updated!\n");
  //   } else {
  fprintf(fp, "Min %f Max %f Average %f \n", ((Stats *)st)->Min(),
          ((Stats *)st)->Max(), StatAverage(st));
  ((Stats *)st)->Display();
  //   }
}
