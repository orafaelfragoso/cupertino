# cupertino-control

The desktop-control service connects local clients to the compositor for output-scoped observation, capture and
input. The compositor decides what each run may do; the controller has no UI and does not depend on CupertinoKit.

- `--serve`: the compositor starts this process with the desktop-control role. It serves the session's private
  socket and multiplexes client runs over its Wayland connection.
- `--session DISPLAY`: forwards newline-delimited JSON requests and replies through that display's controller socket.
- `--mcp [DISPLAY]`: serves MCP over standard input and output. Without a display argument it uses `WAYLAND_DISPLAY`.
  Tool discovery works offline; a desktop operation connects to the selected controller.

See [the desktop-control protocol and examples](../../docs/src/building.md#desktop-automation-development-strategy).
