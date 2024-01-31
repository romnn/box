use color_eyre::eyre;
use itertools::Itertools;
use stats::{
    cache::{AccessStat, AccessStatus, RequestStatus, ReservationFailure},
    mem::AccessKind,
};
use std::collections::HashMap;
use strum::IntoEnumIterator;

pub type Stat = (String, usize, String);
pub type Map = indexmap::IndexMap<Stat, f64>;

/// Stats
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub is_release_build: bool,
    pub inner: Map,
}

impl IntoIterator for Stats {
    type Item = (Stat, f64);
    type IntoIter = indexmap::map::IntoIter<Stat, f64>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
    }
}

impl FromIterator<(Stat, f64)> for Stats {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = (Stat, f64)>,
    {
        Self {
            is_release_build: false,
            inner: iter.into_iter().collect(),
        }
    }
}

impl Stats {
    pub fn find_stat(&self, name: impl AsRef<str>) -> Option<&f64> {
        self.inner.iter().find_map(|((_, _, stat_name), value)| {
            if stat_name == name.as_ref() {
                Some(value)
            } else {
                None
            }
        })
    }
}

impl std::ops::Deref for Stats {
    type Target = Map;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl std::ops::DerefMut for Stats {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let mut stats = self.inner.clone();
        stats.sort_keys();

        let mut s = f.debug_struct("Stats");
        for ((current_kernel, running_kcount, stat_name), value) in &stats {
            s.field(
                &format!("{current_kernel} / {running_kcount} / {stat_name}"),
                value,
            );
        }
        s.finish_non_exhaustive()
    }
}

#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn convert_cache_stats(
    kernel_info: stats::KernelInfo,
    cache_name: &str,
    stats: &Stats,
) -> stats::PerCache {
    let mut cache_stats = stats::Cache::default();
    for kind in AccessKind::iter() {
        for reservation_failure in ReservationFailure::iter() {
            let per_cache_stat = stats.get(&(
                kernel_info.mangled_name.to_string(),
                kernel_info.launch_id,
                format!("{cache_name}_{kind:?}_{reservation_failure:?}"),
            ));
            let access = AccessStatus((kind, AccessStat::ReservationFailure(reservation_failure)));
            cache_stats.inner.insert(
                (None, access),
                per_cache_stat.copied().unwrap_or(0.0) as usize,
            );
        }
        for status in RequestStatus::iter() {
            let per_cache_stat = stats.get(&(
                kernel_info.mangled_name.to_string(),
                kernel_info.launch_id,
                format!("{cache_name}_{kind:?}_{status:?}"),
            ));
            let access = AccessStatus((kind, AccessStat::Status(status)));
            cache_stats.inner.insert(
                (None, access),
                per_cache_stat.copied().unwrap_or(0.0) as usize,
            );
        }
    }

    // dbg!(&cache_stats);
    // dbg!(format!("{cache_name}_total_accesses"));
    // dbg!(stats.get(&key!(format!("{cache_name}_total_accesses"))));
    //
    // if let Some(total_accesses) = stats.get(&key!(format!("{cache_name}_total_accesses"))) {
    //     assert_eq!(*total_accesses, cache_stats.total_accesses() as f64);
    // }

    // accelsim only reports the sum of all cache statistics
    stats::PerCache {
        kernel_info,
        ..stats::PerCache::from_iter([cache_stats])
    }
}

impl TryFrom<Stats> for stats::PerKernel {
    type Error = eyre::Report;

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn try_from(stats: Stats) -> Result<Self, Self::Error> {
        let kernels: Vec<_> = stats
            .keys()
            .cloned()
            .map(|(kernel_name, kernel_launch_id, _)| (kernel_name, kernel_launch_id))
            .dedup()
            .collect();
        let kernel_stats = kernels
            .into_iter()
            .map(|(kernel_name, kernel_launch_id)| {
                let kernel_info = stats::KernelInfo {
                    mangled_name: kernel_name.clone(),
                    name: "".to_string(),
                    launch_id: kernel_launch_id,
                };

                let accesses: HashMap<(Option<usize>, AccessKind), u64> = [
                    (None, AccessKind::GLOBAL_ACC_R, "num_global_mem_read"),
                    (None, AccessKind::LOCAL_ACC_R, "num_local_mem_read"),
                    (
                        None,
                        AccessKind::CONST_ACC_R,
                        "num_const_mem_total_accesses",
                    ),
                    (
                        None,
                        AccessKind::TEXTURE_ACC_R,
                        "num_tex_mem_total_accesses",
                    ),
                    (None, AccessKind::GLOBAL_ACC_W, "num_global_mem_write"),
                    (None, AccessKind::LOCAL_ACC_W, "num_local_mem_write"),
                    // the following metrics are not printed out by accelsim (internal?)
                    // (AccessKind::L1_WRBK_ACC, 0),
                    // (AccessKind::L2_WRBK_ACC, 0),
                    // (AccessKind::INST_ACC_R, 0),
                    // (AccessKind::L1_WR_ALLOC_R, 0),
                    // (AccessKind::L2_WR_ALLOC_R, 0),
                ]
                .into_iter()
                .map(|(alloc_id, kind, stat)| {
                    (
                        (alloc_id, kind),
                        stats
                            .get(&(kernel_name.clone(), kernel_launch_id, stat.to_string()))
                            .copied()
                            .unwrap_or(0.0) as u64,
                        // stats.get(&key!(stat)).copied().unwrap_or(0.0) as u64,
                    )
                })
                .collect();

                let instructions = stats::InstructionCounts {
                    kernel_info: kernel_info.clone(),
                    ..stats::InstructionCounts::default()
                };

                let l2_data_stats = convert_cache_stats(kernel_info.clone(), "l2_cache", &stats);
                let l1_inst_stats =
                    convert_cache_stats(kernel_info.clone(), "l1_inst_cache", &stats);
                let l1_data_stats =
                    convert_cache_stats(kernel_info.clone(), "l1_data_cache", &stats);
                let l1_const_stats =
                    convert_cache_stats(kernel_info.clone(), "l1_const_cache", &stats);
                let l1_tex_stats = convert_cache_stats(kernel_info.clone(), "l1_tex_cache", &stats);

                let total_dram_reads = stats
                    .get(&(
                        kernel_name.clone(),
                        kernel_launch_id,
                        "total_dram_reads".to_string(),
                    ))
                    .copied()
                    .unwrap_or(0.0) as u64;
                let total_dram_writes = stats
                    .get(&(
                        kernel_name.clone(),
                        kernel_launch_id,
                        "total_dram_writes".to_string(),
                    ))
                    .copied()
                    .unwrap_or(0.0) as u64;

                let mut bank_accesses =
                    ndarray::Array4::from_elem((1, 1, 1, AccessKind::count()), 0);
                bank_accesses[(0, 0, 0, AccessKind::GLOBAL_ACC_R as usize)] = total_dram_reads;
                bank_accesses[(0, 0, 0, AccessKind::GLOBAL_ACC_W as usize)] = total_dram_writes;

                let dram = stats::DRAM {
                    kernel_info: kernel_info.clone(),
                    bank_accesses,
                    num_banks: 1,
                    num_cores: 1,
                    num_chips: 1,
                };

                stats::Stats {
                    sim: stats::Sim {
                        kernel_name: "".to_string(),
                        kernel_name_mangled: kernel_name.clone(),
                        kernel_launch_id,
                        cycles: stats
                            .get(&(
                                kernel_name.clone(),
                                kernel_launch_id,
                                "gpu_tot_sim_cycle".to_string(),
                            ))
                            .copied()
                            .unwrap_or(0.0) as u64,
                        instructions: stats
                            .get(&(
                                kernel_name.clone(),
                                kernel_launch_id,
                                "gpu_total_instructions".to_string(),
                            ))
                            .copied()
                            .unwrap_or(0.0) as u64,
                        num_blocks: stats
                            .get(&(
                                kernel_name.clone(),
                                kernel_launch_id,
                                "num_issued_blocks".to_string(),
                            ))
                            .copied()
                            .unwrap_or(0.0) as u64,
                        elapsed_millis: 0,
                        is_release_build: stats.is_release_build,
                    },
                    accesses: stats::Accesses {
                        kernel_info: kernel_info.clone(),
                        ..stats::Accesses::from_iter(accesses)
                    },
                    dram,
                    instructions,
                    l1i_stats: l1_inst_stats,
                    l1t_stats: l1_tex_stats,
                    l1c_stats: l1_const_stats,
                    l1d_stats: l1_data_stats,
                    l2d_stats: l2_data_stats,
                    stall_dram_full: 0, // todo
                }
            })
            .collect();

        Ok(Self {
            kernel_stats,
            no_kernel: stats::Stats::empty(),
            config: stats::Config {
                num_total_cores: 1,
                num_mem_units: 1,
                num_dram_banks: 1,
                num_sub_partitions: 1,
            },
        })
    }
}
