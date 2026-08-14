//! Deterministic virtual-network smoke runner for bounded audio buffering.

use sonimesh_audio::{DriftConfig, JitterMode, JitterTargetConfig};
use sonimesh_network_simulator::{
    DEFAULT_MIXER_CONTROLLER_PCM_BLOCK, DEFAULT_MIXER_CONTROLLER_SLOTS,
    DEFAULT_MIXER_JITTER_PCM_BLOCK, DEFAULT_MIXER_JITTER_SLOTS, DEFAULT_MIXER_MEMBERSHIP_PCM_BLOCK,
    DEFAULT_MIXER_MEMBERSHIP_SLOTS, DriftScenarioConfig, JitterSchedulerScenarioConfig,
    JitterTargetScenarioConfig, MixerControllerGainScript, MixerControllerScenarioConfig,
    MixerDriftScenarioConfig, MixerDriftStreamConfig, MixerJitterGainScript,
    MixerJitterSchedulerScenarioConfig, MixerMembershipScenarioConfig,
    MultiStreamJitterTargetScenarioConfig, MultiStreamScenarioConfig, NetworkProfile,
    ScenarioConfig, opus_experiment_cases, simulate, simulate_control_reconnect, simulate_drift,
    simulate_jitter_scheduler, simulate_jitter_target, simulate_mixer_controller_membership,
    simulate_mixer_drift, simulate_mixer_jitter_scheduler, simulate_mixer_membership,
    simulate_multi_stream, simulate_multi_stream_jitter_target, simulate_opus_experiment,
    simulate_opus_worker_experiment,
};

fn main() {
    println!("SoniMesh deterministic virtual-network profiles");
    for loss_percent in [0, 1, 3, 5] {
        let mut profile = NetworkProfile::stable(0x5A17_2026);
        profile.loss_percent = loss_percent;
        profile.jitter_ms = 8;
        profile.reorder_percent = 5;
        profile.duplicate_percent = 2;
        let report = simulate(ScenarioConfig {
            network: profile,
            ..ScenarioConfig::default()
        })
        .expect("built-in profiles are valid");

        println!(
            "loss={loss_percent:>2}% sent={} network_lost={} decoded={} plc={} duplicates={} late={} resets={}",
            report.network.submitted_packets,
            report.network.random_loss_packets + report.network.burst_loss_packets,
            report.playout_frames,
            report.plc_frames,
            report.duplicate_frames,
            report.late_frames,
            report.resynchronizations
        );
    }

    println!("SoniMesh codec-neutral Opus experiment suite");
    for case_ in opus_experiment_cases() {
        let report =
            simulate_opus_experiment(case_).expect("built-in Opus experiment cases are valid");
        let loss_variant = case_
            .independent_loss_percent
            .map_or_else(|| "n/a".to_owned(), |percent| format!("{percent}%"));
        println!(
            "opus={} frame={}ms bitrate={}bps independent_loss={} decoded={} plc={} network_lost={} reordered={}",
            case_.id.label(),
            case_.frame.frame_duration_ms(),
            case_.proposed_bitrate_bps,
            loss_variant,
            report.playout_frames,
            report.plc_frames,
            report.network.random_loss_packets + report.network.burst_loss_packets,
            report.reordered_arrivals,
        );
    }

    println!("SoniMesh codec-neutral mock decode-worker suite");
    for case_ in opus_experiment_cases() {
        let report = simulate_opus_worker_experiment(case_)
            .expect("built-in mock worker experiment cases are valid");
        println!(
            "mock_worker opus={} frame={}ms decoded={} plc={} resyncs={} pcm_high_water={} network_lost={}",
            case_.id.label(),
            case_.frame.frame_duration_ms(),
            report.decoded_frames,
            report.plc_frames,
            report.resynchronizations,
            report.queued_pcm_high_water_blocks,
            report.network.random_loss_packets + report.network.burst_loss_packets,
        );
    }

    let drift = simulate_drift(DriftScenarioConfig {
        ticks: 20_000,
        initial_occupancy_blocks: 5,
        source_rate_ppm: 100,
        controller: DriftConfig {
            target_blocks: 4,
            low_resync_blocks: 0,
            high_resync_blocks: 8,
            proportional_ppm_per_block: 25,
            integral_ppm_per_block_tick: 1,
            max_adjustment_ppm: 500,
            max_step_ppm: 25,
            max_integral_block_ticks: 256,
        },
    })
    .expect("built-in drift scenario is valid");
    println!(
        "drift source=100ppm final_occupancy={} final_adjustment={}ppm resyncs={}",
        drift.final_occupancy_blocks, drift.final_adjustment_ppm, drift.hard_resynchronizations
    );

    let control =
        simulate_control_reconnect().expect("built-in control reconnect scenario is valid");
    println!(
        "control_reconnect stale_completion_rejected={} fresh_hello_required={} stale_session_command_rejected={} fresh_session_command_accepted={}",
        control.retired.completion_rejected,
        control.fresh.hello_required,
        control.retired.command_rejected,
        control.fresh.command_accepted,
    );

    run_multi_stream_suite();
    run_mixer_membership_suite();
    run_mixer_drift_suite();
    run_mixer_controller_membership_suite();
    run_mixer_jitter_scheduler_suite();
    run_jitter_target_suite();
    run_multi_stream_jitter_target_suite();
    run_jitter_scheduler_suite();
}

fn run_multi_stream_suite() {
    println!("SoniMesh multi-stream shared-link suite");
    let mut network = NetworkProfile::stable(0x5A17_2026);
    network.burst_interval_packets = 20;
    network.burst_length_packets = 3;
    let report = simulate_multi_stream::<2>(MultiStreamScenarioConfig {
        frames: 200,
        network,
        ..MultiStreamScenarioConfig::default()
    })
    .expect("built-in multi-stream profile is valid");
    println!(
        "multi_stream streams=2 submitted={} burst_loss={} stream0_plc={} stream1_plc={}",
        report.network.submitted_packets,
        report.network.burst_loss_packets,
        report.streams[0].plc_frames,
        report.streams[1].plc_frames,
    );
}

fn run_mixer_membership_suite() {
    println!("SoniMesh fixed-slot mixer membership suite");
    let mut network = NetworkProfile::stable(0x5A17_2026);
    network.burst_interval_packets = 20;
    network.burst_length_packets = 3;
    let report = simulate_mixer_membership::<
        2,
        DEFAULT_MIXER_MEMBERSHIP_SLOTS,
        DEFAULT_MIXER_MEMBERSHIP_PCM_BLOCK,
    >(MixerMembershipScenarioConfig {
        frames: 200,
        network,
        ..MixerMembershipScenarioConfig::default()
    })
    .expect("built-in mixer membership profile is valid");
    println!(
        "mixer_membership streams=2 slots={} submitted={} burst_loss={} active={} inserts={} removes={} renders={} underflows={} finite={}",
        report.mixer.slots,
        report.network.submitted_packets,
        report.network.burst_loss_packets,
        report.mixer.active_streams,
        report.mixer.published_inserts,
        report.mixer.published_removes,
        report.mixer.renders,
        report.mixer.slot_underflows.iter().sum::<u64>(),
        report.mixer.output_finite,
    );
}

fn run_mixer_drift_suite() {
    println!("SoniMesh multi-stream mixer-drift suite");
    let report = simulate_mixer_drift(MixerDriftScenarioConfig {
        ticks: 20_000,
        controller: DriftConfig {
            target_blocks: 4,
            low_resync_blocks: 0,
            high_resync_blocks: 8,
            proportional_ppm_per_block: 25,
            integral_ppm_per_block_tick: 1,
            max_adjustment_ppm: 500,
            max_step_ppm: 25,
            max_integral_block_ticks: 256,
        },
        streams: [
            MixerDriftStreamConfig {
                initial_occupancy_blocks: 5,
                source_rate_ppm: 100,
            },
            MixerDriftStreamConfig {
                initial_occupancy_blocks: 3,
                source_rate_ppm: -80,
            },
            MixerDriftStreamConfig {
                initial_occupancy_blocks: 4,
                source_rate_ppm: 1_000,
            },
        ],
    })
    .expect("built-in mixer-drift profile is valid");
    for (index, stream) in report.streams.iter().enumerate() {
        println!(
            "mixer_drift stream={index} final_occupancy={} final_adjustment={}ppm resyncs={} (low={} high={})",
            stream.final_occupancy_blocks,
            stream.final_adjustment_ppm,
            stream.hard_resynchronizations,
            stream.low_resynchronizations,
            stream.high_resynchronizations,
        );
    }
}

fn run_mixer_controller_membership_suite() {
    println!("SoniMesh controller-driven mixer membership suite");
    let mut network = NetworkProfile::stable(0x5A17_2026);
    network.burst_interval_packets = 20;
    network.burst_length_packets = 3;
    let report = simulate_mixer_controller_membership::<
        2,
        DEFAULT_MIXER_CONTROLLER_SLOTS,
        DEFAULT_MIXER_CONTROLLER_PCM_BLOCK,
    >(MixerControllerScenarioConfig {
        frames: 200,
        network,
        // Muting every stream mid-session proves the all-muted bus goes
        // exactly silent once the bounded ramp converges, then stream 0
        // unmutes to prove per-source control does not stall the others.
        gain_scripts: [
            MixerControllerGainScript {
                mute_at_frame: Some(150),
                unmute_at_frame: Some(180),
                ..MixerControllerGainScript::default()
            },
            MixerControllerGainScript {
                mute_at_frame: Some(150),
                ..MixerControllerGainScript::default()
            },
        ],
        ..MixerControllerScenarioConfig::default()
    })
    .expect("built-in mixer controller membership profile is valid");
    println!(
        "mixer_controller_membership streams=2 slots={} submitted={} burst_loss={} active={} projected={} inserts={} removes={} renders={} projection_matched={} underflows={} stream0_plc_ratio={} stream1_plc_ratio={} muted0={} muted1={} silent_ticks={} finite={}",
        report.mixer.slots,
        report.network.submitted_packets,
        report.network.burst_loss_packets,
        report.mixer.active_streams,
        report.mixer.projected_streams,
        report.mixer.published_inserts,
        report.mixer.published_removes,
        report.mixer.renders,
        report.mixer.projection_matched,
        report.mixer.slot_underflows.iter().sum::<u64>(),
        report.streams[0].quality.plc_ratio_milli,
        report.streams[1].quality.plc_ratio_milli,
        report.streams[0].muted,
        report.streams[1].muted,
        report.mixer.all_muted_silent_ticks,
        report.mixer.output_finite,
    );
}

fn run_mixer_jitter_scheduler_suite() {
    println!("SoniMesh combined jitter-scheduler and mixer suite");
    let mut network = NetworkProfile::stable(0x5A17_2026);
    network.burst_interval_packets = 20;
    // A burst longer than the default gate deadline (8 ticks) is required to
    // exercise forced playouts; the scenario test pins this exact profile.
    network.burst_length_packets = 18;
    let report = simulate_mixer_jitter_scheduler::<
        2,
        DEFAULT_MIXER_JITTER_SLOTS,
        DEFAULT_MIXER_JITTER_PCM_BLOCK,
    >(MixerJitterSchedulerScenarioConfig {
        frames: 200,
        network,
        // Muting every stream mid-session proves the all-muted bus goes
        // exactly silent once the bounded ramp converges, then stream 0
        // unmutes to prove per-source control does not stall the others.
        gain_scripts: [
            MixerJitterGainScript {
                mute_at_frame: Some(150),
                unmute_at_frame: Some(180),
                ..MixerJitterGainScript::default()
            },
            MixerJitterGainScript {
                mute_at_frame: Some(150),
                ..MixerJitterGainScript::default()
            },
        ],
        ..MixerJitterSchedulerScenarioConfig::default()
    })
    .expect("built-in mixer jitter scheduler profile is valid");
    println!(
        "mixer_jitter_scheduler streams=2 slots={} submitted={} burst_loss={} active={} renders={} stream0_plc={} stream1_plc={} stream0_plc_ratio={} stream1_plc_ratio={} stream0_forced={} stream1_forced={} underflows={} muted0={} muted1={} silent_ticks={} finite={}",
        report.mixer.slots,
        report.network.submitted_packets,
        report.network.burst_loss_packets,
        report.mixer.active_streams,
        report.mixer.renders,
        report.streams[0].playout.plc_frames,
        report.streams[1].playout.plc_frames,
        report.streams[0].quality.plc_ratio_milli,
        report.streams[1].quality.plc_ratio_milli,
        report.streams[0].forced_playouts,
        report.streams[1].forced_playouts,
        report.mixer.slot_underflows.iter().sum::<u64>(),
        report.streams[0].muted,
        report.streams[1].muted,
        report.mixer.all_muted_silent_ticks,
        report.mixer.output_finite,
    );

    let mut four_stream_network = NetworkProfile::stable(7);
    // Four streams share one link, so a burst of `N` consecutive dropped
    // packets starves each stream for about `N/4` ticks; 36/40 starves each
    // stream ~9 ticks, past the 8-tick gate deadline, exercising forced
    // playouts on the Phase 2 upper bound.
    four_stream_network.burst_interval_packets = 40;
    four_stream_network.burst_length_packets = 36;
    let four_stream_report = simulate_mixer_jitter_scheduler::<
        4,
        DEFAULT_MIXER_JITTER_SLOTS,
        DEFAULT_MIXER_JITTER_PCM_BLOCK,
    >(MixerJitterSchedulerScenarioConfig {
        frames: 200,
        network: four_stream_network,
        ..MixerJitterSchedulerScenarioConfig::default()
    })
    .expect("built-in four-stream mixer jitter scheduler profile is valid");
    let four_stream_forced: u32 = four_stream_report
        .streams
        .iter()
        .map(|stream| stream.forced_playouts)
        .sum();
    println!(
        "mixer_jitter_scheduler streams=4 slots={} submitted={} burst_loss={} active={} renders={} forced={} underflows={} finite={}",
        four_stream_report.mixer.slots,
        four_stream_report.network.submitted_packets,
        four_stream_report.network.burst_loss_packets,
        four_stream_report.mixer.active_streams,
        four_stream_report.mixer.renders,
        four_stream_forced,
        four_stream_report.mixer.slot_underflows.iter().sum::<u64>(),
        four_stream_report.mixer.output_finite,
    );
}

fn run_jitter_target_suite() {
    println!("SoniMesh adaptive jitter-target suite");
    let mut network = NetworkProfile::stable(0x5A17_2026);
    network.jitter_ms = 8;
    network.reorder_percent = 5;
    let report = simulate_jitter_target(JitterTargetScenarioConfig {
        frames: 300,
        network,
        target: JitterTargetConfig::preset(JitterMode::Balanced, 20, 64),
        ..JitterTargetScenarioConfig::default()
    })
    .expect("built-in jitter-target profile is valid");
    println!(
        "jitter_target frames={} final_target={} (min={} base={} max={}) smoothed_jitter={}ms raised={} lowered={} buffered={}",
        report.observations,
        report.final_target_frames,
        report.min_frames,
        report.base_frames,
        report.max_frames,
        report.final_smoothed_jitter_ms,
        report.raised_steps,
        report.lowered_steps,
        report.final_buffered_frames,
    );
}

fn run_jitter_scheduler_suite() {
    println!("SoniMesh target-gated playout scheduler suite");
    let mut network = NetworkProfile::stable(0x5A17_2026);
    network.burst_interval_packets = 20;
    network.burst_length_packets = 3;
    let report = simulate_jitter_scheduler(JitterSchedulerScenarioConfig {
        frames: 200,
        network,
        ..JitterSchedulerScenarioConfig::default()
    })
    .expect("built-in scheduler profile is valid");
    println!(
        "scheduler ticks={} consumed={} gated={} forced={} final_target={} (min={} base={} max={}) raised={} lowered={} plc={} excess_latency={}ms",
        report.observations,
        report.playout.playout_frames + report.playout.plc_frames,
        report.gated_ticks,
        report.forced_playouts,
        report.final_target_frames,
        report.min_frames,
        report.base_frames,
        report.max_frames,
        report.raised_steps,
        report.lowered_steps,
        report.playout.plc_frames,
        report.excess_latency_ms,
    );
}

fn run_multi_stream_jitter_target_suite() {
    println!("SoniMesh multi-stream adaptive jitter-target suite");
    let mut network = NetworkProfile::stable(0x5A17_2026);
    network.burst_interval_packets = 20;
    network.burst_length_packets = 3;
    let report = simulate_multi_stream_jitter_target::<2>(MultiStreamJitterTargetScenarioConfig {
        frames: 200,
        network,
        ..MultiStreamJitterTargetScenarioConfig::default()
    })
    .expect("built-in multi-stream jitter-target profile is valid");
    println!(
        "multi_stream_jitter_target submitted={} burst_loss={}",
        report.network.submitted_packets, report.network.burst_loss_packets,
    );
    for (index, stream) in report.streams.iter().enumerate() {
        println!(
            "multi_stream_jitter_target stream={index} final_target={} (min={} base={} max={}) raised={} lowered={} plc={}",
            stream.final_target_frames,
            stream.min_frames,
            stream.base_frames,
            stream.max_frames,
            stream.raised_steps,
            stream.lowered_steps,
            stream.playout.plc_frames,
        );
    }
}
