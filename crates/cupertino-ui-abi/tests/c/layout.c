/* SPDX-License-Identifier: MPL-2.0 */
/*
 * Prints the size and alignment of every struct of the ABI and the offset of every field, as a C compiler sees them
 * through the installed header, in the format of abi/layout-1.txt. ci/c-sdk.py and the package's install test compile
 * it against the installed SDK and compare its output with that file, which tests/layout.rs compares with the Rust
 * side.
 */

#include <cupertinokit.h>

#include <stdalign.h>
#include <stddef.h>
#include <stdio.h>

#define STRUCT(type) printf("%s size %zu align %zu\n", #type, sizeof(type), alignof(type))
#define FIELD(type, field) printf("  %s %zu\n", #field, offsetof(type, field))

int main(void) {
  STRUCT(CkRuntime);
  FIELD(CkRuntime, bits);
  STRUCT(CkWaker);
  FIELD(CkWaker, bits);
  STRUCT(CkWindow);
  FIELD(CkWindow, bits);
  STRUCT(CkStr);
  FIELD(CkStr, ptr);
  FIELD(CkStr, len);
  STRUCT(CkRuntimeOptions);
  FIELD(CkRuntimeOptions, struct_size);
  FIELD(CkRuntimeOptions, abi_major);
  FIELD(CkRuntimeOptions, abi_minor);
  FIELD(CkRuntimeOptions, backend);
  FIELD(CkRuntimeOptions, app_id);
  STRUCT(CkEvent);
  FIELD(CkEvent, kind);
  FIELD(CkEvent, reserved);
  FIELD(CkEvent, window);
  FIELD(CkEvent, message);
  FIELD(CkEvent, value);
  FIELD(CkEvent, payload_offset);
  FIELD(CkEvent, payload_len);
  return 0;
}
