# Windows Bluetooth playback discovery experiment

Run on Windows with `cargo run -p windows-bluetooth-prototype`. The tool asks
`AudioPlaybackConnection` for its device selector and reports the number of
matching devices. It does not print device names or identifiers, open a
connection, or capture audio. A zero count describes only the current
enumeration; it does not establish that the adapter or operating system cannot
support a connection.

On the development machine, Windows build 26200 with an Intel Bluetooth
adapter returned zero candidates on 2026-11-25. No Bluetooth audio source was
verified and this result does not demonstrate playback, latency, or an
application-readable PCM path. A paired compatible source and a separate open,
close, and listening experiment are needed to assess system-routed playback.
