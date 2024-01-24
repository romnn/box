mod progress;

use chrono::offset::Local;
use clap::Parser;
use color_eyre::{
    eyre::{self, WrapErr},
    Help,
};
use console::{style, Style};
use futures::stream::{self, StreamExt};
use itertools::Itertools;
use std::io::Write;
use std::time::Duration;

use indicatif::ProgressBar;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use utils::fs::PathExt;
use validate::{
    materialized::{self, BenchmarkConfig, Benchmarks},
    options::{self, Command, Options},
    Target,
};

#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("benchmark {0} skipped")]
    Skipped(BenchmarkConfig),
    #[error("benchmark {0} canceled")]
    Canceled(BenchmarkConfig),
    #[error("benchmark {bench} failed")]
    Failed {
        bench: BenchmarkConfig,
        #[source]
        source: eyre::Report,
    },
}

impl From<Error> for Result<(), validate::RunError> {
    fn from(err: Error) -> Self {
        match err {
            Error::Failed { source, .. } => Err(validate::RunError::Failed(source)),
            _ => Ok(()),
        }
    }
}

async fn run_make(
    bench: &BenchmarkConfig,
    options: &Options,
    _bar: &indicatif::ProgressBar,
) -> Result<Duration, validate::RunError> {
    if let Command::Build(_) = options.command {
        if !options.force && bench.executable_path.is_file() {
            return Err(validate::RunError::Skipped);
        }
    }

    let makefile = bench.path.join("Makefile");
    if !makefile.is_file() {
        return Err(validate::RunError::from(eyre::eyre!(
            "Makefile at {} not found",
            makefile.display()
        )));
    }
    let mut cmd = async_process::Command::new("make");
    cmd.args(["-B", "-C", &*bench.path.to_string_lossy()]);
    if let Command::Clean(_) = options.command {
        cmd.arg("clean");
    }
    log::debug!("{:?}", &cmd);
    let start = Instant::now();
    let result = cmd.output().await.map_err(eyre::Report::from)?;
    if !result.status.success() {
        return Err(validate::RunError::Failed(
            utils::CommandError::new(&cmd, result).into_eyre(),
        ));
    }
    Ok(start.elapsed())
}

async fn run_benchmark(
    command: &Command,
    bench: BenchmarkConfig,
    options: &Options,
    bar: &ProgressBar,
) -> Result<Duration, validate::RunError> {
    bar.set_message(bench.name.clone());
    match command {
        Command::All(ref _opts) => unreachable!(),
        Command::Expand(ref _opts) => {
            println!("{}", &bench.uid);
            // println!("{:#?}", &bench.uid);
            let dur = Duration::from_millis(100);
            tokio::time::sleep(dur).await;
            Ok(dur)
        }
        Command::Profile(ref opts) => {
            validate::profile::profile(&bench, options, opts, bar).await
            // Ok(())
        }
        Command::AccelsimTrace(ref opts) => {
            validate::accelsim::trace(&bench, options, opts, bar).await
            // Ok(())
        }
        Command::Trace(ref opts) => {
            validate::trace::trace(&bench, options, opts, bar).await
            // Ok(())
        }
        Command::ExecSimulate(ref opts) => {
            validate::simulate::exec::simulate(bench, options, opts, bar).await
            // Ok(())
        }
        Command::Simulate(ref opts) => {
            validate::simulate::simulate(bench, options, opts, bar).await
            // Ok(())
        }
        Command::AccelsimSimulate(ref opts) => {
            validate::accelsim::simulate(&bench, options, opts, bar).await
            // Ok(())
        }
        Command::PlaygroundSimulate(ref opts) => {
            validate::playground::simulate(bench, options, opts, bar).await
            // Ok(())
        }
        Command::Build(_) | Command::Clean(_) => {
            run_make(&bench, options, bar).await
            // Ok(())
        }
        Command::Run(ref _opts) => unreachable!(),
        // run_custom_bench(&bench, options, opts, bar).await
        // Ok(())
        // }
    }
}

#[allow(dead_code)]
async fn run_custom_bench(
    _bench: &BenchmarkConfig,
    _options: &Options,
    _run_options: &options::Run,
    _bar: &indicatif::ProgressBar,
) -> Result<Duration, validate::RunError> {
    let start = Instant::now();
    // dbg!(run_options);
    // if let Command::Build(_) = options.command {
    //     if !options.force && bench.executable.is_file() {
    //         return Err(validate::RunError::Skipped);
    //     }
    // }
    Ok(start.elapsed())
}

fn parse_benchmarks(options: &Options) -> eyre::Result<Benchmarks> {
    let cwd = std::env::current_dir()?;
    let benches_file_path = options.benchmark_file_path();
    let benches_file_path = benches_file_path
        .canonicalize()
        .wrap_err_with(|| format!("{} does not exist", benches_file_path.display()))?;

    let base_dir = benches_file_path
        .parent()
        .ok_or_else(|| eyre::eyre!("{} has no parent base path", benches_file_path.display()))?
        .to_path_buf();
    let benchmarks = validate::Benchmarks::from(&benches_file_path)?;

    let materialize_path = benchmarks
        .config
        .materialize_to
        .as_ref()
        .map(|p| p.resolve(&base_dir));

    // the materialized config is the source of truth for downstream consumers
    let materialized = benchmarks.materialize(&base_dir)?;

    match materialize_path {
        Some(materialize_path) if !options.dry_run => {
            let mut materialize_file = validate::open_writable(&materialize_path)?;
            write!(
                &mut materialize_file,
                r"
##
## AUTO GENERATED! DO NOT EDIT
##
## this configuration was materialized from {} on {}
##

",
                benches_file_path.display(),
                Local::now().format("%d/%m/%Y %T"),
            )?;

            serde_yaml::to_writer(&mut materialize_file, &materialized)?;
            println!(
                "materialized to {}",
                materialize_path.relative_to(cwd).display()
            );
        }
        _ => {
            println!("dry-run: skipped materialization");
        }
    }

    Ok(materialized)
}

fn print_benchmark_result(
    command: &Command,
    bench_config: &BenchmarkConfig,
    result: Result<&Duration, &Error>,
    approx_elapsed: Duration,
    bar: &ProgressBar,
    _options: &Options,
) {
    let profile = |is_debug: bool| -> &str {
        if is_debug {
            "debug"
        } else {
            "release"
        }
    };
    let op = match command {
        Command::Profile(opts) => format!(
            "profiling[{}]",
            match (
                opts.use_nvprof.unwrap_or(true),
                opts.use_nsight.unwrap_or(true)
            ) {
                (true, true) => "nvprof+nsight",
                (true, false) => "nvprof",
                (false, true) => "nsight",
                (false, false) => "",
            }
        ),
        Command::Trace(_) => format!(
            "tracing [gpucachesim][{}]",
            profile(gpucachesim::is_debug())
        ),
        Command::AccelsimTrace(_) => {
            format!("tracing [accelsim][{}]", profile(accelsim_sim::is_debug()))
        }
        Command::Simulate(_) | Command::ExecSimulate(_) => format!(
            "simulating [gpucachesim][{}]",
            profile(gpucachesim::is_debug())
        ),
        Command::AccelsimSimulate(_) => format!(
            "simulating [accelsim][{}]",
            profile(accelsim_sim::is_debug()),
        ),
        Command::PlaygroundSimulate(_) => format!(
            "simulating [playground][{}]",
            profile(playground::is_debug()),
        ),
        Command::Build(_) => "building".to_string(),
        Command::Clean(_) => "cleaning".to_string(),
        Command::Expand(_) => "expanding".to_string(),
        Command::All(_) => unreachable!(),
        Command::Run(options::Run { target, .. }) => match target.unwrap_or(Target::Simulate) {
            Target::PlaygroundSimulate => format!(
                "simulating [playground][{}]",
                profile(playground::is_debug())
            ),
            Target::Simulate | Target::ExecDrivenSimulate => format!(
                "simulating [gpucachesim][{}]",
                profile(gpucachesim::is_debug())
            ),
            Target::AccelsimSimulate => format!(
                "simulating [accelsim][{}]",
                profile(accelsim_sim::is_debug())
            ),
            Target::Trace => format!(
                "tracing [gpucachesim][{}]",
                profile(gpucachesim::is_debug())
            ),
            Target::AccelsimTrace => {
                format!("tracing [accelsim][{}]", profile(accelsim_sim::is_debug()))
            }
            Target::Profile => "profile".to_string(),
        },
    };
    let op = style(op).cyan();
    let executable = std::env::current_dir().ok().map_or_else(
        || bench_config.executable_path.clone(),
        |cwd| bench_config.executable_path.relative_to(cwd),
    );
    let (color, status) = match result {
        Ok(precise_elapsed) => (
            Style::new().green(),
            format!("succeeded in {precise_elapsed:?}"),
        ),
        Err(Error::Canceled(_)) => (Style::new().color256(0), "canceled".to_string()),
        Err(Error::Skipped(_)) => (
            Style::new().yellow(),
            "skipped (already exists)".to_string(),
        ),
        Err(Error::Failed { source, .. }) => {
            static PREVIEW_LEN: usize = 75;
            let mut err_preview = source.to_string();
            if err_preview.len() > PREVIEW_LEN {
                err_preview = format!("{} ...", &err_preview[..err_preview.len().min(PREVIEW_LEN)]);
            }
            (
                Style::new().red(),
                format!("failed after {approx_elapsed:?}: {err_preview}"),
            )
        }
    };
    match command {
        Command::Build(_) | Command::Clean(_) => {
            bar.println(format!(
                "{} {:<20} [ {} ] {}",
                op,
                color.apply_to(&bench_config.name),
                executable.display(),
                color.apply_to(status),
            ));
        }
        _ => {
            let benchmark_config_id =
                format!("{}@{:<3}", &bench_config.name, bench_config.input_idx);
            // dbg!(&benchmark_config_id);
            // bar.println(format!(
            //     "{} {:<20} [ {} ][ {} {} ] {}",
            //     op,
            //     color.apply_to(benchmark_config_id),
            //     materialized::bench_config_name(&bench_config.name, &bench_config.values, true),
            //     executable.display(),
            //     bench_config.args.join(" "),
            //     color.apply_to(status),
            // ));
            bar.println(format!(
                "{} {:<20} [ {} ] {}",
                op,
                color.apply_to(benchmark_config_id),
                materialized::bench_config_name(&bench_config.name, &bench_config.values, true),
                color.apply_to(status),
            ));
        }
    };
}

fn available_concurrency(options: &Options, config: &materialized::Config) -> usize {
    let benchmark_concurrency = match options.command {
        Command::Profile(_) => config.profile.common.concurrency,
        Command::Trace(_) => config.trace.common.concurrency,
        Command::AccelsimTrace(_) => config.accelsim_trace.common.concurrency,
        Command::ExecSimulate(_) => config.exec_driven_simulate.common.concurrency,
        Command::Simulate(_) => config.simulate.common.concurrency,
        Command::AccelsimSimulate(_) => config.accelsim_simulate.common.concurrency,
        Command::PlaygroundSimulate(_) => config.playground_simulate.common.concurrency,
        Command::Build(_) | Command::Clean(_) => None, // no limit on concurrency
        Command::All(_) | Command::Expand(_) => Some(1),
        Command::Run(_) => Some(1),
    };

    let max_concurrency = num_cpus::get_physical();
    let concurrency = options
        .concurrency
        .or(benchmark_concurrency)
        .unwrap_or(max_concurrency);
    concurrency.min(max_concurrency)
}

impl Error {
    #[must_use]
    pub fn new(err: validate::RunError, bench_config: BenchmarkConfig) -> Self {
        match err {
            validate::RunError::Skipped => Error::Skipped(bench_config),
            validate::RunError::Failed(source) => Error::Failed {
                source,
                bench: bench_config,
            },
        }
    }
}

fn compute_per_command_bench_configs<'a>(
    materialized: &'a Benchmarks,
    commands: &[Command],
    options: &'a Options,
) -> eyre::Result<Vec<(Command, Vec<&'a BenchmarkConfig>)>> {
    // dbg!(&options.query);
    let queries: Vec<validate::benchmark::Input> = options
        .query
        .iter()
        .map(|q| {
            serde_json::from_str(&q.replace("'", "\""))
                .wrap_err_with(|| format!("failed to parse query {q:?}"))
        })
        .try_collect()?;

    let per_command_bench_configs: Vec<(_, _)> = commands
        .iter()
        .filter_map(|command| {
            use std::collections::{HashMap, HashSet};
            if let Command::Run(..) = command {
                return None;
            }
            let targets: HashSet<_> = command.targets().collect();
            let benchmark_names: HashMap<_, _> = materialized.benchmark_names();

            let mut bench_configs: Vec<_> = materialized
                .benchmark_configs()
                .filter(|bench_config| {
                    if !bench_config.common.enabled.unwrap_or(true) {
                        return false;
                    }

                    if !targets.contains(&bench_config.target) {
                        return false;
                    }

                    if !options.selected_benchmarks.is_empty() {
                        let name = bench_config.name.to_lowercase();

                        let is_match = options.selected_benchmarks.iter().any(|b| {
                            if b.contains("@") {
                                // must be an exact match
                                b.to_lowercase() == format!("{}@{}", name, bench_config.input_idx)
                            } else {
                                let name_exists = benchmark_names
                                    .get(&bench_config.target)
                                    .map(|names| names.contains(&b.to_lowercase()))
                                    .unwrap_or(false);
                                if !name_exists {
                                    name.contains(&b.to_lowercase())
                                } else {
                                    name == b.to_lowercase()
                                }
                            }
                        });
                        if !is_match {
                            return false;
                        }
                    }

                    if !options.query.is_empty() {
                        let is_match = queries
                            .iter()
                            .any(|query| bench_config.input_matches(query));
                        if !is_match {
                            return false;
                        }
                    }

                    // check for baseline
                    if let Ok(input) = gpucachesim::config::parse_input(&bench_config.values) {
                        if options.baseline && !input.is_baseline(options.parallel) {
                            return false;
                        }
                    }

                    true
                })
                .collect();

            if let Command::Build(_) | Command::Clean(_) = command {
                // do not build the same executables multiple times
                // dbg!(bench_configs.len());
                bench_configs.dedup_by_key(|bench_config| bench_config.executable_path.clone());
            }

            // sort benchmarks
            bench_configs.sort_by_key(|bench_config| {
                (
                    bench_config.target,
                    bench_config.name.clone(),
                    bench_config.input_idx,
                )
            });

            Some((command.clone(), bench_configs))
        })
        .collect();
    Ok(per_command_bench_configs)
}

fn compute_custom_bench_config<'a>(
    materialized: &'a Benchmarks,
    commands: &[Command],
) -> eyre::Result<Vec<(Command, Vec<BenchmarkConfig>)>> {
    commands
        .iter()
        .map(|cmd| match cmd {
            Command::Run(ref run_opts) => {
                let (_args, argv) = argmap::parse(run_opts.args.iter());
                let values: serde_json::Value = serde_json::to_value(&argv).map_err(|err| {
                    eyre::Report::from(err).wrap_err(
                        eyre::eyre!("failed to parse arguments")
                            .with_section(|| format!("{:#?}", argv)),
                    )
                })?;
                dbg!(&values);

                let target = run_opts.target.unwrap_or(Target::ExecDrivenSimulate);
                assert_eq!(target, Target::ExecDrivenSimulate);
                let input_config = materialized
                    .get_input_configs(target, run_opts.benchmark.clone())
                    .next()
                    .unwrap();
                dbg!(input_config);

                // let bench_config = BenchmarkConfig {
                //     name: "test",
                //     /// Relative index of the benchmark for this target.
                //     benchmark_idx: usize,
                //     uid: String,
                //
                //     path: PathBuf,
                //     executable: input.executable,
                //
                //     /// Input values for this benchmark config.
                //     values,
                //     /// Command line arguments for invoking the benchmark for this target.
                //     args: vec![],
                //     /// Relative index of the input configuration for this target.
                //     input_idx: 0,
                //
                //     common: validate::config::GenericBenchmark::default(),
                //
                //     target: run_opts.target,
                //     target_config: validate::TargetBenchmarkConfig::default(),
                // };

                // eprintln!["args={:?}", &args];
                // eprintln!["argv={:?}", &argv];
                Ok(Some((cmd.clone(), vec![])))
            }
            _ => Ok(None),
        })
        .map(Result::transpose)
        .filter_map(|x| x)
        .try_collect()
}

#[allow(clippy::too_many_lines)]
#[tokio::main(flavor = "multi_thread")]
async fn main() -> eyre::Result<()> {
    // let mut log_builder = env_logger::Builder::new();
    // log_builder.format(|buf, record| {
    //     use std::io::Write;
    //     let level_style = buf.default_level_style(record.level());
    //     writeln!(
    //         buf,
    //         "[ {} {} ] {}",
    //         // Local::now().format("%Y-%m-%dT%H:%M:%S"),
    //         level_style.value(record.level()),
    //         record.module_path().unwrap_or(""),
    //         record.args()
    //     )
    // });
    let log_after_cycle = std::env::var("LOG_AFTER")
        .unwrap_or_default()
        .parse::<u64>()
        .ok();

    if log_after_cycle.is_none() {
        gpucachesim::init_logging();
        // log_builder.filter_level(log::LevelFilter::Off);
        // log_builder.parse_default_env();
        // log_builder.init();
    }

    color_eyre::install()?;

    let dotenv_file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../das6.env");
    dotenv::from_path(&dotenv_file).ok();

    let start = Instant::now();
    let options = Arc::new(Options::parse());

    // parse benchmarks
    let materialized = parse_benchmarks(&options)?;

    if let Command::Expand(ref opts) = options.command {
        if opts.full {
            println!("{:#?}", &materialized);
            return Ok(());
        }
    }

    let concurrency = available_concurrency(&options, &materialized.config);
    println!("concurrency: {concurrency}");

    let commands = match &options.command {
        Command::All(_) => vec![
            // Command::Build(options::Build::default()),
            // profiling requires sudo so we skip for now
            // Command::Profile(options::Profile::default()),
            // Command::Trace(options::Trace::default()),
            // Command::AccelsimTrace(options::AccelsimTrace::default()),
            Command::ExecSimulate(options::Sim::default()),
            Command::Simulate(options::Sim::default()),
            Command::AccelsimSimulate(options::AccelsimSim::default()),
            Command::PlaygroundSimulate(options::PlaygroundSim::default()),
        ],
        other => vec![other.clone()],
    };

    let mut per_command_bench_configs =
        compute_per_command_bench_configs(&materialized, &commands, &options)?;

    // add custom bench commands
    let custom_bench_configs = compute_custom_bench_config(&materialized, &commands)?;
    for (command, bench_configs) in custom_bench_configs.iter() {
        per_command_bench_configs.push((command.clone(), bench_configs.iter().collect()));
    }

    let num_bench_configs = per_command_bench_configs
        .iter()
        .flat_map(|(_command, bench_configs)| bench_configs)
        .count();

    for (command, bench_configs) in per_command_bench_configs.iter() {
        log::info!(
            "command {:>30}  =>  {:<4} configurations",
            command.to_string(),
            bench_configs.len()
        );
    }

    // create progress bar
    let bar = Arc::new(ProgressBar::new(num_bench_configs as u64));
    if options.no_progress {
        bar.set_draw_target(indicatif::ProgressDrawTarget::hidden());
    }
    match options.command {
        Command::Expand(_) => {
            // manually draw
        }
        _ => {
            bar.enable_steady_tick(Duration::from_secs_f64(1.0 / 100.0));
        }
    }
    bar.set_style(progress::Style::default().into());

    let should_exit = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let mut results: Vec<Result<_, Error>> = Vec::new();

    for (command, bench_configs) in per_command_bench_configs {
        let step_results: Vec<Result<_, Error>> = stream::iter(bench_configs.into_iter())
            .map(|bench_config| {
                let options = options.clone();
                let bar = bar.clone();
                let should_exit = should_exit.clone();
                let bench_config = bench_config.clone();
                let command = command.clone();
                async move {
                    use std::sync::atomic::Ordering::Relaxed;

                    let start = Instant::now();
                    let res: Result<_, Error> = if should_exit.load(Relaxed) {
                        Err(Error::Canceled(bench_config.clone()))
                    } else {
                        run_benchmark(&command, bench_config.clone(), &options, &bar)
                            .await
                            .map_err(|err| Error::new(err, bench_config.clone()))
                    };
                    print_benchmark_result(
                        &command,
                        &bench_config,
                        res.as_ref(),
                        start.elapsed(),
                        &bar,
                        &options,
                    );

                    bar.inc(1);

                    match res {
                        Err(Error::Failed { .. }) if options.fail_fast => {
                            should_exit.store(true, Relaxed);
                        }
                        _ => {}
                    }
                    res
                }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;
        results.extend(step_results);
        if results
            .iter()
            .any(|res| matches!(res, Err(Error::Failed { .. })))
        {
            break;
        }
    }
    // do not finish the bar if a stage failed
    if results.len() == num_bench_configs {
        // bar.finish();
    }

    let _ = utils::fs::rchmod_writable(&materialized.config.results_dir);

    let (_succeeded, failed): (Vec<_>, Vec<_>) = utils::partition_results(results);

    let mut num_failed = 0;
    let mut num_skipped = 0;
    let mut num_canceled = 0;
    for err in failed {
        match err {
            Error::Failed { ref source, bench } => {
                num_failed += 1;
                eprintln!(
                    "============ {} ============",
                    style(format!(
                        "{:?}: {}@{} failed",
                        bench.target, bench.name, bench.input_idx
                    ))
                    .red()
                );
                eprintln!("{source:?}\n");
            }
            Error::Skipped(_) => num_skipped += 1,
            Error::Canceled(_) => num_canceled += 1,
        }
    }

    let failed_msg = style(format!("{num_failed} failed"));
    println!(
        "\n\n => ran {} benchmark configurations in {:?}: {} canceled, {} skipped, {}",
        num_bench_configs,
        start.elapsed(),
        num_canceled,
        num_skipped,
        if num_failed > 0 {
            failed_msg.red()
        } else {
            failed_msg
        },
    );

    std::process::exit(i32::from(num_failed > 0));
}
