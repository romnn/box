use crate::config::Parallelization;
use crate::sync::Arc;
use crate::{config, interconn as ic, mem_fetch, Simulator};
use color_eyre::eyre;

pub struct GTX1080 {
    pub config: Arc<config::GPU>,
    pub sim: Simulator<
        ic::SimpleInterconnect<ic::Packet<mem_fetch::MemFetch>>,
        crate::mcu::PascalMemoryControllerUnit,
    >,
}

impl std::ops::Deref for GTX1080 {
    type Target = Simulator<
        ic::SimpleInterconnect<ic::Packet<mem_fetch::MemFetch>>,
        crate::mcu::PascalMemoryControllerUnit,
    >;

    fn deref(&self) -> &Self::Target {
        &self.sim
    }
}

impl std::ops::DerefMut for GTX1080 {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.sim
    }
}

impl Default for GTX1080 {
    fn default() -> Self {
        let config = Arc::new(config::GPU::default());
        Self::new(config)
    }
}

impl GTX1080 {
    pub fn new(config: Arc<config::GPU>) -> Self {
        let interconn = Arc::new(ic::SimpleInterconnect::new(
            config.num_simt_clusters,
            config.total_sub_partitions(),
        ));
        let mem_controller =
            Arc::new(crate::mcu::PascalMemoryControllerUnit::new(&config).unwrap());
        // let mem_controller: Arc<dyn crate::mcu::MemoryController> = if config.accelsim_compat {
        //     Arc::new(mcu::MemoryControllerUnit::new(&config).unwrap())
        // } else {
        //     Arc::new(mcu::PascalMemoryControllerUnit::new(&config).unwrap())
        // };

        // let mem_controller: Arc<dyn mcu::MemoryController> = if config.accelsim_compat {
        //     Arc::new(mcu::MemoryControllerUnit::new(&config).unwrap())
        // } else {
        //     Arc::new(mcu::PascalMemoryControllerUnit::new(&config).unwrap())
        // };
        // TODO: REMOVE
        // let mem_controller: Arc<dyn mcu::MemoryController> =
        //     Arc::new(mcu::MemoryControllerUnit::new(&config).unwrap());

        let mut sim = Simulator::new(interconn, mem_controller, Arc::clone(&config));

        sim.log_after_cycle = config.log_after_cycle;
        Self { config, sim }
    }
}

pub fn build_config(input: &crate::config::Input) -> eyre::Result<crate::config::GPU> {
    let parallelization = match (
        input
            .parallelism_mode
            .as_deref()
            .map(str::to_lowercase)
            .as_deref(),
        input.parallelism_run_ahead,
    ) {
        (Some("serial") | None, _) => Parallelization::Serial,
        #[cfg(feature = "parallel")]
        (Some("deterministic"), _) => Parallelization::Deterministic,
        #[cfg(feature = "parallel")]
        (Some("nondeterministic" | "nondeterministic_interleave"), run_ahead) => {
            Parallelization::Nondeterministic {
                run_ahead: run_ahead.unwrap_or(10),
            }
        }
        (Some(other), _) => panic!("unknown parallelization mode: {other}"),
        #[cfg(not(feature = "parallel"))]
        _ => {
            use color_eyre::Help;
            return Err(eyre::eyre!("parallel feature is disabled")
                .suggestion(format!(r#"enable the "parallel" feature"#)));
        }
    };
    let log_after_cycle = std::env::var("LOG_AFTER")
        .unwrap_or_default()
        .parse::<u64>()
        .ok();

    // 8 mem controllers * 2 sub partitions = 16 (l2s_count from nsight)
    let mut config = crate::config::GPU {
        parallelization,
        log_after_cycle,
        simulation_threads: input.parallelism_threads,
        ..crate::config::GPU::default()
    };

    if let Some(mem_only) = input.memory_only {
        config.memory_only = mem_only;
    }
    if let Some(num_clusters) = input.num_clusters {
        config.num_simt_clusters = num_clusters;
    }
    if let Some(num_cores_per_cluster) = input.cores_per_cluster {
        config.num_cores_per_simt_cluster = num_cores_per_cluster;
    }

    Ok(config)
}
