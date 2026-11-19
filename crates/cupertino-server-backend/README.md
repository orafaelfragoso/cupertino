# `cupertino-server-backend`

Backend primitives of the Cupertino compositor (`layer = "core"`). Depends on no other `cupertino-server-*` crate (D25).

## Modules

- `session` (`udev` feature): Smithay `Session` and calloop `EventSource` over `cupertino-seat` (`LogindSession`, `LogindNotifier`, `SessionError`).
