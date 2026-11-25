# Windows Bluetooth playback discovery experiment

Run on Windows with `cargo run -p windows-bluetooth-prototype`. The tool asks
`AudioPlaybackConnection` for its device selector and lists up to 64 matching
devices by name, without printing their identifiers or opening a connection.
A zero count describes only the current enumeration; it does not establish
that the adapter or operating system cannot support a connection.

To try system-routed playback, run
`cargo run -p windows-bluetooth-prototype -- --connect`, choose a listed
number, and press Enter again to close the connection. Enter `r` to refresh the
list or `q` to quit without connecting; refresh is available even when no
candidates are found. Numbers refer to the most recently displayed list. The
tool checks connection state while waiting and
reports if the system closes it; it does not automatically reconnect. It
attempts to close the connection even if start, open, monitoring, or waiting
fails. This does not capture audio or provide a PCM
source to SoniMesh, and an open connection alone does not establish that sound
is audible. Device names may be personal; review output before sharing it.

On the development machine, Windows build 26200 with an Intel Bluetooth
adapter initially returned zero candidates on 2026-11-25; a later run returned
one. On a later attempt that day, both the desktop probe backend and the command
line experiment failed to open the sole candidate; the latter reported Windows
status 3 and HRESULT `0x8007001F`. The source's state was not established, so
the cause is unknown. These observations do not verify playback, latency, or an
application-readable PCM path. A successful open, close, and listening
experiment with a compatible source is still needed.
