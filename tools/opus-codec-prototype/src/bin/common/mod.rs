//! Shared command-line options for the Android audio labs.

#[derive(Debug, Eq, PartialEq)]
pub enum Mode {
    Check,
    Play { select_output: bool },
}

pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Mode, &'static str> {
    let mut args = args.into_iter();
    let mode = match args.next().as_deref() {
        Some("--check") => Mode::Check,
        Some("--play") => match args.next().as_deref() {
            None => {
                return Ok(Mode::Play {
                    select_output: false,
                });
            }
            Some("--select-output") => Mode::Play {
                select_output: true,
            },
            _ => return Err("usage: --check | --play [--select-output]"),
        },
        _ => return Err("usage: --check | --play [--select-output]"),
    };
    if args.next().is_some() {
        return Err("usage: --check | --play [--select-output]");
    }
    Ok(mode)
}

#[cfg(all(windows, feature = "playback"))]
pub fn prepare_output(
    select_output: bool,
) -> Result<
    (sonimesh_platform::CpalOutput, sonimesh_app::OutputSelection),
    Box<dyn std::error::Error>,
> {
    let (output, selection, _) = prepare_output_with_default_choice(select_output)?;
    Ok((output, selection))
}

#[cfg(all(windows, feature = "playback"))]
pub fn prepare_output_with_default_choice(
    select_output: bool,
) -> Result<
    (
        sonimesh_platform::CpalOutput,
        sonimesh_app::OutputSelection,
        bool,
    ),
    Box<dyn std::error::Error>,
> {
    use sonimesh_platform::{AudioOutput, CpalOutput};
    use std::io::{self, Write};

    let output = CpalOutput::new();
    let (probe, follow_default) = if select_output {
        let devices = output.devices()?;
        println!("0: System default output");
        for (index, device) in devices.iter().enumerate() {
            let name: String = device
                .display_name
                .chars()
                .filter(|ch| !ch.is_control())
                .take(160)
                .collect();
            println!(
                "{}: {}{}",
                index + 1,
                name,
                if device.is_default { " (default)" } else { "" }
            );
        }
        print!("Choose an output [0]: ");
        io::stdout().flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            return Err("output selection was cancelled".into());
        }
        let index = parse_choice(&input, devices.len())?;
        if index == 0 {
            (output.default_device_probe()?, true)
        } else {
            (output.device_probe(&devices[index - 1].id)?, false)
        }
    } else {
        (output.default_device_probe()?, true)
    };
    let config = probe.output_config(None, 8_192)?;
    let selection = sonimesh_app::OutputSelection {
        device: Some(probe.device.id),
        config,
    };
    Ok((output, selection, follow_default))
}

#[cfg(any(test, all(windows, feature = "playback")))]
fn parse_choice(input: &str, count: usize) -> Result<usize, &'static str> {
    let value = input.trim();
    let index = if value.is_empty() {
        0
    } else {
        value.parse().map_err(|_| "invalid output selection")?
    };
    if index > count {
        return Err("output selection is out of range");
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modes_and_rejects_extra_arguments() {
        let args = |values: &[&str]| values.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(parse_args(args(&["--check"])), Ok(Mode::Check));
        assert_eq!(
            parse_args(args(&["--play", "--select-output"])),
            Ok(Mode::Play {
                select_output: true
            })
        );
        assert!(parse_args(args(&["--check", "--select-output"])).is_err());
        assert!(parse_args(args(&["--play", "--select-output", "extra"])).is_err());
    }

    #[test]
    fn validates_choice_against_the_current_device_list() {
        assert_eq!(parse_choice("\n", 2), Ok(0));
        assert_eq!(parse_choice("2", 2), Ok(2));
        assert!(parse_choice("3", 2).is_err());
        assert!(parse_choice("-1", 2).is_err());
    }
}
