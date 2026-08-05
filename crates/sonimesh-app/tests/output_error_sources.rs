//! Structured output/render error source-chain tests.

use std::error::Error;

use sonimesh_app::{
    ChannelMappedRenderSourceError, OutputLifecycleError, SampleRateMappedRenderSourceError,
};
use sonimesh_audio::{ChannelMappingError, ResamplerError};
use sonimesh_platform::{OutputConfigError, OutputError, RenderSourceFactoryError};

fn source<'a>(error: &'a (dyn Error + 'static)) -> &'a (dyn Error + 'static) {
    error
        .source()
        .expect("composite output error must retain its cause")
}

#[test]
fn render_source_wrappers_retain_audio_causes() {
    let channel =
        ChannelMappedRenderSourceError::Mapping(ChannelMappingError::InvalidInputChannelCount);
    assert!(
        source(&channel)
            .downcast_ref::<ChannelMappingError>()
            .is_some()
    );
    assert!(Error::source(&ChannelMappedRenderSourceError::InvalidMaximumCallbackFrames).is_none());

    let sample_rate =
        SampleRateMappedRenderSourceError::Resampler(ResamplerError::InvalidInputSampleRate);
    assert!(
        source(&sample_rate)
            .downcast_ref::<ResamplerError>()
            .is_some()
    );
    assert!(Error::source(&SampleRateMappedRenderSourceError::ScratchSizeOverflow).is_none());
}

#[test]
fn platform_output_wrappers_retain_config_causes() {
    let factory = RenderSourceFactoryError::InvalidConfig(OutputConfigError::InvalidSampleRate);
    assert!(
        source(&factory)
            .downcast_ref::<OutputConfigError>()
            .is_some()
    );
    assert!(Error::source(&RenderSourceFactoryError::Build(String::from("test"))).is_none());

    let output = OutputError::InvalidConfig(OutputConfigError::InvalidChannelCount);
    assert!(
        source(&output)
            .downcast_ref::<OutputConfigError>()
            .is_some()
    );
    assert!(Error::source(&OutputError::Backend(String::from("test"))).is_none());
}

#[test]
fn output_lifecycle_wrappers_retain_platform_causes() {
    let source_factory = OutputLifecycleError::SourceFactory(
        RenderSourceFactoryError::InvalidConfig(OutputConfigError::InvalidRequestedCallbackFrames),
    );
    let platform_factory = source(&source_factory);
    assert!(
        platform_factory
            .downcast_ref::<RenderSourceFactoryError>()
            .is_some()
    );
    let config = source(platform_factory);
    assert!(config.downcast_ref::<OutputConfigError>().is_some());

    let output = OutputLifecycleError::Output(OutputError::InvalidConfig(
        OutputConfigError::RequestedExceedsMaximum,
    ));
    let platform_output = source(&output);
    assert!(platform_output.downcast_ref::<OutputError>().is_some());
    let config = source(platform_output);
    assert!(config.downcast_ref::<OutputConfigError>().is_some());

    assert!(Error::source(&OutputLifecycleError::MissingSelection).is_none());
}
